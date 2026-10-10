//! Image generation (optional) — image/video *search* lives in `search`, this is generation.

use crate::config::Config;
use crate::error::VaneError;
use crate::config::ProviderKind;
use reqwest::Client;
use serde_json::{json, Value};

pub async fn generate(http: &Client, cfg: &Config, prompt: &str) -> Result<Value, VaneError> {
    if !cfg.image_generation_enabled {
        return Err(VaneError::BadRequest("image generation is disabled".into()));
    }
    let llm = crate::llm::resolve_chat(cfg, http, &cfg.default_image_model)?;
    if llm.kind != ProviderKind::OpenAi {
        return Err(VaneError::BadRequest(
            "image generation needs an OpenAI-compatible provider with /images/generations".into(),
        ));
    }
    let resp = http
        .post(format!("{}/images/generations", llm.base_url))
        .header("authorization", format!("Bearer {}", llm.api_key))
        .json(&json!({
            "model": cfg.default_image_model,
            "prompt": prompt,
            "n": 1,
            "size": "1024x1024",
            "response_format": "b64_json",
        }))
        .send()
        .await
        .map_err(|e| VaneError::Provider {
            provider: cfg.default_image_model.clone(),
            message: e.to_string(),
        })?;
    if !resp.status().is_success() {
        return Err(VaneError::Provider {
            provider: cfg.default_image_model.clone(),
            message: resp.text().await.unwrap_or_default(),
        });
    }
    let v: Value = resp.json().await.map_err(|e| VaneError::Provider {
        provider: cfg.default_image_model.clone(),
        message: e.to_string(),
    })?;
    let item = v["data"].as_array().and_then(|a| a.first()).cloned().unwrap_or(json!({}));
    Ok(json!({
        "url": item["url"],
        "b64": item["b64_json"],
        "revised_prompt": item["revised_prompt"],
    }))
}
