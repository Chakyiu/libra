use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// A search/provider upstream failed. Kept separate so one dead engine never
/// takes down a request — the hub logs it and tries the next provider.
#[derive(Debug, thiserror::Error)]
#[error("{provider} upstream error ({status}): {message}")]
pub struct ProviderError {
    pub provider: String,
    pub status: u16,
    pub message: String,
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        ProviderError {
            provider: String::new(),
            status: 0,
            message: e.to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LibraError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("no model configured: {0}")]
    NoModel(String),
    #[error("{provider} search/provider error: {message}")]
    Provider { provider: String, message: String },
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl LibraError {
    pub fn any<E: std::fmt::Display>(e: E) -> Self {
        LibraError::Anyhow(anyhow::anyhow!("{e}"))
    }
}

impl IntoResponse for LibraError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            LibraError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            LibraError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            LibraError::NoModel(_) => (StatusCode::PRECONDITION_FAILED, "no_model"),
            LibraError::Provider { .. } => (StatusCode::BAD_GATEWAY, "provider_error"),
            LibraError::Anyhow(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        (
            status,
            axum::Json(json!({ "error": { "code": code, "message": self.to_string() } })),
        )
            .into_response()
    }
}

pub type LibraResult<T> = Result<T, LibraError>;
