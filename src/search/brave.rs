use crate::config::SettingsHandle;
use crate::error::ProviderError;
use crate::search::SearchProvider;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;

pub struct Brave {
    http: Client,
    cfg: SettingsHandle,
}

impl Brave {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self {
            http: http.clone(),
            cfg,
        }
    }

    async fn get(&self, path: &str, q: &SearchQuery) -> Result<Value, ProviderError> {
        let cfg = self.cfg.get();
        let mut url = format!("https://api.search.brave.com/res/v1/{path}");
        url.push_str(&format!(
            "?q={}&count={}",
            crate::search::urlencode(&q.query),
            q.num_results.max(1)
        ));
        if let Some(c) = &q.country {
            url.push_str(&format!("&country={c}&search_lang={c}"));
        }
        if q.safe_search {
            url.push_str("&safesearch=strict");
        }
        if let Some(f) = q.freshness.as_deref().filter(|f| *f != "noLimit") {
            url.push_str(&format!("&freshness={f}"));
        }
        let resp = self
            .http
            .get(&url)
            .header("X-Subscription-Token", &cfg.brave_api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(ProviderError {
                provider: "brave".into(),
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(resp.json().await?)
    }
}

impl SearchProvider for Brave {
    fn name(&self) -> &'static str {
        "brave"
    }

    fn supports(&self, kind: SearchKind) -> bool {
        matches!(
            kind,
            SearchKind::Web | SearchKind::News | SearchKind::Images
        )
    }

    fn search<'a>(
        &'a self,
        q: &'a SearchQuery,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, ProviderError>> + Send + 'a>>
    {
        Box::pin(async move {
            let (path, key) = match q.kind {
                SearchKind::News => ("news/search", "news"),
                SearchKind::Images => ("images/search", "results"),
                _ => ("web/search", "web"),
            };
            let resp = self.get(path, q).await?;
            let items = resp[key]["results"]
                .as_array()
                .cloned()
                .or_else(|| resp["results"].as_array().cloned())
                .unwrap_or_default();
            let results = items
                .iter()
                .map(|r| SearchResult {
                    title: r["title"].as_str().unwrap_or("").to_string(),
                    url: r["url"].as_str().unwrap_or("").to_string(),
                    snippet: r["description"]
                        .as_str()
                        .or_else(|| r["page_age"].as_str())
                        .unwrap_or("")
                        .to_string(),
                    content: None,
                    score: None,
                    published_at: r["age"].as_str().map(Into::into),
                    author: None,
                    thumbnail: r["thumbnail"]["url"].as_str().map(Into::into),
                    image: r["original"]["url"]
                        .as_str()
                        .or_else(|| r["media_url"].as_str())
                        .map(Into::into),
                    width: r["width"].as_i64(),
                    height: r["height"].as_i64(),
                    source: "brave".into(),
                })
                .filter(|r| !r.url.is_empty())
                .collect();
            Ok(SearchResponse {
                provider: "brave".into(),
                results,
                answer: None,
                images: vec![],
            })
        })
    }
}
