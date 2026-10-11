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
}

pub fn msg(role: &str, content: impl Into<String>) -> Msg {
    Msg {
        role: role.to_string(),
        content: content.into(),
    }
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
    Error(String),
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

impl Llm {
    pub fn new(kind: ProviderKind, base_url: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>, http: &Client) -> Self {
        Self {
            kind,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
            http: http.clone(),
        }
    }

    pub async fn stream(&self, req: ChatRequest) -> anyhow::Result<tokio::sync::mpsc::UnboundedReceiver<Delta>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        match self.kind {
            ProviderKind::OpenAi | ProviderKind::Ollama | ProviderKind::OpenRouter | ProviderKind::Custom | ProviderKind::OpenAiCompatible => {
                let ollama = self.kind == ProviderKind::Ollama;
                openai::spawn_stream(&self.http, &self.base_url, &self.api_key, &self.model, req, tx, ollama)
            }
            ProviderKind::Anthropic => anthropic::spawn_stream(&self.http, &self.base_url, &self.api_key, &self.model, req, tx),
            ProviderKind::Gemini => gemini::spawn_stream(&self.http, &self.base_url, &self.api_key, &self.model, req, tx),
        }
        Ok(rx)
    }

    /// Structured call used by the classifier, planner, extractor and suggester.
    pub async fn json_call(&self, system: &str, user: &str, max_tokens: Option<u32>) -> anyhow::Result<serde_json::Value> {
        let req = ChatRequest {
            messages: vec![msg("user", user)],
            temperature: Some(0.0),
            max_tokens,
            json_mode: true,
        };
        let system_prompt = format!("{system}\n\nReturn ONLY raw JSON. No markdown fences, no commentary.");
        let raw = match self.kind {
            ProviderKind::OpenAi | ProviderKind::Ollama | ProviderKind::OpenRouter | ProviderKind::Custom | ProviderKind::OpenAiCompatible => openai::call(&self.http, &self.base_url, &self.api_key, &self.model, &system_prompt, &req.messages, true, req.max_tokens).await?,
            ProviderKind::Anthropic => anthropic::call(&self.http, &self.base_url, &self.api_key, &self.model, &system_prompt, &req.messages, true, req.max_tokens).await?,
            ProviderKind::Gemini => gemini::call(&self.http, &self.base_url, &self.api_key, &self.model, &system_prompt, &req.messages, true, req.max_tokens).await?,
        };
        json::extract_json(&raw)
    }

    pub async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        match self.kind {
            ProviderKind::Gemini => gemini::embed(&self.http, &self.base_url, &self.api_key, &self.model, texts).await,
            ProviderKind::Anthropic => anyhow::bail!("anthropic has no embeddings endpoint"),
            ProviderKind::Ollama => match openai::embed(&self.http, &self.base_url, &self.api_key, &self.model, texts).await {
                Ok(v) => Ok(v),
                // Older Ollama has no /v1/embeddings; fall back to the native API.
                Err(_) => openai::ollama_embed_native(&self.http, &self.base_url, &self.model, texts).await,
            },
            _ => openai::embed(&self.http, &self.base_url, &self.api_key, &self.model, texts).await,
        }
    }
}

/// Pick the model for a request from a `provider/model` reference in the config.
pub fn resolve_chat(cfg: &Config, http: &Client, reference: &str) -> Result<Llm, LibraError> {
    if reference.is_empty() {
        return Err(LibraError::NoModel("add a model provider in Settings, then pick an answer model".into()));
    }
    let (provider, model) = cfg.resolve_model(reference).ok_or_else(|| {
        LibraError::NoModel(format!("model '{reference}' does not match a configured provider - pick one in Settings"))
    })?;
    let base = if provider.base_url.is_empty() { default_base(provider.kind) } else { provider.base_url.clone() };
    Ok(Llm::new(provider.kind, base, &provider.api_key, model, http))
}

pub fn resolve_embed(cfg: &Config, http: &Client, reference: &str) -> Result<Llm, LibraError> {
    if reference.is_empty() {
        return Err(LibraError::NoModel("pick an embedding model in Settings → Answering to read files".into()));
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

pub(crate) async fn read_error_body(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let body: String = body.chars().take(600).collect();
    format!("upstream {status}: {body}")
}
