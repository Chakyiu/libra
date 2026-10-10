use crate::config::SettingsHandle;
use crate::error::ProviderError;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::search::{parse_html, select, select_in, text_of, urlencode, SearchProvider};
use reqwest::Client;
use std::pin::Pin;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Keyless fallback: scrape Bing result pages. Works out of the box, breaks when Bing changes markup.
pub struct HtmlScrape {
    http: Client,
    cfg: SettingsHandle,
}

impl HtmlScrape {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self { http: http.clone(), cfg }
    }
}

/// Bing wraps every result in a `/ck/a?...&u=a1<base64>` redirect; citations must show the real URL.
fn clean_url(raw: &str) -> String {
    use base64::Engine;
    if let Some(rest) = raw.split("&u=a1").nth(1) {
        let token: String = rest.chars().take_while(|c| *c != '&' && *c != '#').collect();
        if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.trim_start_matches('=')) {
            if let Ok(url) = String::from_utf8(bytes) {
                if url.starts_with("http") {
                    return url;
                }
            }
        }
    }
    raw.to_string()
}

impl SearchProvider for HtmlScrape {
    fn name(&self) -> &'static str {
        "html-scrape"
    }

    fn supports(&self, kind: SearchKind) -> bool {
        matches!(kind, SearchKind::Web | SearchKind::News | SearchKind::Images)
    }

    fn search<'a>(&'a self, q: &'a SearchQuery) -> Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            let cfg = self.cfg.get();
            let url = match q.kind {
                SearchKind::News => format!(
                    "https://www.bing.com/news/search?q={}&count={}",
                    urlencode(&q.query),
                    q.num_results
                ),
                SearchKind::Images => format!(
                    "https://www.bing.com/images/search?q={}&form=HDRSC2&first=1&count={}",
                    urlencode(&q.query),
                    q.num_results
                ),
                _ => {
                    let market = q.country.clone().unwrap_or_else(|| cfg.country.clone());
                    format!(
                        "https://www.bing.com/search?q={}&count={}&mkt={}",
                        urlencode(&q.query),
                        q.num_results,
                        market
                    )
                }
            };
            let html = self
                .http
                .get(&url)
                .header("user-agent", UA)
                .header("accept-language", "en-US,en;q=0.9")
                .send()
                .await?
                .text()
                .await?;
            let kind = q.kind;
            let results = parse_html(&html, |doc| match kind {
                SearchKind::Images => select(doc, "a.iusc")
                    .iter()
                    .filter_map(|a| {
                        let meta: serde_json::Value = serde_json::from_str(a.value().attr("m")?).ok()?;
                        Some(SearchResult {
                            title: meta["t"].as_str().unwrap_or("").to_string(),
                            url: meta["purl"].as_str().unwrap_or("").to_string(),
                            image: meta["murl"].as_str().map(Into::into),
                            thumbnail: meta["turl"].as_str().map(Into::into),
                            source: "bing".into(),
                            ..Default::default()
                        })
                    })
                    .filter(|r| r.image.is_some())
                    .collect(),
                SearchKind::News => select(doc, "div.news-card, div.t_s")
                    .iter()
                    .map(|n| SearchResult {
                        title: text_of(n, "a.title, h2 a"),
                        url: clean_url(
                            select_in(n, "a.title, h2 a")
                                .first()
                                .and_then(|a| a.value().attr("href"))
                                .unwrap_or_default(),
                        ),
                        snippet: text_of(n, "div.snippet, .news_dt ~ *"),
                        published_at: Some(text_of(n, "span.news_dt")).filter(|s| !s.is_empty()),
                        source: "bing".into(),
                        ..Default::default()
                    })
                    .filter(|r| !r.url.is_empty())
                    .collect(),
                _ => select(doc, "li.b_algo")
                    .iter()
                    .map(|n| SearchResult {
                        title: text_of(n, "h2 a"),
                        url: clean_url(
                            select_in(n, "h2 a")
                                .first()
                                .and_then(|a| a.value().attr("href"))
                                .unwrap_or_default(),
                        ),
                        snippet: text_of(n, "div.b_caption p, p"),
                        source: "bing".into(),
                        ..Default::default()
                    })
                    .filter(|r| !r.url.is_empty())
                    .collect(),
            });
            Ok(SearchResponse {
                provider: "html-scrape".into(),
                results,
                answer: None,
                images: vec![],
            })
        })
    }
}
