//! SSE event envelope. Compact names match the original project's stream protocol:
//! id / answer chunk / block / close (+ error message on close).

use crate::blocks::Block;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub enum Event {
    Id(String),
    AnswerChunk(String),
    /// Replace the answer text already streamed. A reasoning model sometimes
    /// plans the real reply and then emits a refusal; this swaps in the reply.
    AnswerSet(String),
    /// A piece of the model's reasoning, streamed ahead of the answer.
    Thought(String),
    Block(Block),
    /// Ask the browser for the user's place. `precise` is coordinates; otherwise only the timezone.
    NeedLocation {
        precise: bool,
    },
    Close(Option<String>),
}

impl Event {
    pub fn to_json(&self) -> Value {
        match self {
            Event::Id(id) => json!({ "type": "id", "id": id }),
            Event::AnswerChunk(c) => json!({ "type": "a", "c": c }),
            Event::AnswerSet(c) => json!({ "type": "a", "c": c, "r": true }),
            Event::Thought(c) => json!({ "type": "t", "c": c }),
            Event::Block(b) => json!({ "type": "b", "block": b.to_value() }),
            Event::NeedLocation { precise } => json!({ "type": "loc", "precise": precise }),
            Event::Close(None) => json!({ "type": "e" }),
            Event::Close(Some(err)) => json!({ "type": "c", "message": err }),
        }
    }
}

use crate::cache::Cache;
use crate::config::{Config, SettingsHandle};
use crate::db::Db;
use crate::pipeline::Pipeline;
use crate::search::SearchHub;
use crate::uploads::UploadStore;
use crate::widgets::Registry;
use reqwest::Client;
use std::sync::Arc;

/// Everything a handler needs. Cheap to clone (all Arcs), shared by axum as State.
#[derive(Clone)]
pub struct App {
    pub settings: SettingsHandle,
    pub db: Arc<Db>,
    pub cache: Cache,
    pub http: Client,
    pub search: Arc<SearchHub>,
    pub uploads: Arc<UploadStore>,
    pub pipeline: Arc<Pipeline>,
}

impl App {
    pub fn new(settings: SettingsHandle, db: Arc<Db>, cache: Cache) -> anyhow::Result<Self> {
        let cfg = settings.get();
        let http = http_client(&cfg)?;
        let search = Arc::new(SearchHub::new(&http, settings.clone(), cache.clone()));
        let widgets = Arc::new(Registry::new(&http, settings.clone(), cache.clone()));
        let uploads = Arc::new(UploadStore::new(
            &http,
            settings.clone(),
            db.clone(),
            cache.clone(),
        ));
        let pipeline = Arc::new(Pipeline {
            cfg: settings.clone(),
            db: db.clone(),
            http: http.clone(),
            search: search.clone(),
            widgets: widgets.clone(),
            uploads: uploads.clone(),
            runs: crate::pipeline::Runs::default(),
            locations: crate::pipeline::Locations::default(),
        });
        Ok(Self {
            settings,
            db,
            cache,
            http,
            search,
            uploads,
            pipeline,
        })
    }
}

/// The shared outbound client: every provider, model, embedding and image call uses it,
/// so a LAN provider behind its own CA is configured once here.
fn http_client(cfg: &Config) -> anyhow::Result<Client> {
    // Stall timeout, not a cap on how long an answer may stream. Reqwest's total timeout
    // runs until the body ends, so a 120s deadline was aborting long answers as
    // "error decoding response body".
    let stall = std::time::Duration::from_secs(cfg.request_timeout_secs.max(1));
    let mut builder = Client::builder()
        .user_agent("libra/0.1 (+self-hosted answer engine)")
        .connect_timeout(stall.min(std::time::Duration::from_secs(30)))
        .read_timeout(stall)
        .tcp_nodelay(true)
        .pool_max_idle_per_host(16);
    // A provider on a LAN host with its own CA fails the handshake otherwise: rustls
    // reports UnknownIssuer and the call never reaches the server. Trust the bundle, or —
    // for a self-signed host you own — skip verification when explicitly asked.
    if let Some(path) = &cfg.tls_ca_file {
        let pem = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("LIBRA_TLS_CA_FILE {}: {e}", path.display()))?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| anyhow::anyhow!("LIBRA_TLS_CA_FILE {}: {e}", path.display()))?;
        if certs.is_empty() {
            anyhow::bail!(
                "LIBRA_TLS_CA_FILE {}: no certificate in the file",
                path.display()
            );
        }
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }
    if cfg.tls_skip_verify {
        builder = builder.danger_accept_invalid_certs(true);
    }
    Ok(builder.build()?)
}
