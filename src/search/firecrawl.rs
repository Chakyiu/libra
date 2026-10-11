use crate::config::SettingsHandle;
use crate::error::ProviderError;
use crate::search::SearchProvider;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use reqwest::Client;
use serde_json::{Value, json};
use std::pin::Pin;

pub struct Firecrawl {
    http: Client,
    cfg: SettingsHandle,
}

impl Firecrawl {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self {
            http: http.clone(),
            cfg,
        }
    }
}

impl SearchProvider for Firecrawl {
    fn name(&self) -> &'static str {
        "firecrawl"
    }

    fn supports(&self, kind: SearchKind) -> bool {
        matches!(kind, SearchKind::Web | SearchKind::News)
    }

    fn search<'a>(
        &'a self,
        q: &'a SearchQuery,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, ProviderError>> + Send + 'a>>
    {
        Box::pin(async move {
            let cfg = self.cfg.get();
            let source = if q.kind == SearchKind::News {
                "news"
            } else {
                "web"
            };
            let body = json!({
                "query": q.query,
                "limit": q.num_results,
                "sources": [{ "type": source, "parameters": { "limit": q.num_results, "scrapeOptions": { "formats": ["markdown"] } } }],
            });
            let resp = self
                .http
                .post(format!(
                    "{}/v2/search",
                    cfg.firecrawl_url.trim_end_matches('/')
                ))
                .header("Authorization", format!("Bearer {}", cfg.firecrawl_api_key))
                .json(&body)
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(ProviderError {
                    provider: "firecrawl".into(),
                    status: resp.status().as_u16(),
                    message: resp.text().await.unwrap_or_default(),
                });
            }
            let v: Value = resp.json().await?;
            let items = v[source]
                .as_array()
                .cloned()
                .or_else(|| v["data"].as_array().cloned())
                .unwrap_or_default();
            let results = items
                .iter()
                .map(|r| SearchResult {
                    title: r["title"].as_str().unwrap_or("").to_string(),
                    url: r["url"].as_str().unwrap_or("").to_string(),
                    snippet: r["description"].as_str().unwrap_or("").to_string(),
                    content: r["markdown"].as_str().map(Into::into),
                    published_at: r["publishedAt"].as_str().map(Into::into),
                    source: "firecrawl".into(),
                    ..Default::default()
                })
                .filter(|r| !r.url.is_empty())
                .collect();
            Ok(SearchResponse {
                provider: "firecrawl".into(),
                results,
                answer: None,
                images: vec![],
            })
        })
    }
}
