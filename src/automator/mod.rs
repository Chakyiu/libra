//! The Automator: schedules and timely jobs, run on the Libra host.
//!
//! microsandbox is the isolation layer, not the scheduler — its own automation example
//! puts the timer outside the sandbox (docs.microsandbox.dev/examples/automation/dependency-audits),
//! so the cron loop lives here and each job starts a fresh, disposable microVM with the
//! `msb` CLI, which is what `libra compose` already shells out to.

use crate::db::{Job, JobRun};
use crate::error::{LibraError, LibraResult};
use crate::pipeline::AskRequest;
use crate::prompts;
use crate::state::{App, Event};
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// How often the due list is checked. Cron's finest unit is a minute, so this is slack.
const TICK: Duration = Duration::from_secs(20);
/// Output is stored for the UI, not shipped whole: a chatty job is clipped to this.
const MAX_OUTPUT_CHARS: usize = 20_000;
/// How long one run may take, per kind. A command job hands its number to microsandbox, so
/// it may be short. A prompt job is a whole search-and-answer, which takes minutes on a
/// quiet day, so it has a floor of its own — a 60-second budget there only ever reports a
/// job that was going fine.
const COMMAND_MIN_SECS: i64 = 10;
const PROMPT_MIN_SECS: i64 = 300;
const MAX_SECS: i64 = 3_600;

/// The stored budget made legal for this kind of job. The API clamps with this on save and
/// the scheduler reads it back, so an old row typed before the floor existed gets a real
/// budget without a migration.
pub fn clamp_duration(kind: &str, secs: i64) -> i64 {
    let floor = if kind == "prompt" {
        PROMPT_MIN_SECS
    } else {
        COMMAND_MIN_SECS
    };
    secs.clamp(floor, MAX_SECS)
}

fn budget_secs(job: &Job) -> i64 {
    clamp_duration(&job.kind, job.max_duration_secs)
}

/// A job's result, before it is written to `job_runs`.
struct Outcome {
    output: String,
    detail: String,
    exit_code: Option<i64>,
    chat_id: String,
    /// Whether the job succeeded. A prompt job has no exit code to read that off.
    ok: bool,
}

pub fn spawn(app: &Arc<App>) {
    let app = app.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            if let Err(e) = sweep(&app).await {
                tracing::warn!("automator sweep failed: {e}");
            }
        }
    });
}

/// Fire everything whose computed next-run time has passed, then reschedule it.
/// The next time is written before the job starts so a long run cannot fire twice.
async fn sweep(app: &Arc<App>) -> anyhow::Result<()> {
    let now = Utc::now();
    let stamp = format_time(now);
    for job in app.db.due_jobs(&stamp).await? {
        let next = match next_run(&job.cron, &job.timezone, now) {
            Some(t) => t,
            // Unreachable through the API, which validates on save. Retry in an hour
            // rather than logging every tick.
            None => {
                tracing::warn!("job '{}' has an unusable schedule: {}", job.name, job.cron);
                app.db
                    .bump_job(
                        &job.id,
                        &format_time(now + ChronoDuration::hours(1)),
                        &job.last_run_at,
                        "invalid schedule",
                    )
                    .await?;
                continue;
            }
        };
        app.db.bump_job(&job.id, &next, &stamp, "running").await?;
        let run_id = app.db.start_job_run(&job.id, &stamp).await?;
        let (app, job) = (app.clone(), job.clone());
        tokio::spawn(async move {
            execute(app, job, run_id).await;
        });
    }
    Ok(())
}

/// Run a job immediately, as the Run button does. Returns the run row id.
pub async fn run_now(app: &Arc<App>, job: &Job) -> LibraResult<i64> {
    let stamp = app.settings.get().now();
    let run_id = app.db.start_job_run(&job.id, &stamp).await?;
    app.db
        .bump_job(&job.id, &job.next_run_at, &stamp, "running")
        .await?;
    let (app, job) = (app.clone(), job.clone());
    tokio::spawn(async move {
        execute(app, job, run_id).await;
    });
    Ok(run_id)
}

async fn execute(app: Arc<App>, job: Job, run_id: i64) {
    let started = Utc::now();
    let outcome = match job.kind.as_str() {
        "prompt" => run_prompt(&app, &job).await,
        _ => run_command(&app, &job).await,
    };
    let (status, run) = match outcome {
        Ok(o) => (
            if o.ok { "ok" } else { "error" },
            JobRun {
                id: run_id,
                job_id: job.id.clone(),
                started_at: format_time(started),
                finished_at: app.settings.get().now(),
                status: if o.ok { "ok" } else { "error" }.into(),
                exit_code: o.exit_code,
                output: clip(&o.output),
                detail: if !o.ok && o.detail.is_empty() {
                    tail(&o.output)
                } else {
                    clip(&o.detail)
                },
                chat_id: o.chat_id,
            },
        ),
        Err(e) => (
            "error",
            JobRun {
                id: run_id,
                job_id: job.id.clone(),
                started_at: format_time(started),
                finished_at: app.settings.get().now(),
                status: "error".into(),
                exit_code: None,
                output: String::new(),
                detail: e.to_string(),
                chat_id: String::new(),
            },
        ),
    };
    if let Err(e) = app.db.finish_job_run(&run).await {
        tracing::warn!("could not record run {run_id}: {e}");
    }
    notify(&app, &job, &run).await;
    tracing::info!("automator: '{}' {status}", job.name);
}

/// One shell line inside a throwaway microVM. `msb` does the isolation and the
/// time limit; the outer timeout is only there in case the CLI itself hangs.
async fn run_command(app: &Arc<App>, job: &Job) -> anyhow::Result<Outcome> {
    let cfg = app.settings.get();
    let seconds = budget_secs(job) as u64;
    let limit = format!("{seconds}s");
    let image = pick(&job.image, "alpine");
    let memory = pick(&job.memory, "256M");
    // One sandbox per job, replaced on every run, so leftovers from a crashed run
    // cannot pile up.
    let name = format!("libra-{}", job.id.replace('-', ""));

    let mut cmd = tokio::process::Command::new(&cfg.msb_bin);
    cmd.arg("run")
        .arg("--name")
        .arg(&name)
        .arg("--replace")
        .arg("--memory")
        .arg(&memory)
        .arg("--max-duration")
        .arg(&limit)
        .arg(&image)
        .arg("--")
        .arg("sh")
        .arg("-lc")
        .arg(&job.command)
        .kill_on_drop(true);

    let output = match tokio::time::timeout(Duration::from_secs(seconds + 90), cmd.output()).await {
        Err(_) => anyhow::bail!("`{}` did not return within {}s", cfg.msb_bin, seconds + 90),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "`{}` is not on this machine. Install microsandbox (curl -fsSL https://install.microsandbox.dev | sh) \
             or point MSB_BIN at the binary.",
            cfg.msb_bin
        ),
        Ok(Err(e)) => return Err(e.into()),
        Ok(Ok(out)) => out,
    };
    Ok(Outcome {
        output: String::from_utf8_lossy(&output.stdout).into_owned(),
        detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().map(i64::from),
        chat_id: String::new(),
        ok: output.status.success(),
    })
}

/// Ask a question through the normal answer pipeline. The thread is stored like any
/// other, so the run links straight to it.
async fn run_prompt(app: &Arc<App>, job: &Job) -> anyhow::Result<Outcome> {
    let chat_id = uuid::Uuid::new_v4().to_string();
    let seconds = budget_secs(job);
    let budget = Duration::from_secs(seconds as u64);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let pipeline = app.pipeline.clone();
    let req = AskRequest {
        id: chat_id.clone(),
        query: job.prompt.clone(),
        model: None,
        models: Default::default(),
        message_history: Vec::new(),
        files: Vec::new(),
        filters: Default::default(),
        disabled_widgets: Vec::new(),
        depth: String::new(),
        mode: String::new(),
    };
    // Not awaited: an answer that runs past the budget is still stored as a thread.
    tokio::spawn(async move {
        pipeline.ask(req, tx).await;
    });

    // Both kinds of job are watched, not joined: a prompt answer that runs past its budget
    // is still stored as a thread, so a cut-off run reports what it has and keeps the link.
    let mut answer = String::new();
    let mut failure: Option<String> = None;
    let deadline = tokio::time::sleep(budget);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => {
                return Ok(Outcome {
                    output: answer,
                    detail: format!(
                        "no answer within {seconds}s — the part it wrote is kept in the thread, which carries on in the background"
                    ),
                    exit_code: None,
                    chat_id,
                    ok: false,
                });
            }
            event = rx.recv() => {
                match event {
                    Some(Event::AnswerChunk(chunk)) => answer.push_str(&chunk),
                    Some(Event::Close(message)) => {
                        failure = message;
                        break;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        }
    }
    drop(rx);
    match failure {
        Some(e) => Ok(Outcome {
            output: answer,
            detail: e,
            exit_code: None,
            chat_id,
            ok: false,
        }),
        None if answer.trim().is_empty() => Ok(Outcome {
            output: answer,
            detail: "the answer came back empty".into(),
            exit_code: None,
            chat_id,
            ok: false,
        }),
        None => Ok(Outcome {
            output: answer,
            detail: String::new(),
            exit_code: None,
            chat_id,
            ok: true,
        }),
    }
}

/// Optional webhook per job. The keys ntfy understands are first, so a topic URL works
/// with no extra setup.
/// What a finished run tells the outside world: the job's own webhook (an ntfy topic takes
/// it directly), and the instance-wide Telegram chat from Settings when it is configured
/// and the notification policy covers this run.
async fn notify(app: &Arc<App>, job: &Job, run: &JobRun) {
    let cfg = app.settings.get();
    let body = if run.detail.trim().is_empty() {
        tail(&run.output)
    } else {
        run.detail.trim().to_string()
    };
    let url = job.notify_url.trim();
    if !url.is_empty() {
        let payload = json!({
            "title": format!("Libra · {} · {}", job.name, run.status),
            "body": body,
            "job": job.name,
            "kind": job.kind,
            "status": run.status,
            "exitCode": run.exit_code,
            "startedAt": run.started_at,
            "finishedAt": run.finished_at,
            "chatId": run.chat_id,
        });
        if let Err(e) = app.http.post(url).json(&payload).send().await {
            tracing::warn!("notification for '{}' failed: {e}", job.name);
        }
    }
    if !crate::notify::announces(&cfg.notify_runs, &run.status) {
        return;
    }
    if let Err(e) = crate::notify::announce_run(&app.http, &cfg, job, run, &body).await {
        tracing::warn!("telegram notification for '{}' failed: {e}", job.name);
    }
}

/// What the AI helper made of a sentence: a job ready to store, plus what it decided.
pub struct Plan {
    pub job: Job,
    /// One sentence from the model: what will run, and when.
    pub why: String,
    /// Guesses the user should see before the first run — a schedule it had to invent,
    /// a timezone it could not read, a command that never arrived.
    pub notes: Vec<String>,
}

/// One sentence in, one job out. Nothing is stored and nothing runs here; the API
/// validates the job through `checked_job` and saves it. The light model is enough —
/// this is a form to fill, not a question to answer.
pub async fn plan(app: &Arc<App>, text: &str) -> LibraResult<Plan> {
    let text = text.trim();
    if text.is_empty() {
        return Err(LibraError::BadRequest("tell me what to schedule".into()));
    }
    let cfg = app.settings.get();
    let llm = crate::llm::resolve_chat(&cfg, &app.http, cfg.light_model())?;
    let user = prompts::fill(
        prompts::JOB_IDEA_USER,
        &[("date", &cfg.today()), ("text", text)],
    );
    let v = llm
        .json_call(prompts::JOB_IDEA_SYSTEM, &user, None)
        .await
        .map_err(|e| LibraError::Provider {
            provider: "automator helper".into(),
            message: e.to_string(),
        })?;
    Ok(draft(&v, text, &cfg.now()))
}

/// The model's JSON made into a job the scheduler can trust. Anything missing falls back
/// to the form's own defaults, the schedule and timezone go through the same checks a
/// hand-written job gets, and every substitution the user has not seen becomes a note.
/// A schedule the model got wrong never rejects the whole request — the day job is kept
/// and the user is told, because they can see the form it lands in.
fn draft(v: &Value, text: &str, now: &str) -> Plan {
    let mut notes: Vec<String> = Vec::new();
    let field = |key: &str| -> String { v[key].as_str().unwrap_or("").trim().to_string() };

    let mut kind = if field("kind") == "command" {
        "command"
    } else {
        "prompt"
    };
    let command = field("command");
    let mut prompt = field("prompt");
    if kind == "command" && command.is_empty() {
        kind = "prompt";
        notes.push("It did not give a command, so the job asks Libra instead.".into());
    }
    if kind == "prompt" && prompt.is_empty() {
        prompt = text.to_string();
        notes
            .push("With no question of its own, the job asks Libra exactly what you wrote.".into());
    }

    let mut timezone = field("timezone");
    if !timezone.is_empty() && timezone.parse::<Tz>().is_err() {
        notes.push(format!(
            "'{timezone}' is not a timezone I know, so the job uses the server's own."
        ));
        timezone = String::new();
    }

    let given = field("cron");
    let mut cron = match normalize_cron(&given) {
        Ok(cron) => cron,
        Err(_) => {
            notes.push(if given.is_empty() {
                "No schedule came with it, so it runs at 07:00 every day — change it if that is wrong.".into()
            } else {
                format!(
                    "'{given}' is not a schedule I can read, so it runs at 07:00 every day — change it if that is wrong."
                )
            });
            DAILY.to_string()
        }
    };
    let mut when = next_run(&cron, &timezone, Utc::now());
    if when.is_none() {
        notes.push(format!(
            "'{given}' never fires, so it runs at 07:00 every day instead."
        ));
        cron = DAILY.to_string();
        when = next_run(&cron, &timezone, Utc::now());
    }
    if fires_every_minute(&cron) {
        notes.push("This fires every minute, so runs will pile up — widen it unless that is what you meant.".into());
    }
    if kind == "command" {
        notes.push("A command job runs that shell line in a microVM on the Libra server. Check it before the first run.".into());
    }

    let name = {
        let named = field("name");
        if named.is_empty() {
            clip_name(text)
        } else {
            named
        }
    };

    let asked_secs = v["max_duration_secs"].as_i64().unwrap_or(300);
    let budget = clamp_duration(kind, asked_secs);
    if kind == "prompt" && budget != asked_secs {
        notes.push(format!(
            "An answer that searches takes minutes, not seconds, so this job waits {budget}s rather than {asked_secs}s."
        ));
    }

    Plan {
        job: Job {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            kind: kind.to_string(),
            cron,
            timezone,
            enabled: true,
            image: pick(&field("image"), "alpine"),
            command,
            prompt,
            memory: pick(&field("memory"), "256M"),
            max_duration_secs: budget,
            notify_url: field("notify_url"),
            created_at: now.to_string(),
            // Recomputed by checked_job on the way in; this is only so the draft is a
            // complete job on its own.
            next_run_at: when.unwrap_or_else(|| now.to_string()),
            last_run_at: String::new(),
            last_status: String::new(),
        },
        why: field("why"),
        notes,
    }
}

/// The schedule the helper falls back to when the model's cannot be read or never fires.
const DAILY: &str = "0 0 7 * * *";

/// After normalization the minute field is the second one; a bare `*` or a step under
/// five minutes is the one schedule mistake worth warning about.
fn fires_every_minute(cron: &str) -> bool {
    matches!(
        cron.split_whitespace().nth(1),
        Some("*") | Some("*/1") | Some("*/2") | Some("*/3") | Some("*/4")
    )
}

/// A name from the sentence itself when the model left the field empty.
fn clip_name(text: &str) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    if chars.len() <= 40 {
        return text.trim().to_string();
    }
    let mut name: String = chars[..40].iter().collect();
    while name.ends_with([' ', ',', ':', ';', '-']) {
        name.pop();
    }
    format!("{name}…")
}

/// Accept standard 5-field cron and the 6/7-field form, and store one expression the
/// scheduler can parse directly: seconds first, and weekdays counted the way the `cron`
/// crate counts them (1 = Sunday … 7 = Saturday), so `0 7 * * 1` means Monday 07:00,
/// as everywhere else. A 6/7-field expression is already in stored form and is kept as
/// it stands — shifting it again would walk the weekday one day every time a saved job
/// was edited or re-drafted.
pub fn normalize_cron(expression: &str) -> Result<String, LibraError> {
    let expression = expression.trim();
    if expression.is_empty() {
        return Err(LibraError::BadRequest("a schedule is required".into()));
    }
    let stored = match expression.split_whitespace().count() {
        5 => shift_weekday(&format!("0 {expression}")),
        6 | 7 => expression.to_string(),
        _ => return Err(invalid(expression)),
    };
    if Schedule::from_str(&stored).is_err() {
        return Err(invalid(expression));
    }
    Ok(stored)
}

fn invalid(expression: &str) -> LibraError {
    LibraError::BadRequest(format!(
        "'{expression}' is not a cron expression — try '0 7 * * *' for 07:00 every day, or '30 4 * * 1' for 04:30 on Mondays"
    ))
}

/// Rewrite the day-of-week field (6th once seconds are prefixed) from cron's 0-7 to the
/// crate's 1-7. Names and `*` pass through untouched.
fn shift_weekday(expression: &str) -> String {
    let mut parts: Vec<String> = expression.split_whitespace().map(str::to_string).collect();
    if parts.len() < 6 {
        return expression.to_string();
    }
    parts[5] = parts[5]
        .split(',')
        .map(|item| match item.split_once('/') {
            Some((range, step)) => format!("{}/{}", shift_range(range), step),
            None => shift_range(item),
        })
        .collect::<Vec<_>>()
        .join(",");
    parts.join(" ")
}

fn shift_range(item: &str) -> String {
    match item.split_once('-') {
        Some((from, to)) => format!("{}-{}", shift_day(from), shift_day(to)),
        None => shift_day(item),
    }
}

fn shift_day(day: &str) -> String {
    match day.parse::<u32>() {
        // cron: 0 and 7 are Sunday, 1 is Monday. The crate: 1 is Sunday, 2 is Monday.
        Ok(0) | Ok(7) => "1".into(),
        Ok(n @ 1..=6) => (n + 1).to_string(),
        _ => day.to_string(),
    }
}

/// Next fire time in UTC, as the RFC3339 stamps the rest of the app stores.
pub fn next_run(cron: &str, timezone: &str, after: DateTime<Utc>) -> Option<String> {
    let schedule = Schedule::from_str(cron.trim()).ok()?;
    let timezone = timezone.trim();
    let next = if timezone.is_empty() {
        schedule
            .after(&after.with_timezone(&Local))
            .next()?
            .with_timezone(&Utc)
    } else {
        let zone: Tz = timezone.parse().ok()?;
        schedule
            .after(&after.with_timezone(&zone))
            .next()?
            .with_timezone(&Utc)
    };
    Some(format_time(next))
}

fn format_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn pick(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
    format!(
        "{kept}\n… {} more characters clipped",
        text.chars().count() - MAX_OUTPUT_CHARS
    )
}

/// The last few lines, which is where a failure explanation usually is.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let skip = lines.len().saturating_sub(8);
    lines[skip..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stamp in the past, so every drafted job's next run is ahead of it.
    const NOW: &str = "2026-10-20T00:00:00Z";

    #[test]
    fn five_field_cron_is_normalized() {
        assert_eq!(normalize_cron("0 7 * * *").unwrap(), "0 0 7 * * *");
        assert_eq!(normalize_cron("0 0 7 * * *").unwrap(), "0 0 7 * * *");
        assert_eq!(normalize_cron("*/15 * * * *").unwrap(), "0 */15 * * * *");
        assert!(normalize_cron("").is_err());
        assert!(normalize_cron("not a cron").is_err());
        assert!(normalize_cron("0 7 * * 1 2 3 4").is_err());
    }

    #[test]
    fn weekdays_are_counted_the_way_cron_counts_them() {
        assert_eq!(normalize_cron("0 7 * * 1").unwrap(), "0 0 7 * * 2");
        assert_eq!(normalize_cron("0 7 * * 0").unwrap(), "0 0 7 * * 1");
        assert_eq!(normalize_cron("0 7 * * 7").unwrap(), "0 0 7 * * 1");
        assert_eq!(normalize_cron("0 9 * * 1-5").unwrap(), "0 0 9 * * 2-6");
        assert_eq!(
            normalize_cron("0 9 * * MON-FRI").unwrap(),
            "0 0 9 * * MON-FRI"
        );
        assert_eq!(normalize_cron("0 9 * * 1,4").unwrap(), "0 0 9 * * 2,5");
        // A stored job comes back through the form unchanged: editing it must not walk
        // the weekday another day.
        assert_eq!(normalize_cron("0 0 9 * * 2-6").unwrap(), "0 0 9 * * 2-6");
        assert_eq!(normalize_cron("0 0 7 * * *").unwrap(), "0 0 7 * * *");
        // 2026-10-19 is a Monday, so 1 must land on the 26th, not the 25th.
        let now = DateTime::parse_from_rfc3339("2026-10-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let monday = next_run(&normalize_cron("0 12 * * 1").unwrap(), "UTC", now).unwrap();
        assert_eq!(monday, "2026-10-26T12:00:00Z");
    }

    #[test]
    fn next_run_follows_the_expression_and_zone() {
        // 2026-07-08 06:00 UTC.
        let now = DateTime::parse_from_rfc3339("2026-07-08T06:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            next_run("0 0 7 * * *", "UTC", now).as_deref(),
            Some("2026-07-08T07:00:00Z")
        );
        // Auckland is UTC+12 in July: 06:00Z is already 18:00 there, so the next
        // 07:00 local is the following day's, which is 19:00Z today.
        assert_eq!(
            next_run("0 0 7 * * *", "Pacific/Auckland", now).as_deref(),
            Some("2026-07-08T19:00:00Z")
        );
        // Empty means the server's own zone; the run is still in the future.
        let local = next_run("0 0 7 * * *", "", now).unwrap();
        assert!(local.parse::<DateTime<Utc>>().unwrap() > now);
        assert!(next_run("0 0 7 * * *", "Mars/Olympus_Mons", now).is_none());
        assert!(next_run("nonsense", "UTC", now).is_none());
    }

    #[test]
    fn the_helper_draft_is_a_job_the_scheduler_can_store() {
        let v = json!({
            "name": "Rust release notes",
            "kind": "prompt",
            "cron": "0 8 * * 1-5",
            "timezone": "Europe/London",
            "prompt": "What changed in the Rust release notes since the last check?",
            "why": "Weekdays at 08:00 London time"
        });
        let plan = draft(&v, "every weekday morning tell me about rust releases", NOW);
        assert_eq!(plan.job.name, "Rust release notes");
        assert_eq!(plan.job.cron, "0 0 8 * * 2-6");
        assert_eq!(plan.job.timezone, "Europe/London");
        assert_eq!(plan.job.kind, "prompt");
        assert!(plan.job.enabled);
        assert_eq!(plan.job.image, "alpine");
        assert_eq!(plan.job.memory, "256M");
        assert_eq!(plan.job.max_duration_secs, 300);
        assert!(plan.notes.is_empty());
        assert!(plan.job.next_run_at.as_str() > NOW);
        assert_eq!(plan.why, "Weekdays at 08:00 London time");
    }

    #[test]
    fn a_schedule_the_helper_could_not_read_falls_back_to_daily() {
        let plan = draft(&json!({ "cron": "sometime soon" }), "do it soon", NOW);
        assert_eq!(plan.job.cron, DAILY);
        assert!(plan.notes.iter().any(|n| n.contains("sometime soon")));
        // No schedule at all is still a job, and the note says so without quoting "".
        let bare = draft(&json!({ "prompt": "x" }), "x", NOW);
        assert_eq!(bare.job.cron, DAILY);
        assert!(bare.notes.iter().any(|n| n.contains("No schedule")));
    }

    #[test]
    fn a_cron_that_never_fires_is_replaced() {
        // 30 February is a valid expression per field and has no fire time in it.
        let plan = draft(&json!({ "cron": "0 0 30 2 *" }), "on 30 february", NOW);
        assert_eq!(plan.job.cron, DAILY);
        assert!(plan.notes.iter().any(|n| n.contains("never fires")));
    }

    #[test]
    fn a_timezone_that_does_not_exist_uses_the_servers_own() {
        let plan = draft(
            &json!({ "timezone": "Mars/Olympus_Mons", "cron": "0 7 * * *", "prompt": "x" }),
            "x",
            NOW,
        );
        assert_eq!(plan.job.timezone, "");
        assert!(plan.notes.iter().any(|n| n.contains("Mars/Olympus_Mons")));
    }

    #[test]
    fn a_command_job_without_a_command_asks_a_question_instead() {
        let plan = draft(
            &json!({ "kind": "command", "cron": "0 7 * * *", "prompt": "disk report" }),
            "x",
            NOW,
        );
        assert_eq!(plan.job.kind, "prompt");
        assert_eq!(plan.job.prompt, "disk report");
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("did not give a command"))
        );
    }

    #[test]
    fn a_missing_prompt_falls_back_to_the_users_own_words() {
        let plan = draft(
            &json!({ "kind": "prompt", "cron": "0 7 * * *" }),
            "summarise the news",
            NOW,
        );
        assert_eq!(plan.job.prompt, "summarise the news");
        assert_eq!(plan.job.name, "summarise the news");
    }

    #[test]
    fn a_prompt_job_is_never_given_a_budget_it_cannot_meet() {
        let plan = draft(
            &json!({
                "kind": "prompt",
                "prompt": "what is the price of AAPL",
                "cron": "0 9 * * 1",
                "max_duration_secs": 60
            }),
            "x",
            NOW,
        );
        assert_eq!(plan.job.max_duration_secs, 300);
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("300s rather than 60s")),
            "{:?}",
            plan.notes
        );
        // A command job keeps the short limit: microsandbox stops the shell line itself.
        assert_eq!(clamp_duration("command", 60), 60);
        // A row stored before the floor existed still gets a real budget at run time.
        let mut job = plan.job;
        job.max_duration_secs = 30;
        assert_eq!(budget_secs(&job), 300);
        job.kind = "command".into();
        assert_eq!(budget_secs(&job), 30);
    }

    #[test]
    fn a_command_job_says_where_the_line_runs() {
        let plan = draft(
            &json!({
                "kind": "command",
                "command": "curl -s https://example.com",
                "cron": "*/15 * * * *",
                "image": "python:3.12",
                "memory": "512M",
                "max_duration_secs": 99_999
            }),
            "x",
            NOW,
        );
        assert_eq!(plan.job.cron, "0 */15 * * * *");
        assert_eq!(plan.job.image, "python:3.12");
        assert_eq!(plan.job.memory, "512M");
        // A schedule may not hold the machine hostage.
        assert_eq!(plan.job.max_duration_secs, 3600);
        assert!(plan.notes.iter().any(|n| n.contains("microVM")));
        assert!(!plan.notes.iter().any(|n| n.contains("every minute")));
    }

    #[test]
    fn a_schedule_that_fires_every_minute_is_warned_about() {
        let plan = draft(&json!({ "prompt": "x", "cron": "* * * * *" }), "x", NOW);
        assert!(plan.notes.iter().any(|n| n.contains("every minute")));
    }

    #[test]
    fn a_long_sentence_without_a_name_still_gets_one() {
        let long =
            "every weekday morning at eight check the rust release notes and tell me what changed";
        let plan = draft(&json!({ "prompt": "x", "cron": "0 7 * * *" }), long, NOW);
        assert!(plan.job.name.chars().count() <= 41);
        assert!(plan.job.name.ends_with('…'));
        assert!(plan.job.name.starts_with("every weekday morning"));
    }

    #[test]
    fn failure_tails_show_the_last_lines() {
        assert_eq!(tail("one\ntwo\nthree"), "one\ntwo\nthree");
        let long = (1..=20)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(tail(&long), "13\n14\n15\n16\n17\n18\n19\n20");
        assert!(clip(&(1..=30_000).map(|_| "x").collect::<String>()).contains("clipped"));
    }
}
