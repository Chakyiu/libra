use crate::config::SettingsHandle;
use crate::error::ProviderError;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::search::{urlencode, SearchProvider};
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;

/// Self-hosted SearxNG metasearch — the privacy-first default of upstream Vane.
pub struct Searxng {
    http: Client,
    cfg: SettingsHandle,
}

impl Searxng {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self { http: http.clone(), cfg }
    }
}

impl SearchProvider for Searxng {
    fn name(&self) -> &'static str {
        "searxng"
    }

    fn supports(&self, kind: SearchKind) -> bool {
        matches!(kind, SearchKind::Web | SearchKind::News | SearchKind::Images | SearchKind::Videos)
    }

    fn search<'a>(&'a self, q: &'a SearchQuery) -> Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let cfg = self.cfg.get();
            let base = cfg.searxng_url.trim_end_matches('/');
            let category = match q.kind {
                SearchKind::News => "news",
                SearchKind::Images => "images",
                SearchKind::Videos => "videos",
                _ => "general",
            };
            let mut url = format!(
                "{base}/search?format=json&q={}&categories={}&pageno={}",
                urlencode(&q.query),
                category,
                q.page.unwrap_or(1)
            );
            if let Some(c) = q.country.as_deref().filter(|c| *c != "auto") {
                url.push_str(&format!("&language={c}"));
            }
            if let Some(f) = q.freshness.as_deref().filter(|f| *f != "noLimit") {
                url.push_str(&format!("&time_range={f}"));
            }
            if !q.safe_search {
                url.push_str("&safesearch=0");
            }
            let mut req = self.http.get(&url);
            if !cfg.searxng_password.is_empty() {
                req = req.basic_auth(&cfg.searxng_user, Some(&cfg.searxng_password));
            }
            let resp = req.send().await?;
            if !resp.status().is_success() {
                return Err(ProviderError {
                    provider: "searxng".into(),
                    status: resp.status().as_u16(),
                    message: resp.text().await.unwrap_or_default(),
                });
            }
            let body: Value = resp.json().await?;
            let items = body["results"].as_array().cloned().unwrap_or_default();
            let results = items
                .iter()
                .map(|r| SearchResult {
                    title: r["title"].as_str().unwrap_or("").to_string(),
                    url: r["url"].as_str().unwrap_or("").to_string(),
                    snippet: r["content"].as_str().unwrap_or("").to_string(),
                    published_at: r["publishedDate"].as_str().map(Into::into),
                    thumbnail: r["thumbnail"].as_str().map(Into::into),
                    image: r["img_src"].as_str().map(Into::into),
                    width: r["height"].as_i64().map(|_| 0),
                    source: r["engine"].as_str().unwrap_or("searxng").to_string(),
                    ..Default::default()
                })
                .filter(|r| !r.url.is_empty())
                .take(q.num_results)
                .collect();
            Ok(SearchResponse {
                provider: "searxng".into(),
                results,
                answer: body["answers"].as_array().and_then(|a| a.first()).and_then(|a| a["content"].as_str()).map(Into::into),
                images: vec![],
            })
        })
    }
}
