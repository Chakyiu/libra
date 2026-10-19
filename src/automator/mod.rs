//! The Automator: schedules and timely jobs, run on the Libra host.
//!
//! microsandbox is the isolation layer, not the scheduler — its own automation example
//! puts the timer outside the sandbox (docs.microsandbox.dev/examples/automation/dependency-audits),
//! so the cron loop lives here and each job starts a fresh, disposable microVM with the
//! `msb` CLI, which is what `libra compose` already shells out to.

use crate::db::{Job, JobRun};
use crate::error::{LibraError, LibraResult};
use crate::pipeline::AskRequest;
use crate::state::{App, Event};
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde_json::json;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// How often the due list is checked. Cron's finest unit is a minute, so this is slack.
const TICK: Duration = Duration::from_secs(20);
/// Output is stored for the UI, not shipped whole: a chatty job is clipped to this.
const MAX_OUTPUT_CHARS: usize = 20_000;

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
    let seconds = job.max_duration_secs.clamp(10, 3600) as u64;
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
    let budget = Duration::from_secs(job.max_duration_secs.clamp(60, 3600) as u64);
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

    let collect = async {
        let mut answer = String::new();
        let mut error: Option<String> = None;
        while let Some(event) = rx.recv().await {
            match event {
                Event::AnswerChunk(chunk) => answer.push_str(&chunk),
                Event::Close(Some(message)) => error = Some(message),
                _ => {}
            }
        }
        // A failed answer still leaves a thread, so keep the link either way.
        match error {
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
    };
    match tokio::time::timeout(budget, collect).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!("the answer did not finish within {}s", budget.as_secs()),
    }
}

/// Optional webhook per job. The keys ntfy understands are first, so a topic URL works
/// with no extra setup.
async fn notify(app: &Arc<App>, job: &Job, run: &JobRun) {
    let url = job.notify_url.trim();
    if url.is_empty() {
        return;
    }
    let body = if run.detail.trim().is_empty() {
        tail(&run.output)
    } else {
        run.detail.trim().to_string()
    };
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

/// Accept standard 5-field cron and the 6/7-field form, and store one expression the
/// scheduler can parse directly: seconds first, and weekdays counted the way cron
/// counts them. The `cron` crate numbers days 1 = Sunday … 7 = Saturday, so the
/// day-of-week numbers are shifted (`0 7 * * 1` means Monday 07:00, as everywhere else).
pub fn normalize_cron(expression: &str) -> Result<String, LibraError> {
    let expression = expression.trim();
    if expression.is_empty() {
        return Err(LibraError::BadRequest("a schedule is required".into()));
    }
    let fields = expression.split_whitespace().count();
    let with_seconds = match fields {
        5 => format!("0 {expression}"),
        6 | 7 => expression.to_string(),
        _ => return Err(invalid(expression)),
    };
    let shifted = shift_weekday(&with_seconds);
    if Schedule::from_str(&shifted).is_err() {
        return Err(invalid(expression));
    }
    Ok(shifted)
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
