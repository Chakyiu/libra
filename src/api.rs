//! HTTP surface: JSON in, SSE out. One endpoint per frontend feature.

use crate::db::Job;
use crate::error::{LibraError, LibraResult};
use crate::pipeline::{AskRequest, GeoFix};
use crate::search::types::{SearchKind, SearchQuery};
use crate::state::{App, Event};
use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, HeaderValue};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest as _;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tower_http::services::{ServeDir, ServeFile};

pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/models", get(models))
        .route("/chats", get(list_chats).post(create_chat))
        .route(
            "/chats/{id}",
            get(get_chat).delete(delete_chat).patch(rename_chat),
        )
        .route("/chats/{id}/stop", post(stop_chat))
        .route("/chats/{id}/location", post(provide_location))
        .route("/ask", post(ask))
        .route("/upload", post(upload))
        .route("/search", get(search))
        .route("/suggestions", get(suggestions))
        .route("/discover", get(discover))
        .route("/generate/image", post(generate_image))
        .route("/stocks/{symbol}", get(stock_chart))
        // The Automator's schedules: CRUD, run now, and run history.
        .route("/jobs", get(list_jobs).post(create_job))
        // The AI helper: one sentence in, a saved job out.
        .route("/jobs/from-text", post(job_from_text))
        .route(
            "/jobs/{id}",
            get(get_job).put(update_job).delete(delete_job),
        )
        .route("/jobs/{id}/run", post(run_job))
        .route("/jobs/{id}/runs", get(list_job_runs))
        .with_state(app.clone())
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            require_token,
        ));

    // The Next.js build is a static export; serve it straight from the binary.
    let static_dir = app.settings.get().static_dir.clone();
    let index = std::path::Path::new(&static_dir).join("index.html");
    let fallback = ServeDir::new(&static_dir).fallback(ServeFile::new(index));
    let origins = app.settings.get().allowed_origins.clone();

    Router::new()
        .nest("/api", api)
        .fallback_service(fallback)
        .layer(cors_layer(&origins))
}

/// `LIBRA_AUTH_TOKEN` turns the API from "open on my LAN" into "needs the token".
/// The static UI stays public — it is just an app shell; every route that reads or
/// writes data lives under /api and is gated here.
async fn require_token(
    State(app): State<Arc<App>>,
    req: Request,
    next: axum::middleware::Next,
) -> Result<Response, Response> {
    let expected = app.settings.get().auth_token.clone();
    if expected.is_empty() || token_matches(req.headers().get(AUTHORIZATION), &expected) {
        return Ok(next.run(req).await);
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": { "code": "unauthorized", "message": "a valid access token is required" } })),
    )
        .into_response())
}

fn token_matches(header: Option<&HeaderValue>, expected: &str) -> bool {
    let Some(raw) = header.and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let given = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();
    // Both sides are hashed first so the compare cannot leak the token's length.
    let (given, expected) = (
        sha2::Sha256::digest(given.as_bytes()),
        sha2::Sha256::digest(expected.as_bytes()),
    );
    given == expected
}

/// Same-origin by default. `LIBRA_ALLOWED_ORIGINS=*` restores the wide-open dev layer.
fn cors_layer(origins: &[String]) -> tower_http::cors::CorsLayer {
    use tower_http::cors::{AllowOrigin, Any};
    if origins.iter().any(|o| o == "*") {
        return tower_http::cors::CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
    }
    let allowed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o.trim()).ok())
        .collect();
    if allowed.is_empty() {
        return tower_http::cors::CorsLayer::new();
    }
    tower_http::cors::CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed))
        .allow_methods(Any)
        .allow_headers(Any)
}

async fn health(State(app): State<Arc<App>>) -> Json<Value> {
    let cfg = app.settings.get();
    Json(json!({
        "status": "ok",
        "redis": app.cache.enabled(),
        "searchProviders": app.search.provider_names(),
        "pageReader": crate::fetch::reader_kind(&cfg),
        "models": cfg.model_options(),
    }))
}

// ---------------------------------------------------------------- settings

async fn get_settings(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.settings.get().settings_json())
}

async fn put_settings(
    State(app): State<Arc<App>>,
    Json(patch): Json<Value>,
) -> LibraResult<Json<Value>> {
    let saved =
        crate::settings::apply_patch(&app.settings, &patch, std::path::Path::new(&app.db.path))?;
    Ok(Json(saved))
}

async fn models(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.settings.get().model_options())
}

// ------------------------------------------------------------------- chats

#[derive(Deserialize)]
struct ChatQuery {
    #[serde(default)]
    chat_id: Option<String>,
}

async fn list_chats(State(app): State<Arc<App>>) -> LibraResult<Json<Value>> {
    let chats = app.db.list_chats(100).await?;
    Ok(Json(json!(chats)))
}

async fn create_chat(
    State(app): State<Arc<App>>,
    Query(q): Query<ChatQuery>,
) -> LibraResult<Json<Value>> {
    let cfg = app.settings.get();
    let id = q
        .chat_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let chat = app.db.create_chat(&id, &cfg.now()).await?;
    Ok(Json(json!(chat)))
}

async fn get_chat(State(app): State<Arc<App>>, Path(id): Path<String>) -> LibraResult<Json<Value>> {
    let Some(chat) = app.db.get_chat(&id).await? else {
        return Err(LibraError::NotFound(format!("chat {id}")));
    };
    let messages = app.db.get_messages(&id).await?;
    Ok(Json(json!({ "chat": chat, "messages": messages })))
}

async fn delete_chat(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> LibraResult<StatusCode> {
    app.db.delete_chat(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_chat(State(app): State<Arc<App>>, Path(id): Path<String>) -> LibraResult<StatusCode> {
    app.pipeline.runs.cancel(&id);
    app.db.mark_stopped(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct LocationBody {
    #[serde(default)]
    latitude: Option<f64>,
    #[serde(default)]
    longitude: Option<f64>,
    #[serde(default)]
    timezone: String,
    #[serde(default)]
    denied: bool,
}

/// The browser's answer to a location request on the ask stream.
async fn provide_location(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<LocationBody>,
) -> StatusCode {
    app.pipeline.provide_location(
        &id,
        GeoFix {
            latitude: body.latitude.unwrap_or(f64::NAN),
            longitude: body.longitude.unwrap_or(f64::NAN),
            timezone: body.timezone,
            denied: body.denied,
        },
    );
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
    #[serde(default)]
    emoji: String,
}

async fn rename_chat(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> LibraResult<StatusCode> {
    app.db.rename_chat(&id, &body.title, &body.emoji).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------- ask

/// Runs the pipeline on its own task and streams events as SSE.
async fn ask(State(app): State<Arc<App>>, Json(body): Json<Value>) -> Response {
    let req: AskRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => return sse_error(&format!("invalid request body: {e}")),
    };
    if req.query.trim().is_empty() {
        return sse_error("query is required");
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let pipeline = app.pipeline.clone();
    tokio::spawn(async move {
        pipeline.ask(req, tx.clone()).await;
        drop(tx);
    });
    stream_events(rx)
}

fn sse_error(message: &str) -> Response {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let _ = tx.send(Event::Close(Some(message.to_string())));
    stream_events(rx)
}

/// Shared SSE plumbing: pipeline events -> `data: {...}` frames.
fn stream_events(rx: UnboundedReceiver<Event>) -> Response {
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(|event| {
        let payload = serde_json::to_string(&event.to_json()).unwrap_or_default();
        Ok::<_, Infallible>(SseEvent::default().data(payload))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default().interval(std::time::Duration::from_secs(10)))
        .into_response()
}

// ------------------------------------------------------------------- stocks

#[derive(Deserialize)]
struct StockQuery {
    #[serde(default)]
    range: String,
}

async fn stock_chart(
    State(app): State<Arc<App>>,
    Path(symbol): Path<String>,
    Query(q): Query<StockQuery>,
) -> LibraResult<Json<Value>> {
    let range = if q.range.is_empty() {
        "1d"
    } else {
        q.range.as_str()
    };
    match crate::widgets::stocks::chart(&app.http, &symbol, range).await {
        Ok(body) => Ok(Json(body)),
        Err(crate::widgets::stocks::ChartError::Rejected) => {
            Err(LibraError::BadRequest("unknown symbol or range".into()))
        }
        Err(crate::widgets::stocks::ChartError::Unavailable) => {
            Err(LibraError::NotFound(format!("no chart for {symbol}")))
        }
    }
}

// ------------------------------------------------------------------ upload

async fn upload(State(app): State<Arc<App>>, mut multipart: Multipart) -> LibraResult<Json<Value>> {
    let cfg = app.settings.get();
    let mut chat_id = String::new();
    let mut files: Vec<(String, String, Vec<u8>)> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| LibraError::BadRequest(format!("multipart: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "chatId" || name == "chat_id" {
            chat_id = field.text().await.unwrap_or_default();
            continue;
        }
        if name != "files" {
            continue;
        }
        let filename = field.file_name().unwrap_or("upload.txt").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| LibraError::BadRequest(format!("read upload: {e}")))?;
        if bytes.len() > cfg.max_upload_mb * 1024 * 1024 {
            return Err(LibraError::BadRequest(format!(
                "{} is larger than the {max} MB limit",
                filename,
                max = cfg.max_upload_mb
            )));
        }
        files.push((uuid::Uuid::new_v4().to_string(), filename, bytes.to_vec()));
    }

    if chat_id.trim().is_empty() {
        return Err(LibraError::BadRequest("chatId is required".into()));
    }

    let mut out = Vec::new();
    for (file_id, filename, bytes) in files {
        let chunks = app
            .uploads
            .ingest(&chat_id, &file_id, &filename, &bytes)
            .await?;
        out.push(json!({ "id": file_id, "name": filename, "chunks": chunks }));
    }
    Ok(Json(json!({ "files": out })))
}

// ------------------------------------------------------------------ search

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    num_results: Option<usize>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    freshness: Option<String>,
    #[serde(default)]
    page: Option<usize>,
}

async fn search(
    State(app): State<Arc<App>>,
    Query(p): Query<SearchParams>,
) -> LibraResult<Json<Value>> {
    let kind = SearchKind::parse(p.kind.as_deref().unwrap_or("web"));
    let mut query = SearchQuery {
        query: p.q,
        num_results: p.num_results.unwrap_or(10),
        country: p.country,
        language: p.language,
        freshness: p.freshness,
        page: p.page.map(|p| p as u32),
        ..Default::default()
    };
    let response = app.search.run(kind, &mut query).await;
    Ok(Json(
        serde_json::to_value(response).map_err(LibraError::any)?,
    ))
}

// ------------------------------------------------------- suggestions & co

#[derive(Deserialize)]
struct Q {
    q: String,
}

async fn suggestions(State(app): State<Arc<App>>, Query(p): Query<Q>) -> LibraResult<Json<Value>> {
    let cfg = app.settings.get();
    let items = crate::suggestions::suggestions(&app.http, &cfg, &p.q).await?;
    Ok(Json(json!(items)))
}

#[derive(Deserialize)]
struct DiscoverQuery {
    #[serde(default)]
    topic: Option<String>,
}

async fn discover(State(app): State<Arc<App>>, Query(p): Query<DiscoverQuery>) -> Json<Value> {
    let topic = p.topic.as_deref().unwrap_or("tech");
    Json(crate::discover::articles(&app.search, &app.cache, topic).await)
}

#[derive(Deserialize)]
struct ImageBody {
    prompt: String,
}

async fn generate_image(
    State(app): State<Arc<App>>,
    Json(b): Json<ImageBody>,
) -> LibraResult<Json<Value>> {
    let cfg = app.settings.get();
    let v = crate::media::generate(&app.http, &cfg, &b.prompt).await?;
    Ok(Json(v))
}

// ------------------------------------------------------------------ automator

#[derive(Deserialize)]
struct RunsQuery {
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_jobs(State(app): State<Arc<App>>) -> LibraResult<Json<Value>> {
    Ok(Json(json!(app.db.list_jobs().await?)))
}

#[derive(Deserialize)]
struct JobIdea {
    text: String,
}

/// Turn one sentence into a job and store it, switched on. The helper only drafts, so
/// the schedule and timezone go through exactly the checks the hand-written form gets.
async fn job_from_text(
    State(app): State<Arc<App>>,
    Json(body): Json<JobIdea>,
) -> LibraResult<Json<Value>> {
    let plan = crate::automator::plan(&app, &body.text).await?;
    let job = checked_job(&app, plan.job, None)?;
    app.db.save_job(&job).await?;
    Ok(Json(
        json!({ "job": job, "why": plan.why, "notes": plan.notes }),
    ))
}

async fn get_job(State(app): State<Arc<App>>, Path(id): Path<String>) -> LibraResult<Json<Value>> {
    let job = app
        .db
        .get_job(&id)
        .await?
        .ok_or_else(|| LibraError::NotFound(format!("job {id}")))?;
    Ok(Json(json!(job)))
}

async fn create_job(
    State(app): State<Arc<App>>,
    Json(body): Json<Job>,
) -> LibraResult<Json<Value>> {
    let job = checked_job(&app, body, None)?;
    app.db.save_job(&job).await?;
    Ok(Json(json!(job)))
}

async fn update_job(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<Job>,
) -> LibraResult<Json<Value>> {
    let existing = app
        .db
        .get_job(&id)
        .await?
        .ok_or_else(|| LibraError::NotFound(format!("job {id}")))?;
    let job = checked_job(&app, body, Some(&existing))?;
    app.db.save_job(&job).await?;
    Ok(Json(json!(job)))
}

async fn delete_job(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> LibraResult<StatusCode> {
    if !app.db.delete_job(&id).await? {
        return Err(LibraError::NotFound(format!("job {id}")));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Start a job outside its schedule. The run lands in the same history as a timed one.
async fn run_job(State(app): State<Arc<App>>, Path(id): Path<String>) -> LibraResult<Json<Value>> {
    let job = app
        .db
        .get_job(&id)
        .await?
        .ok_or_else(|| LibraError::NotFound(format!("job {id}")))?;
    let run_id = crate::automator::run_now(&app, &job).await?;
    Ok(Json(json!({ "runId": run_id })))
}

async fn list_job_runs(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(q): Query<RunsQuery>,
) -> LibraResult<Json<Value>> {
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    Ok(Json(json!(app.db.list_job_runs(&id, limit).await?)))
}

/// Fill in what the form leaves out, and refuse a job that could never run — a typo in
/// the schedule should be a 400, not a silent never-fires.
fn checked_job(app: &Arc<App>, body: Job, existing: Option<&Job>) -> LibraResult<Job> {
    let cfg = app.settings.get();
    let id = if !body.id.trim().is_empty() {
        body.id.trim().to_string()
    } else {
        existing
            .map(|j| j.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    };
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(LibraError::BadRequest("a job needs a name".into()));
    }
    if !matches!(body.kind.as_str(), "command" | "prompt") {
        return Err(LibraError::BadRequest(
            "kind is either command or prompt".into(),
        ));
    }
    let (what, needs) = if body.kind == "prompt" {
        (&body.prompt, "a prompt job needs a question")
    } else {
        (&body.command, "a command job needs something to run")
    };
    if what.trim().is_empty() {
        return Err(LibraError::BadRequest(needs.into()));
    }
    let timezone = body.timezone.trim().to_string();
    if !timezone.is_empty() && timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(LibraError::BadRequest(format!(
            "'{timezone}' is not a timezone I recognise — try 'Europe/London', or leave it blank for the server's own"
        )));
    }
    let cron = crate::automator::normalize_cron(&body.cron)?;
    let next_run_at = crate::automator::next_run(&cron, &timezone, chrono::Utc::now())
        .ok_or_else(|| LibraError::BadRequest(format!("'{cron}' never fires")))?;
    Ok(Job {
        id,
        name,
        kind: body.kind.clone(),
        cron,
        timezone,
        enabled: body.enabled,
        image: body.image.trim().to_string(),
        command: body.command.clone(),
        prompt: body.prompt.clone(),
        memory: body.memory.trim().to_string(),
        // A schedule may not hold the machine hostage; microsandbox caps the VM anyway.
        max_duration_secs: body.max_duration_secs.clamp(10, 3600),
        notify_url: body.notify_url.trim().to_string(),
        // Editing a job must not reset its history or pretend it is new.
        created_at: existing
            .map(|j| j.created_at.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| cfg.now()),
        next_run_at,
        last_run_at: existing.map(|j| j.last_run_at.clone()).unwrap_or_default(),
        last_status: existing.map(|j| j.last_status.clone()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(v: &str) -> HeaderValue {
        HeaderValue::from_str(v).unwrap()
    }

    #[test]
    fn a_bearer_token_is_required_to_match() {
        assert!(token_matches(Some(&header("Bearer secret")), "secret"));
        assert!(token_matches(Some(&header("bearer secret")), "secret"));
        assert!(token_matches(Some(&header("secret")), "secret"));
        assert!(!token_matches(Some(&header("Bearer wrong")), "secret"));
        assert!(!token_matches(Some(&header("secret")), ""));
        assert!(!token_matches(None, "secret"));
    }
}
