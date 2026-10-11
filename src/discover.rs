//! Discover: follow-up questions to explore, generated from recent chats and cached in Redis.

use crate::cache::Cache;
use crate::config::Config;
use crate::db::Db;
use crate::error::LibraError;
use crate::llm;
use crate::prompts;
use serde_json::{json, Value};

const TTL_SECS: u64 = 3600;

pub async fn questions(db: &Db, cache: &Cache, http: &reqwest::Client, cfg: &Config, chat_id: &str) -> Result<Value, LibraError> {
    let key = format!("discover:{chat_id}");
    if let Some(cached) = cache.get_json::<Value>(&key).await {
        return Ok(cached);
    }
    let messages = db.get_messages(chat_id).await?;
    let history = messages
        .iter()
        .rev()
        .take(6)
        .rev()
        .map(|m| format!("{}: {}", m.role, m.content.chars().take(400).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n");
    if history.is_empty() {
        return Ok(json!({ "questions": [] }));
    }
    let llm_client = llm::resolve_chat(cfg, http, cfg.light_model())?;
    let user = prompts::fill(prompts::DISCOVER_QUESTIONS_USER, &[("history", &history)]);
    let v = llm_client
        .json_call(prompts::DISCOVER_QUESTIONS_SYSTEM, &user, None)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("discover generation failed: {e}");
            json!({ "questions": [] })
        });
    cache.set_json(&key, &v, TTL_SECS).await;
    Ok(v)
}
