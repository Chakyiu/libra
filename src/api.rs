//! HTTP surface: JSON in, SSE out. One endpoint per frontend feature.

use crate::error::{LibraError, LibraResult};
use crate::pipeline::{AskRequest, GeoFix};
use crate::search::types::{SearchKind, SearchQuery};
use crate::state::{App, Event};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
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
        .with_state(app.clone());

    // The Next.js build is a static export; serve it straight from the binary.
    let static_dir = app.settings.get().static_dir.clone();
    let index = std::path::Path::new(&static_dir).join("index.html");
    let fallback = ServeDir::new(&static_dir).fallback(ServeFile::new(index));

    Router::new()
        .nest("/api", api)
        .fallback_service(fallback)
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}

async fn health(State(app): State<Arc<App>>) -> Json<Value> {
    let cfg = app.settings.get();
    Json(json!({
        "status": "ok",
        "redis": app.cache.enabled(),
        "searchProviders": app.search.provider_names(),
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
