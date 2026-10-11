//! Search-box autocomplete via DuckDuckGo's suggestion endpoint (keyless), like upstream.

use crate::config::Config;
use crate::error::LibraError;
use reqwest::Client;
use serde_json::Value;

pub async fn suggestions(http: &Client, cfg: &Config, query: &str) -> Result<Vec<String>, LibraError> {
    if query.trim().is_empty() {
        return Ok(vec![]);
    }
    let url = format!(
        "https://duckduckgo.com/ac/?q={}&kl={}",
        crate::search::urlencode(query),
        cfg.country
    );
    let v: Value = http
        .get(&url)
        .header("user-agent", "Mozilla/5.0 (X11; Linux x86_64)")
        .send()
        .await
        .map_err(|e| LibraError::Provider {
            provider: "duckduckgo-suggest".into(),
            message: e.to_string(),
        })?
        .error_for_status()
        .map_err(|e| LibraError::Provider {
            provider: "duckduckgo-suggest".into(),
            message: e.to_string(),
        })?
        .json()
        .await
        .map_err(|e| LibraError::Provider {
            provider: "duckduckgo-suggest".into(),
            message: e.to_string(),
        })?;
    Ok(v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s["phrase"].as_str().map(|s| s.to_string()))
        .take(7)
        .collect())
}
