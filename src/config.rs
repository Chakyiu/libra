use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Ollama,
    OpenAi,
    OpenRouter,
    Custom,
    Anthropic,
    Gemini,
    OpenAiCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub embedding_models: Vec<String>,
}

/// Runtime settings: env/file defaults, live-overridable through the settings API.
#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub redis_url: String,
    pub redis_enabled: bool,
    pub static_dir: PathBuf,

    pub providers: Vec<Provider>,
    pub default_model: String,
    pub default_light_model: String,
    pub default_rerank_model: String,
    pub default_embedding_model: String,
    pub default_image_model: String,
    pub default_temperature: f32,

    pub search_order_web: Vec<String>,
    pub search_order_news: Vec<String>,
    pub search_order_images: Vec<String>,
    pub search_order_videos: Vec<String>,

    pub searxng_url: String,
    pub searxng_user: String,
    pub searxng_password: String,
    pub brave_api_key: String,
    pub exa_api_key: String,
    pub tavily_api_key: String,
    pub firecrawl_api_key: String,
    pub firecrawl_url: String,

    /// Read the pages behind a question's links and the sources an answer cites.
    pub read_pages: bool,
    /// `obscura` on `PATH`, or a path to the binary.
    pub obscura_bin: String,
    /// Browser-like TLS fingerprint. Needs an Obscura build with the stealth feature.
    pub obscura_stealth: bool,
    /// How many cited sources Balance reads, besides links in the question.
    pub fetch_page_limit: usize,
    /// Seconds Obscura may spend on one page.
    pub fetch_timeout_secs: usize,

    pub num_web_results: usize,
    pub rerank_limit: usize,
    pub document_chunk_limit: usize,
    pub country: String,
    pub language: String,
    pub freshness: String,
    pub safe_search: bool,
    pub enable_image_search: bool,
    pub enable_video_search: bool,
    pub enable_fresh_search: bool,
    pub enable_reranker: bool,
    pub image_generation_enabled: bool,
    pub custom_instructions: String,
    pub disabled_widgets: Vec<String>,
    pub enabled_providers: Vec<String>,
    pub request_timeout_secs: u64,
    pub search_cache_ttl_secs: u64,
    pub max_upload_mb: usize,

    /// Bearer token every `/api` request must carry. Empty means the API is open — only
    /// sensible on a trusted network or behind a proxy that authenticates. Env-only:
    /// it is never read from settings.json and never returned by GET /api/settings.
    pub auth_token: String,
    /// Origins allowed to call the API from a browser. Empty = same-origin only,
    /// which is what the single-binary deployment needs. `["*"]` is the dev escape hatch.
    pub allowed_origins: Vec<String>,
}

/// Hot-swappable handle to the active config: a settings change swaps the Arc, and each
/// request keeps the snapshot it started with. No subsystem rebuild, no restart.
#[derive(Clone)]
pub struct SettingsHandle(Arc<std::sync::RwLock<Arc<Config>>>);

impl SettingsHandle {
    pub fn new(cfg: Config) -> Self {
        Self(Arc::new(std::sync::RwLock::new(Arc::new(cfg))))
    }
    pub fn get(&self) -> Arc<Config> {
        self.0.read().unwrap().clone()
    }
    pub fn set(&self, cfg: Config) {
        *self.0.write().unwrap() = Arc::new(cfg);
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let data_dir = std::env::var("LIBRA_DATA_DIR")
            .or_else(|_| std::env::var("DATA_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| Self::default_data_dir());
        std::fs::create_dir_all(&data_dir).ok();

        let mut cfg = Self::defaults(&data_dir);
        if let Some(from_file) = Self::read_settings_file(&data_dir)? {
            cfg.apply_settings(&from_file);
        }
        cfg.apply_env();
        Ok(cfg)
    }

    fn default_data_dir() -> PathBuf {
        let dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".libra"));
        dir.join("libra")
    }

    fn defaults(data_dir: &PathBuf) -> Self {
        Self {
            port: std::env::var("PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3001),
            data_dir: data_dir.clone(),
            db_path: data_dir.join("libra.db"),
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".into()),
            redis_enabled: std::env::var("REDIS_DISABLED")
                .map(|v| v != "1")
                .unwrap_or(true),
            static_dir: PathBuf::from(
                std::env::var("LIBRA_STATIC_DIR").unwrap_or_else(|_| "web/out".into()),
            ),
            // Model providers are whatever the user adds in Settings; a fresh instance has none.
            providers: vec![],
            default_model: String::new(),
            default_light_model: String::new(),
            default_rerank_model: String::new(),
            default_embedding_model: String::new(),
            default_image_model: String::new(),
            default_temperature: 0.3,
            search_order_web: vec![
                "searxng".into(),
                "brave".into(),
                "duckduckgo".into(),
                "html-scrape".into(),
            ],
            search_order_news: vec!["searxng".into(), "brave".into(), "duckduckgo".into()],
            search_order_images: vec!["searxng".into(), "brave".into(), "html-scrape".into()],
            search_order_videos: vec!["searxng".into()],
            searxng_url: std::env::var("SEARXNG_API_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
            searxng_user: std::env::var("SEARXNG_USER").unwrap_or_else(|_| "libra".into()),
            searxng_password: std::env::var("SEARXNG_PASSWORD").unwrap_or_default(),
            brave_api_key: String::new(),
            exa_api_key: String::new(),
            tavily_api_key: String::new(),
            firecrawl_api_key: String::new(),
            firecrawl_url: "https://api.firecrawl.dev".into(),
            read_pages: true,
            obscura_bin: std::env::var("OBSCURA_BIN").unwrap_or_else(|_| "obscura".into()),
            obscura_stealth: std::env::var("OBSCURA_STEALTH")
                .map(|v| !matches!(v.as_str(), "0" | "false" | "off"))
                .unwrap_or(true),
            fetch_page_limit: std::env::var("FETCH_PAGE_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            fetch_timeout_secs: std::env::var("FETCH_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            num_web_results: 20,
            rerank_limit: 7,
            document_chunk_limit: 12,
            country: "auto".into(),
            language: "auto".into(),
            freshness: "noLimit".into(),
            safe_search: true,
            enable_image_search: true,
            enable_video_search: true,
            enable_fresh_search: true,
            enable_reranker: true,
            image_generation_enabled: false,
            custom_instructions: String::new(),
            disabled_widgets: vec![],
            enabled_providers: vec![],
            host: std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            request_timeout_secs: std::env::var("REQUEST_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(120),
            search_cache_ttl_secs: std::env::var("SEARCH_CACHE_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            max_upload_mb: std::env::var("MAX_UPLOAD_MB")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(25),
            auth_token: std::env::var("LIBRA_AUTH_TOKEN")
                .unwrap_or_default()
                .trim()
                .to_string(),
            allowed_origins: split_list(
                &std::env::var("LIBRA_ALLOWED_ORIGINS").unwrap_or_default(),
            ),
        }
    }

    fn settings_file(data_dir: &PathBuf) -> PathBuf {
        data_dir.join("settings.json")
    }

    pub fn read_settings_file(data_dir: &PathBuf) -> anyhow::Result<Option<serde_json::Value>> {
        let path = Self::settings_file(data_dir);
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        Ok(Some(serde_json::from_str(&raw)?))
    }

    pub fn save_settings_file(&self) -> anyhow::Result<()> {
        let value = self.settings_json();
        std::fs::write(
            Self::settings_file(&self.data_dir),
            serde_json::to_string_pretty(&value)?,
        )?;
        Ok(())
    }

    /// The mutable part of the config, as exposed by GET /api/settings.
    pub fn settings_json(&self) -> serde_json::Value {
        serde_json::json!({
            "providers": self.providers,
            "defaultModel": self.default_model,
            "defaultLightModel": self.default_light_model,
            "defaultRerankModel": self.default_rerank_model,
            "defaultEmbeddingModel": self.default_embedding_model,
            "defaultImageModel": self.default_image_model,
            "defaultTemperature": self.default_temperature,
            "searchOrder": {
                "web": self.search_order_web,
                "news": self.search_order_news,
                "images": self.search_order_images,
                "videos": self.search_order_videos,
            },
            "searxngUrl": self.searxng_url,
            "searxngUser": self.searxng_user,
            "searxngPassword": self.searxng_password,
            "braveApiKey": self.brave_api_key,
            "exaApiKey": self.exa_api_key,
            "tavilyApiKey": self.tavily_api_key,
            "firecrawlApiKey": self.firecrawl_api_key,
            "firecrawlUrl": self.firecrawl_url,
            "readPages": self.read_pages,
            "obscuraBin": self.obscura_bin,
            "obscuraStealth": self.obscura_stealth,
            "fetchPageLimit": self.fetch_page_limit,
            "fetchTimeoutSecs": self.fetch_timeout_secs,
            "numWebResults": self.num_web_results,
            "rerankLimit": self.rerank_limit,
            "documentChunkLimit": self.document_chunk_limit,
            "country": self.country,
            "language": self.language,
            "freshness": self.freshness,
            "safeSearch": self.safe_search,
            "enableImageSearch": self.enable_image_search,
            "enableVideoSearch": self.enable_video_search,
            "enableFreshSearch": self.enable_fresh_search,
            "enableReranker": self.enable_reranker,
            "imageGenerationEnabled": self.image_generation_enabled,
            "customInstructions": self.custom_instructions,
            "disabledWidgets": self.disabled_widgets,
            "enabledProviders": self.enabled_providers,
        })
    }

    pub fn apply_settings(&mut self, v: &serde_json::Value) {
        macro_rules! s {
            ($k:expr, $f:expr) => {
                if let Some(x) = v[$k].as_str() {
                    $f = x.to_string()
                }
            };
        }
        macro_rules! b {
            ($k:expr, $f:expr) => {
                if let Some(x) = v[$k].as_bool() {
                    $f = x
                }
            };
        }
        macro_rules! n {
            ($k:expr, $f:expr) => {
                if let Some(x) = v[$k].as_u64() {
                    $f = x as usize
                }
            };
        }
        macro_rules! list {
            ($k:expr, $f:expr) => {
                if let Some(arr) = v[$k].as_array() {
                    $f = arr
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                }
            };
        }

        macro_rules! f32v {
            ($k:expr, $f:expr) => {
                if let Some(x) = v[$k].as_f64() {
                    $f = x as f32
                }
            };
        }

        s!("defaultModel", self.default_model);
        s!("defaultLightModel", self.default_light_model);
        s!("defaultRerankModel", self.default_rerank_model);
        s!("defaultEmbeddingModel", self.default_embedding_model);
        s!("defaultImageModel", self.default_image_model);
        f32v!("defaultTemperature", self.default_temperature);
        s!("searxngUrl", self.searxng_url);
        s!("searxngUser", self.searxng_user);
        s!("searxngPassword", self.searxng_password);
        s!("braveApiKey", self.brave_api_key);
        s!("exaApiKey", self.exa_api_key);
        s!("tavilyApiKey", self.tavily_api_key);
        s!("firecrawlApiKey", self.firecrawl_api_key);
        s!("firecrawlUrl", self.firecrawl_url);
        b!("readPages", self.read_pages);
        s!("obscuraBin", self.obscura_bin);
        b!("obscuraStealth", self.obscura_stealth);
        n!("fetchPageLimit", self.fetch_page_limit);
        n!("fetchTimeoutSecs", self.fetch_timeout_secs);
        n!("numWebResults", self.num_web_results);
        n!("rerankLimit", self.rerank_limit);
        n!("documentChunkLimit", self.document_chunk_limit);
        s!("country", self.country);
        s!("language", self.language);
        s!("freshness", self.freshness);
        b!("safeSearch", self.safe_search);
        b!("enableImageSearch", self.enable_image_search);
        b!("enableVideoSearch", self.enable_video_search);
        b!("enableFreshSearch", self.enable_fresh_search);
        b!("enableReranker", self.enable_reranker);
        b!("imageGenerationEnabled", self.image_generation_enabled);
        s!("customInstructions", self.custom_instructions);
        list!("disabledWidgets", self.disabled_widgets);
        list!("enabledProviders", self.enabled_providers);

        if let Some(arr) = v["providers"].as_array() {
            if let Ok(parsed) =
                serde_json::from_value::<Vec<Provider>>(serde_json::Value::Array(arr.clone()))
            {
                self.providers = parsed;
            }
        }
        if let Some(order) = v["searchOrder"].as_object() {
            let pick = |k: &str, dest: &mut Vec<String>| {
                if let Some(arr) = order.get(k).and_then(|o| o.as_array()) {
                    *dest = arr
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect();
                }
            };
            pick("web", &mut self.search_order_web);
            pick("news", &mut self.search_order_news);
            pick("images", &mut self.search_order_images);
            pick("videos", &mut self.search_order_videos);
        }
    }

    /// Search and server settings only. Model providers and the models picked from them live in
    /// settings.json, written from the Settings page.
    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("BRAVE_API_KEY") {
            self.brave_api_key = v;
        }
        if let Ok(v) = std::env::var("EXA_API_KEY") {
            self.exa_api_key = v;
        }
        if let Ok(v) = std::env::var("TAVILY_API_KEY") {
            self.tavily_api_key = v;
        }
        if let Ok(v) = std::env::var("FIRECRAWL_API_KEY") {
            self.firecrawl_api_key = v;
        }
        if std::env::var("OBSCURA_DISABLED").ok().as_deref() == Some("1") {
            self.read_pages = false;
        }
        if let Ok(v) = std::env::var("OBSCURA_BIN") {
            if !v.trim().is_empty() {
                self.obscura_bin = v;
            }
        }
        if let Ok(v) = std::env::var("OBSCURA_STEALTH") {
            self.obscura_stealth = !matches!(v.as_str(), "0" | "false" | "off");
        }
        if let Ok(v) = std::env::var("FETCH_PAGE_LIMIT") {
            if let Ok(n) = v.parse() {
                self.fetch_page_limit = n;
            }
        }
        if let Ok(v) = std::env::var("FETCH_TIMEOUT_SECS") {
            if let Ok(n) = v.parse() {
                self.fetch_timeout_secs = n;
            }
        }
        if let Ok(v) = std::env::var("WEB_SEARCH_NUM_RESULTS") {
            if let Ok(n) = v.parse() {
                self.num_web_results = n;
            }
        }
        if let Ok(v) = std::env::var("WEB_SEARCH_ORDER") {
            self.search_order_web = split_list(&v);
        }
        if let Ok(v) = std::env::var("NEWS_SEARCH_ORDER") {
            self.search_order_news = split_list(&v);
        }
        if let Ok(v) = std::env::var("IMAGE_SEARCH_ORDER") {
            self.search_order_images = split_list(&v);
        }
        if let Ok(v) = std::env::var("VIDEO_SEARCH_ORDER") {
            self.search_order_videos = split_list(&v);
        }
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Model references look like `openrouter/anthropic/claude-3.5-sonnet`:
    /// the first segment is the provider id, the rest is the model name (which may itself contain '/').
    pub fn resolve_model<'a>(&'a self, reference: &'a str) -> Option<(&'a Provider, &'a str)> {
        if reference.is_empty() {
            return None;
        }
        for sep in reference.match_indices('/') {
            let (candidate, model) = (&reference[..sep.0], &reference[sep.0 + 1..]);
            if let Some(p) = self.provider(candidate) {
                return Some((p, model));
            }
        }
        None
    }

    /// The model for background chores (titles, suggestions); unset means "same as the answer model".
    pub fn light_model(&self) -> &str {
        if self.default_light_model.is_empty() {
            &self.default_model
        } else {
            &self.default_light_model
        }
    }

    pub fn model_options(&self) -> serde_json::Value {
        serde_json::json!({
            "chat": self.providers.iter().flat_map(|p| p.models.iter().map(move |m| format!("{}/{}", p.id, m))).collect::<Vec<_>>(),
            "embedding": self.providers.iter().flat_map(|p| p.embedding_models.iter().map(move |m| format!("{}/{}", p.id, m))).collect::<Vec<_>>(),
        })
    }

    pub fn search_order_for(&self, kind: crate::search::types::SearchKind) -> Vec<String> {
        match kind {
            crate::search::types::SearchKind::Web => self.search_order_web.clone(),
            crate::search::types::SearchKind::News => self.search_order_news.clone(),
            crate::search::types::SearchKind::Images => self.search_order_images.clone(),
            crate::search::types::SearchKind::Videos => self.search_order_videos.clone(),
        }
    }

    pub fn provider_enabled(&self, name: &str) -> bool {
        if self.enabled_providers.is_empty() {
            return true;
        }
        self.enabled_providers.iter().any(|p| p == name)
    }

    pub fn now(&self) -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// Local date the models can use. Weekday, calendar date, clock, and offset.
    pub fn today_line() -> String {
        chrono::Local::now()
            .format("%A, %d %B %Y, %H:%M UTC%:z")
            .to_string()
    }

    pub fn today(&self) -> String {
        Self::today_line()
    }

    pub fn year(&self) -> String {
        chrono::Local::now().format("%Y").to_string()
    }

    pub fn units_imperial(&self) -> bool {
        matches!(self.country.as_str(), "us" | "usa" | "lr" | "mm")
    }
}

fn split_list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_line_is_the_local_date() {
        let line = Config::today_line();
        let year = chrono::Local::now().format("%Y").to_string();
        assert!(line.contains(&year), "{line}");
        assert!(line.contains("UTC"), "{line}");
        assert!(line.contains(','), "{line}");
    }

    #[test]
    fn fresh_config_has_no_model_providers() {
        let cfg = Config::defaults(&PathBuf::from("/tmp/libra-test"));
        assert!(cfg.providers.is_empty());
        assert!(cfg.default_model.is_empty());
        assert!(cfg.resolve_model("ollama/llama3.1:8b").is_none());
    }

    #[test]
    fn resolves_slashed_openrouter_models() {
        let mut cfg = Config::defaults(&PathBuf::from("/tmp/libra-test"));
        cfg.apply_settings(&serde_json::json!({ "providers": [{
            "id": "openrouter", "name": "OpenRouter", "type": "open_router",
            "base_url": "https://openrouter.ai/api/v1", "api_key": "",
            "models": ["anthropic/claude-3.5-sonnet"]
        }] }));
        let (p, m) = cfg
            .resolve_model("openrouter/anthropic/claude-3.5-sonnet")
            .unwrap();
        assert_eq!(p.id, "openrouter");
        assert_eq!(m, "anthropic/claude-3.5-sonnet");
        assert!(cfg.resolve_model("nope/model").is_none());
    }

    #[test]
    fn settings_round_trip() {
        let mut cfg = Config::defaults(&PathBuf::from("/tmp/libra-test"));
        let patch = serde_json::json!({ "braveApiKey": "k", "numWebResults": 12, "searchOrder": { "web": ["brave", "searxng"] } });
        cfg.apply_settings(&patch);
        assert_eq!(cfg.brave_api_key, "k");
        assert_eq!(cfg.num_web_results, 12);
        assert_eq!(
            cfg.search_order_web,
            vec!["brave".to_string(), "searxng".to_string()]
        );
        cfg.apply_settings(&serde_json::json!({
            "readPages": false,
            "obscuraBin": "/usr/local/bin/obscura",
            "obscuraStealth": false,
            "fetchPageLimit": 8,
            "fetchTimeoutSecs": 15
        }));
        assert!(!cfg.read_pages);
        assert_eq!(cfg.obscura_bin, "/usr/local/bin/obscura");
        assert!(!cfg.obscura_stealth);
        assert_eq!(cfg.fetch_page_limit, 8);
        assert_eq!(cfg.fetch_timeout_secs, 15);
        let json = cfg.settings_json();
        assert_eq!(json["readPages"], false);
        assert_eq!(json["fetchPageLimit"], 8);
    }

    #[test]
    fn the_access_token_stays_out_of_the_settings_api() {
        let mut cfg = Config::defaults(&PathBuf::from("/tmp/libra-test"));
        cfg.auth_token = "s3cret".into();
        cfg.apply_settings(&serde_json::json!({
            "authToken": "x",
            "auth_token": "y",
            "allowedOrigins": ["*"]
        }));
        let json = cfg.settings_json();
        assert_eq!(cfg.auth_token, "s3cret");
        assert!(json.get("authToken").is_none(), "{json}");
        assert!(json.get("auth_token").is_none(), "{json}");
    }
}
