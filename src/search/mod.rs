use crate::cache::Cache;
use crate::config::{Config, SettingsHandle};
use reqwest::Client;
use std::sync::Arc;

pub mod brave;
pub mod ddg;
pub mod exa;
pub mod firecrawl;
pub mod html_scrape;
pub mod searxng;
pub mod tavily;
pub mod types;

pub use types::{SearchKind, SearchQuery, SearchResponse, SearchResult};

pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, kind: SearchKind) -> bool;
    fn search<'a>(
        &'a self,
        q: &'a SearchQuery,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, crate::error::ProviderError>> + Send + 'a>>;
}

pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub(crate) fn urldecode(s: &str) -> String {
    let mut out = Vec::new();
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hi = it.next().unwrap_or(b'0');
            let lo = it.next().unwrap_or(b'0');
            out.push(u8::from_str_radix(&format!("{}{}", hi as char, lo as char), 16).unwrap_or(b'%'));
        } else if b == b'+' {
            out.push(b' ');
        } else {
            out.push(b);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn select<'a>(doc: &'a scraper::Html, css: &str) -> Vec<scraper::ElementRef<'a>> {
    scraper::Selector::parse(css)
        .map(|s| doc.select(&s).collect())
        .unwrap_or_default()
}

pub(crate) fn parse_html<F: FnOnce(&scraper::Html) -> Vec<SearchResult>>(html: &str, f: F) -> Vec<SearchResult> {
    f(&scraper::Html::parse_document(html))
}

pub(crate) fn select_in<'a>(node: &scraper::ElementRef<'a>, css: &str) -> Vec<scraper::ElementRef<'a>> {
    scraper::Selector::parse(css)
        .map(|s| node.select(&s).collect())
        .unwrap_or_default()
}

pub(crate) fn text_of(node: &scraper::ElementRef, css: &str) -> String {
    select_in(node, css)
        .first()
        .map(|n| n.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}


/// Search hub: tries the configured provider order per search kind, caches results in Redis.
pub struct SearchHub {
    cfg: SettingsHandle,
    cache: Cache,
    providers: Vec<Arc<dyn SearchProvider>>,
}

impl SearchHub {
    pub fn new(http: &Client, cfg: SettingsHandle, cache: Cache) -> Self {
        let providers: Vec<Arc<dyn SearchProvider>> = vec![
            Arc::new(brave::Brave::new(http, cfg.clone())),
            Arc::new(ddg::DuckDuckGo::new(http)),
            Arc::new(searxng::Searxng::new(http, cfg.clone())),
            Arc::new(html_scrape::HtmlScrape::new(http, cfg.clone())),
            Arc::new(exa::Exa::new(http, cfg.clone())),
            Arc::new(tavily::Tavily::new(http, cfg.clone())),
            Arc::new(firecrawl::Firecrawl::new(http, cfg.clone())),
        ];
        Self { cfg, cache, providers }
    }

    pub fn provider_names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    /// Redis-backed so repeated / hot queries never touch the upstream engines.
    pub async fn run(&self, kind: SearchKind, q: &mut SearchQuery) -> SearchResponse {
        let cfg = self.cfg.get();
        q.kind = kind;
        let key = self.cache_key(&cfg, q);
        if let Some(hit) = self.cache.get_json::<SearchResponse>(&key).await {
            return hit;
        }
        let response = self.run_uncached(kind, q, &cfg).await;
        if !response.results.is_empty() && cfg.search_cache_ttl_secs > 0 {
            self.cache.set_json(&key, &response, cfg.search_cache_ttl_secs).await;
        }
        response
    }

    /// Providers are tried in configured order; the first non-empty answer wins.
    async fn run_uncached(&self, kind: SearchKind, q: &SearchQuery, cfg: &Config) -> SearchResponse {
        for name in cfg.search_order_for(kind) {
            let Some(provider) = self.providers.iter().find(|p| p.name() == name) else {
                tracing::warn!(provider = name, "unknown search provider in order list");
                continue;
            };
            if !provider.supports(kind) {
                continue;
            }
            if !cfg.provider_enabled(&name) {
                continue;
            }
            match provider.search(q).await {
                Ok(resp) if !resp.results.is_empty() => return resp,
                Ok(_) => tracing::debug!(provider = name, "empty result set, falling through"),
                Err(e) => tracing::warn!(provider = name, status = e.status, error = %e.message, "search provider failed"),
            }
        }
        SearchResponse {
            provider: "none".into(),
            ..Default::default()
        }
    }

    fn cache_key(&self, cfg: &Config, q: &SearchQuery) -> String {
        let raw = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            q.kind.as_str(),
            q.query,
            q.num_results,
            q.country.as_deref().unwrap_or(""),
            q.language.as_deref().unwrap_or(""),
            q.freshness.as_deref().unwrap_or(""),
            q.site.as_deref().unwrap_or(""),
            q.page.unwrap_or(1),
            cfg.enabled_providers.join(","),
        );
        format!("search:{}:{}", q.kind.as_str(), crate::cache::hex(&raw))
    }
}
