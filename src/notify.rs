//! Where Libra talks to the outside world on its own: Telegram, used by the Automator to
//! announce a finished run. A job's own `notify_url` webhook is separate — it stays a plain
//! JSON `POST` that ntfy can consume — because Telegram needs a bot token, a chat, and a
//! form body, none of which belong on the job form.

use crate::config::Config;
use crate::db::{Job, JobRun};
use crate::error::LibraError;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;

/// Telegram cuts a message at 4096 characters; leave room for the job line and the tail note.
const MAX_TEXT: usize = 3_800;

/// A chat that has spoken to the bot, as the Settings form lists them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chat {
    pub id: String,
    pub title: String,
}

/// Whether a run with this status should be announced under this policy. Anything that is
/// not a clean "ok" counts as a failure, so a run that timed out or never started is told
/// about even in the quiet mode.
pub fn announces(policy: &str, status: &str) -> bool {
    policy != "failure" || status != "ok"
}

/// The message for one finished run. Kept to plain text: Telegram's Markdown needs every
/// stray underscore in a shell command escaped, and a notification nobody can read is
/// worse than one that looks plain.
pub fn run_text(job: &Job, run: &JobRun, body: &str) -> String {
    let mut lines = vec![
        format!("Libra · {} · {}", job.name, run.status),
        format!(
            "{} · started {} · finished {}",
            job.kind, run.started_at, run.finished_at
        ),
    ];
    if let Some(code) = run.exit_code {
        lines.push(format!("exit code: {code}"));
    }
    let body = body.trim();
    if !body.is_empty() {
        lines.push(if body.chars().count() > MAX_TEXT {
            let kept: String = body.chars().take(MAX_TEXT).collect();
            let left = body.chars().count() - MAX_TEXT;
            format!("{kept}\n… {left} more characters clipped")
        } else {
            body.to_string()
        });
    }
    lines.join("\n\n")
}

/// A run announced to the chat configured in Settings. Silent when Telegram is not set up:
/// a job that only has a webhook should not log a failure about the missing bot.
pub async fn announce_run(
    http: &Client,
    cfg: &Config,
    job: &Job,
    run: &JobRun,
    body: &str,
) -> Result<(), LibraError> {
    if cfg.telegram_bot_token.trim().is_empty() || cfg.telegram_chat_id.trim().is_empty() {
        return Ok(());
    }
    send(
        http,
        &cfg.telegram_api_url,
        &cfg.telegram_bot_token,
        &cfg.telegram_chat_id,
        &run_text(job, run, body),
    )
    .await
}

/// One message to one chat. Both the notification path and the Settings "send a test"
/// button come through here.
pub async fn send(
    http: &Client,
    base: &str,
    token: &str,
    chat_id: &str,
    text: &str,
) -> Result<(), LibraError> {
    let token = token.trim();
    let chat_id = chat_id.trim();
    if token.is_empty() {
        return Err(LibraError::BadRequest("a bot token is required".into()));
    }
    if chat_id.is_empty() {
        return Err(LibraError::BadRequest(
            "a chat to post to is required".into(),
        ));
    }
    let body = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "disable_web_page_preview": true,
    });
    let url = format!("{}/bot{}/sendMessage", endpoint(base), token);
    let value = post(http, &url, &body, "sendMessage").await?;
    if value["ok"].as_bool() != Some(true) {
        return Err(describe(&value));
    }
    Ok(())
}

/// The chats that have messaged this bot, newest first, deduplicated. The number a person
/// has to paste into Settings is impossible to find by hand, so the bot finds it.
pub async fn chats(http: &Client, base: &str, token: &str) -> Result<Vec<Chat>, LibraError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(LibraError::BadRequest("a bot token is required".into()));
    }
    let url = format!("{}/bot{}/getUpdates?limit=100", endpoint(base), token);
    let value = post(http, &url, &serde_json::json!({}), "getUpdates").await?;
    if value["ok"].as_bool() != Some(true) {
        return Err(describe(&value));
    }
    Ok(parse_chats(&value))
}

/// Pull every chat out of a `getUpdates` answer: from messages, edited messages, channel
/// posts, and callback queries, since a group is discovered by whatever its members send.
fn parse_chats(value: &Value) -> Vec<Chat> {
    let mut found: Vec<Chat> = Vec::new();
    let updates = match value["result"].as_array() {
        Some(updates) => updates,
        None => return found,
    };
    for update in updates.iter().rev() {
        for key in [
            "message",
            "edited_message",
            "channel_post",
            "edited_channel_post",
            "callback_query",
            "my_chat_member",
        ] {
            let Some(chat) = update[key]["chat"].as_object() else {
                continue;
            };
            let id = chat["id"]
                .as_i64()
                .map(|id| id.to_string())
                .unwrap_or_default();
            if id.is_empty() || found.iter().any(|seen| seen.id == id) {
                continue;
            }
            let title = ["title", "username", "first_name"]
                .iter()
                .map(|key| chat.get(*key).and_then(Value::as_str).unwrap_or("").trim())
                .find(|text| !text.is_empty())
                .unwrap_or("")
                .to_string();
            // an unnamed chat still needs something to show in the picker
            let label = if title.is_empty() { id.clone() } else { title };
            found.push(Chat { id, title: label });
        }
    }
    found
}

/// POST and read the JSON answer. Telegram answers 4xx with a JSON body worth showing,
/// so the status text is never the only thing reported.
async fn post(http: &Client, url: &str, body: &Value, what: &str) -> Result<Value, LibraError> {
    let response = http
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| LibraError::Provider {
            provider: "telegram".into(),
            message: format!("{what}: {e}"),
        })?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .map_err(|e| LibraError::Provider {
            provider: "telegram".into(),
            message: format!("{what}: no JSON answer ({status}): {e}"),
        })?;
    if !status.is_success() && value["ok"].as_bool() != Some(false) {
        return Err(LibraError::Provider {
            provider: "telegram".into(),
            message: format!("{what}: {status}"),
        });
    }
    Ok(value)
}

fn describe(value: &Value) -> LibraError {
    LibraError::Provider {
        provider: "telegram".into(),
        message: value["description"]
            .as_str()
            .unwrap_or("rejected")
            .to_string(),
    }
}

/// Trim the trailing slash so `…/bot<token>/…` is built the same way from
/// `https://api.telegram.org` and a self-hosted `http://localhost:8081/`.
fn endpoint(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Job, JobRun};

    fn job() -> Job {
        Job {
            id: "a1".into(),
            name: "Rust release notes".into(),
            kind: "command".into(),
            cron: "0 0 7 * * *".into(),
            timezone: String::new(),
            enabled: true,
            image: "alpine".into(),
            command: "curl -s https://example.com | head".into(),
            prompt: String::new(),
            memory: "256M".into(),
            max_duration_secs: 300,
            notify_url: String::new(),
            created_at: "2026-10-20T00:00:00Z".into(),
            next_run_at: String::new(),
            last_run_at: String::new(),
            last_status: String::new(),
        }
    }

    fn run(status: &str, exit_code: Option<i64>) -> JobRun {
        JobRun {
            id: 3,
            job_id: "a1".into(),
            started_at: "2026-10-20T07:00:00Z".into(),
            finished_at: "2026-10-20T07:00:09Z".into(),
            status: status.into(),
            exit_code,
            output: String::new(),
            detail: String::new(),
            chat_id: String::new(),
        }
    }

    #[test]
    fn every_policy_shows_a_good_run_but_failure_policy_hides_it() {
        assert!(announces("every", "ok"));
        assert!(announces("failure", "error"));
        assert!(!announces("failure", "ok"));
        // An unset policy is the loud one, so a job set up before this existed still tells.
        assert!(announces("", "ok"));
    }

    #[test]
    fn the_message_names_the_job_the_status_and_the_output() {
        let text = run_text(&job(), &run("ok", Some(0)), "hello from the sandbox");
        assert!(text.contains("Rust release notes"));
        assert!(text.contains("ok"));
        assert!(text.contains("exit code: 0"));
        assert!(text.contains("hello from the sandbox"));
    }

    #[test]
    fn a_long_run_output_is_cut_down_to_what_telegram_accepts() {
        let long = "x".repeat(MAX_TEXT + 900);
        let text = run_text(&job(), &run("error", Some(1)), &long);
        assert!(
            text.chars().count() < 4_096,
            "{} characters",
            text.chars().count()
        );
        assert!(text.contains("900 more characters clipped"));
    }

    #[test]
    fn chats_are_read_from_whatever_a_person_sends_and_listed_once_newest_first() {
        let value = serde_json::json!({
            "ok": true,
            "result": [
                // oldest first, as Telegram sends them
                {"edited_message": {"chat": {"id": 9}}},
                {"message": {"chat": {"id": 11, "first_name": "Ada", "username": "ada"}}},
                {"my_chat_member": {"chat": {"id": 11, "title": "Ada Lovelace"}}},
                {"channel_post": {"chat": {"id": -100, "title": "Releases"}}},
                {"callback_query": {"chat": null}},
                {"message": {"text": "no chat here"}},
            ]
        });
        assert_eq!(
            parse_chats(&value),
            vec![
                Chat {
                    id: "-100".into(),
                    title: "Releases".into()
                },
                // the newest update about a chat wins, so a renamed group shows its new name
                Chat {
                    id: "11".into(),
                    title: "Ada Lovelace".into()
                },
                // and a chat with nothing to show is shown by its number
                Chat {
                    id: "9".into(),
                    title: "9".into()
                },
            ]
        );
    }

    #[test]
    fn a_missing_or_blank_token_is_refused_before_the_network() {
        let http = Client::new();
        let blocked = tokio_test_block_on(send(&http, "https://example.invalid", " ", "12", "hi"));
        assert!(matches!(blocked, Err(LibraError::BadRequest(_))));
        let no_chat = tokio_test_block_on(send(
            &http,
            "https://example.invalid",
            "123:abc",
            "  ",
            "hi",
        ));
        assert!(matches!(no_chat, Err(LibraError::BadRequest(_))));
    }

    #[test]
    fn a_server_base_keeps_its_trailing_slash_from_mattering() {
        assert_eq!(
            endpoint("https://api.telegram.org/"),
            "https://api.telegram.org"
        );
        assert_eq!(endpoint(" http://localhost:8081 "), "http://localhost:8081");
    }

    /// This test file has no async test macro of its own, so drive the one future by hand.
    fn tokio_test_block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(future)
    }
}
