use crate::error::ProviderError;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::search::{SearchProvider, parse_html, select, select_in, text_of, urldecode, urlencode};
use reqwest::Client;
use std::pin::Pin;

/// DuckDuckGo has no free API — scrape the HTML endpoint, like the original project does.
pub struct DuckDuckGo {
    http: Client,
}

impl DuckDuckGo {
    pub fn new(http: &Client) -> Self {
        Self { http: http.clone() }
    }
}

impl SearchProvider for DuckDuckGo {
    fn name(&self) -> &'static str {
        "duckduckgo"
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
            let base = if q.kind == SearchKind::News {
                "https://duckduckgo.com/html/?q="
            } else {
                "https://html.duckduckgo.com/html/?q="
            };
            let mut url = format!("{base}{}", urlencode(&q.query));
            if let Some(c) = &q.country {
                url.push_str(&format!("&kl={c}-us"));
            }
            let html = self.http.get(&url).send().await?.text().await?;
            let results = parse_html(&html, |doc| {
                let mut out = Vec::new();
                for node in select(doc, "div.result div.result__body > :not(.result__extras)") {
                    let title = select_in(&node, "a.result__a")
                        .first()
                        .map(|a| {
                            (
                                a.text().collect::<Vec<_>>().join(""),
                                a.value().attr("href").unwrap_or("").to_string(),
                            )
                        })
                        .unwrap_or_default();
                    let snippet = text_of(&node, "a.result__snippet");
                    let mut link = title.1;
                    if let Some(rest) = link.strip_prefix("https://duckduckgo.com/l/?uddg=") {
                        link = urldecode(rest);
                    }
                    if !link.is_empty() {
                        out.push(SearchResult {
                            title: title.0,
                            url: link,
                            snippet,
                            source: "duckduckgo".into(),
                            ..Default::default()
                        });
                    }
                }
                out
            });
            Ok(SearchResponse {
                provider: "duckduckgo".into(),
                results,
                answer: None,
                images: vec![],
            })
        })
    }
}
