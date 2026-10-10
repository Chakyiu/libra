//! SSE event envelope. Compact names match upstream Vane's stream protocol:
//! id / answer chunk / block / close (+ error message on close).

use crate::blocks::Block;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub enum Event {
    Id(String),
    AnswerChunk(String),
    Block(Block),
    Close(Option<String>),
}

impl Event {
    pub fn to_json(&self) -> Value {
        match self {
            Event::Id(id) => json!({ "type": "id", "id": id }),
            Event::AnswerChunk(c) => json!({ "type": "a", "c": c }),
            Event::Block(b) => json!({ "type": "b", "block": b.to_value() }),
            Event::Close(None) => json!({ "type": "e" }),
            Event::Close(Some(err)) => json!({ "type": "c", "message": err }),
        }
    }
}

use crate::cache::Cache;
use crate::config::SettingsHandle;
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
        let http = Client::builder()
            .user_agent("vane-next/0.1 (+self-hosted answer engine)")
            .timeout(std::time::Duration::from_secs(cfg.request_timeout_secs))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(16)
            .build()?;
        let search = Arc::new(SearchHub::new(&http, settings.clone(), cache.clone()));
        let widgets = Arc::new(Registry::new(&http, settings.clone()));
        let uploads = Arc::new(UploadStore::new(&http, settings.clone(), db.clone(), cache.clone()));
        let pipeline = Arc::new(Pipeline {
            cfg: settings.clone(),
            db: db.clone(),
            http: http.clone(),
            search: search.clone(),
            widgets: widgets.clone(),
            uploads: uploads.clone(),
        });
        Ok(Self { settings, db, cache, http, search, uploads, pipeline })
    }
}
