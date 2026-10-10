use crate::config::SettingsHandle;
use crate::error::ProviderError;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::search::SearchProvider;
use reqwest::Client;
use serde_json::{json, Value};
use std::pin::Pin;

pub struct Tavily {
    http: Client,
    cfg: SettingsHandle,
}

impl Tavily {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self { http: http.clone(), cfg }
    }
}

impl SearchProvider for Tavily {
    fn name(&self) -> &'static str {
        "tavily"
    }

    fn supports(&self, kind: SearchKind) -> bool {
        matches!(kind, SearchKind::Web | SearchKind::News | SearchKind::Images)
    }

    fn search<'a>(&'a self, q: &'a SearchQuery) -> Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let cfg = self.cfg.get();
            let mut body = json!({
                "query": q.query,
                "max_results": q.num_results,
                "search_depth": "basic",
                "include_answer": false,
                "include_images": q.kind == SearchKind::Images,
            });
            if q.kind == SearchKind::News {
                body["topic"] = json!("news");
                body["days"] = json!(7);
            }
            let resp = self
                .http
                .post("https://api.tavily.com/search")
                .header("Authorization", format!("Bearer {}", cfg.tavily_api_key))
                .json(&body)
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(ProviderError {
                    provider: "tavily".into(),
                    status: resp.status().as_u16(),
                    message: resp.text().await.unwrap_or_default(),
                });
            }
            let v: Value = resp.json().await?;
            let results = v["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|r| SearchResult {
                    title: r["title"].as_str().unwrap_or("").to_string(),
                    url: r["url"].as_str().unwrap_or("").to_string(),
                    snippet: r["content"].as_str().unwrap_or("").to_string(),
                    score: r["score"].as_f64(),
                    published_at: r["published_date"].as_str().map(Into::into),
                    image: r["image"].as_str().filter(|s| !s.is_empty()).map(Into::into),
                    source: "tavily".into(),
                    ..Default::default()
                })
                .filter(|r| !r.url.is_empty())
                .collect();
            let images = v["images"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|i| {
                    let url = i["url"].as_str().or_else(|| i.as_str())?;
                    Some(SearchResult {
                        title: i["description"].as_str().unwrap_or("").to_string(),
                        image: Some(url.to_string()),
                        url: url.to_string(),
                        source: "tavily".into(),
                        ..Default::default()
                    })
                })
                .collect();
            Ok(SearchResponse {
                provider: "tavily".into(),
                results,
                answer: None,
                images,
            })
        })
    }
}
