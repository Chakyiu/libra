use crate::config::SettingsHandle;
use crate::error::ProviderError;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::search::SearchProvider;
use reqwest::Client;
use serde_json::{json, Value};
use std::pin::Pin;

pub struct Exa {
    http: Client,
    cfg: SettingsHandle,
}

impl Exa {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self { http: http.clone(), cfg }
    }
}

impl SearchProvider for Exa {
    fn name(&self) -> &'static str {
        "exa"
    }

    fn supports(&self, kind: SearchKind) -> bool {
        matches!(kind, SearchKind::Web | SearchKind::News)
    }

    fn search<'a>(&'a self, q: &'a SearchQuery) -> Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let cfg = self.cfg.get();
            let mut body = json!({
                "query": q.query,
                "numResults": q.num_results,
                "type": "auto",
                "livecrawl": "always",
                "contents": { "text": { "maxCharacters": 1000 }, "highlights": { "numSentences": 3 } },
            });
            if q.kind == SearchKind::News {
                body["category"] = json!("news");
            }
            let resp = self
                .http
                .post("https://api.exa.ai/search")
                .header("x-api-key", &cfg.exa_api_key)
                .json(&body)
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(ProviderError {
                    provider: "exa".into(),
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
                    snippet: r["highlights"]["highlights"]
                        .as_array()
                        .map(|h| h.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" "))
                        .unwrap_or_default(),
                    content: r["text"].as_str().map(Into::into),
                    score: r["score"].as_f64(),
                    published_at: r["publishedDate"].as_str().map(Into::into),
                    author: r["author"].as_str().map(Into::into),
                    source: "exa".into(),
                    ..Default::default()
                })
                .filter(|r| !r.url.is_empty())
                .collect();
            Ok(SearchResponse {
                provider: "exa".into(),
                results,
                answer: None,
                images: vec![],
            })
        })
    }
}
