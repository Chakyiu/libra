pub mod anthropic;
pub mod gemini;
pub mod json;
pub mod openai;
pub mod sse;

use crate::config::{Config, ProviderKind};
use crate::error::LibraError;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Msg {
    pub role: String,
    pub content: String,
    /// Attached images. Not part of the client history JSON; providers encode them per API.
    #[serde(default, skip)]
    pub images: Vec<InlineImage>,
}

#[derive(Clone, Debug)]
pub struct InlineImage {
    pub mime: String,
    pub data: Vec<u8>,
}

pub fn msg(role: &str, content: impl Into<String>) -> Msg {
    Msg {
        role: role.to_string(),
        content: content.into(),
        images: Vec::new(),
    }
}

fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// OpenAI-compatible chat message. Images become `image_url` parts.
pub fn openai_message(m: &Msg) -> serde_json::Value {
    if m.images.is_empty() {
        return serde_json::json!({ "role": m.role, "content": m.content });
    }
    let mut parts = vec![serde_json::json!({ "type": "text", "text": m.content })];
    for img in &m.images {
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{};base64,{}", img.mime, b64(&img.data)) }
        }));
    }
    serde_json::json!({ "role": m.role, "content": parts })
}

/// Anthropic Messages API content. Images are base64 source blocks.
pub fn anthropic_message(m: &Msg) -> serde_json::Value {
    if m.images.is_empty() {
        return serde_json::json!({ "role": m.role, "content": m.content });
    }
    let mut parts = Vec::with_capacity(m.images.len() + 1);
    for img in &m.images {
        parts.push(serde_json::json!({
            "type": "image",
            "source": { "type": "base64", "media_type": img.mime, "data": b64(&img.data) }
        }));
    }
    parts.push(serde_json::json!({ "type": "text", "text": m.content }));
    serde_json::json!({ "role": m.role, "content": parts })
}

/// Gemini `parts`, including inline image data.
pub fn gemini_parts(m: &Msg) -> Vec<serde_json::Value> {
    let mut parts = vec![serde_json::json!({ "text": m.content })];
    for img in &m.images {
        parts.push(serde_json::json!({
            "inline_data": { "mime_type": img.mime, "data": b64(&img.data) }
        }));
    }
    parts
}

#[derive(Clone, Debug, Default)]
pub struct ChatRequest {
    pub messages: Vec<Msg>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub json_mode: bool,
}

#[derive(Clone, Debug)]
pub enum Delta {
    Text(String),
    /// The model's reasoning trace, separate from the answer.
    Thought(String),
    Error(String),
    /// The provider stopped because `max_tokens` was used up.
    Truncated,
}

#[derive(Clone)]
pub struct Llm {
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub http: Client,
}

pub(crate) type StreamTx = UnboundedSender<Delta>;

/// Provider token stream. Dropping it aborts the HTTP read, so stop does not
/// wait for the model to finish the sentence it already started.
pub struct DeltaRx {
    rx: tokio::sync::mpsc::UnboundedReceiver<Delta>,
    abort: tokio::task::AbortHandle,
}

impl DeltaRx {
    pub async fn recv(&mut self) -> Option<Delta> {
        self.rx.recv().await
    }

    pub fn abort_handle(&self) -> tokio::task::AbortHandle {
        self.abort.clone()
    }
}

impl Drop for DeltaRx {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl Llm {
    pub fn new(
        kind: ProviderKind,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        http: &Client,
    ) -> Self {
        Self {
            kind,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
            http: http.clone(),
        }
    }

    pub async fn stream(&self, req: ChatRequest) -> anyhow::Result<DeltaRx> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = match self.kind {
            ProviderKind::OpenAi
            | ProviderKind::Ollama
            | ProviderKind::OpenRouter
            | ProviderKind::Custom
            | ProviderKind::OpenAiCompatible => {
                let ollama = self.kind == ProviderKind::Ollama;
                openai::spawn_stream(
                    &self.http,
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    req,
                    tx,
                    ollama,
                )
            }
            ProviderKind::Anthropic => anthropic::spawn_stream(
                &self.http,
                &self.base_url,
                &self.api_key,
                &self.model,
                req,
                tx,
            ),
            ProviderKind::Gemini => gemini::spawn_stream(
                &self.http,
                &self.base_url,
                &self.api_key,
                &self.model,
                req,
                tx,
            ),
        };
        Ok(DeltaRx {
            rx,
            abort: task.abort_handle(),
        })
    }

    /// One non-streaming completion. Reasoning-only models sometimes leave `content`
    /// empty and put the reply in the thought channel; that text is used when content is blank.
    pub async fn complete(&self, req: ChatRequest) -> anyhow::Result<String> {
        let mut rx = self.stream(req).await?;
        let mut content = String::new();
        let mut thought = String::new();
        while let Some(delta) = rx.recv().await {
            match delta {
                Delta::Text(t) => content.push_str(&t),
                Delta::Thought(t) => thought.push_str(&t),
                Delta::Error(e) => {
                    if content.is_empty() && thought.is_empty() {
                        return Err(anyhow::anyhow!(e));
                    }
                    break;
                }
                Delta::Truncated => break,
            }
        }
        if content.trim().is_empty() {
            Ok(thought)
        } else {
            Ok(content)
        }
    }

    /// Structured call used by the classifier, planner, extractor and suggester.
    pub async fn json_call(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
    ) -> anyhow::Result<serde_json::Value> {
        // A 500-token cap is spent entirely on reasoning by models such as DeepSeek,
        // so content comes back empty. Leave room for the JSON after any thinking.
        let max_tokens = max_tokens.map(|n| n.max(2048));
        let req = ChatRequest {
            messages: vec![msg("user", user)],
            temperature: Some(0.0),
            max_tokens,
            json_mode: true,
        };
        let system_prompt =
            format!("{system}\n\nReturn ONLY raw JSON. No markdown fences, no commentary.");
        let raw = match self.kind {
            ProviderKind::OpenAi
            | ProviderKind::Ollama
            | ProviderKind::OpenRouter
            | ProviderKind::Custom
            | ProviderKind::OpenAiCompatible => {
                openai::call(
                    &self.http,
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    &system_prompt,
                    &req.messages,
                    true,
                    req.max_tokens,
                )
                .await?
            }
            ProviderKind::Anthropic => {
                anthropic::call(
                    &self.http,
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    &system_prompt,
                    &req.messages,
                    true,
                    req.max_tokens,
                )
                .await?
            }
            ProviderKind::Gemini => {
                gemini::call(
                    &self.http,
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    &system_prompt,
                    &req.messages,
                    true,
                    req.max_tokens,
                )
                .await?
            }
        };
        json::extract_json(&raw)
    }

    /// Like `json_call`, but reasoning tokens are handed to `on_thought` as they arrive.
    ///
    /// Reasoning models count thinking against `max_tokens`. A 2048 cap is enough for the
    /// JSON and not for the thought, so the provider stops with `finish_reason=length`
    /// and the structure never closes. Start higher, and retry once if that still happens.
    pub async fn json_call_thinking(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
        mut on_thought: impl FnMut(String),
    ) -> anyhow::Result<serde_json::Value> {
        let budget = max_tokens.unwrap_or(8192).max(8192);
        match self
            .structured_stream(system, user, budget, &mut on_thought)
            .await
        {
            Ok(v) => Ok(v),
            Err(e) if output_truncated(e.as_ref()) && budget < 16_384 => {
                tracing::info!(
                    model = %self.model,
                    budget = 16_384,
                    "structured output hit the token limit; retrying with a larger budget"
                );
                self.structured_stream(system, user, 16_384, &mut on_thought)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    async fn structured_stream(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
        on_thought: &mut impl FnMut(String),
    ) -> anyhow::Result<serde_json::Value> {
        let system_prompt =
            format!("{system}\n\nReturn ONLY raw JSON. No markdown fences, no commentary.");
        let mut rx = self
            .stream(ChatRequest {
                messages: vec![msg("system", system_prompt), msg("user", user)],
                temperature: Some(0.0),
                max_tokens: Some(max_tokens),
                json_mode: true,
            })
            .await?;
        let mut content = String::new();
        let mut thought = String::new();
        let mut truncated = false;
        while let Some(delta) = rx.recv().await {
            match delta {
                Delta::Text(t) => content.push_str(&t),
                Delta::Thought(t) => {
                    thought.push_str(&t);
                    on_thought(t);
                }
                Delta::Truncated => truncated = true,
                Delta::Error(e) => {
                    if output_truncated_msg(&e) {
                        return Err(anyhow::anyhow!(e));
                    }
                    if content.is_empty() && thought.is_empty() {
                        return Err(anyhow::anyhow!(e));
                    }
                    break;
                }
            }
        }
        if let Ok(v) = json::extract_json(&content) {
            return Ok(v);
        }
        if let Ok(v) = json::extract_json(&thought) {
            return Ok(v);
        }
        if truncated {
            anyhow::bail!(
                "structured output was incomplete (finish_reason=length); increase the output budget"
            )
        }
        json::extract_json(&content)
    }

    pub async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        match self.kind {
            ProviderKind::Gemini => {
                gemini::embed(
                    &self.http,
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    texts,
                )
                .await
            }
            ProviderKind::Anthropic => anyhow::bail!("anthropic has no embeddings endpoint"),
            ProviderKind::Ollama => match openai::embed(
                &self.http,
                &self.base_url,
                &self.api_key,
                &self.model,
                texts,
            )
            .await
            {
                Ok(v) => Ok(v),
                // Older Ollama has no /v1/embeddings; fall back to the native API.
                Err(_) => {
                    openai::ollama_embed_native(&self.http, &self.base_url, &self.model, texts)
                        .await
                }
            },
            _ => {
                openai::embed(
                    &self.http,
                    &self.base_url,
                    &self.api_key,
                    &self.model,
                    texts,
                )
                .await
            }
        }
    }
}

/// Pick the model for a request from a `provider/model` reference in the config.
pub fn resolve_chat(cfg: &Config, http: &Client, reference: &str) -> Result<Llm, LibraError> {
    if reference.is_empty() {
        return Err(LibraError::NoModel(
            "add a model provider in Settings, then pick an answer model".into(),
        ));
    }
    let (provider, model) = cfg.resolve_model(reference).ok_or_else(|| {
        LibraError::NoModel(format!(
            "model '{reference}' does not match a configured provider - pick one in Settings"
        ))
    })?;
    let base = if provider.base_url.is_empty() {
        default_base(provider.kind)
    } else {
        provider.base_url.clone()
    };
    Ok(Llm::new(
        provider.kind,
        base,
        &provider.api_key,
        model,
        http,
    ))
}

pub fn resolve_embed(cfg: &Config, http: &Client, reference: &str) -> Result<Llm, LibraError> {
    if reference.is_empty() {
        return Err(LibraError::NoModel(
            "pick an embedding model in Settings → Answering to read files".into(),
        ));
    }
    resolve_chat(cfg, http, reference)
}

fn default_base(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com".into(),
        ProviderKind::Gemini => "https://generativelanguage.googleapis.com".into(),
        ProviderKind::OpenAi => "https://api.openai.com/v1".into(),
        ProviderKind::OpenRouter => "https://openrouter.ai/api/v1".into(),
        _ => "http://127.0.0.1:11434/v1".into(),
    }
}

fn output_truncated(err: &dyn std::error::Error) -> bool {
    output_truncated_msg(&error_detail(err))
}

fn output_truncated_msg(msg: &str) -> bool {
    let s = msg.to_ascii_lowercase();
    s.contains("finish_reason=length")
        || s.contains("finish_reason: length")
        || s.contains("\"finish_reason\":\"length\"")
        || s.contains("output budget")
}

/// Display plus the source chain. Reqwest reports every body failure as
/// "error decoding response body" and hides "operation timed out" or the
/// connection reset underneath.
pub(crate) fn error_detail(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut src = err.source();
    while let Some(e) = src {
        out.push_str(": ");
        out.push_str(&e.to_string());
        src = e.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::output_truncated_msg;

    #[test]
    fn length_stop_is_a_budget_error() {
        assert!(output_truncated_msg(
            "structured output was incomplete (finish_reason=length); increase the output budget"
        ));
        assert!(!output_truncated_msg("no JSON found in model output"));
    }
}

pub(crate) async fn read_error_body(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let body: String = body.chars().take(600).collect();
    format!("upstream {status}: {body}")
}
