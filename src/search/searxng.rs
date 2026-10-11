use crate::config::SettingsHandle;
use crate::error::ProviderError;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::search::{SearchProvider, urldecode, urlencode};
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;

/// Self-hosted SearxNG metasearch — the privacy-first default of the original project.
pub struct Searxng {
    http: Client,
    cfg: SettingsHandle,
}

impl Searxng {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self {
            http: http.clone(),
            cfg,
        }
    }
}

impl SearchProvider for Searxng {
    fn name(&self) -> &'static str {
        "searxng"
    }

    fn supports(&self, kind: SearchKind) -> bool {
        matches!(
            kind,
            SearchKind::Web | SearchKind::News | SearchKind::Images | SearchKind::Videos
        )
    }

    fn search<'a>(
        &'a self,
        q: &'a SearchQuery,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<SearchResponse, ProviderError>> + Send + 'a>>
    {
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
            // SearxNG `language` is a language tag (`en`, `zh-HK`), not a country code.
            // Sending `language=us` makes the image engines return nothing.
            if let Some(lang) = q
                .language
                .as_deref()
                .filter(|c| !c.is_empty() && *c != "auto")
            {
                url.push_str(&format!("&language={}", urlencode(lang)));
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
            let results = parse_results(&body, q.num_results);
            Ok(SearchResponse {
                provider: "searxng".into(),
                results,
                answer: body["answers"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|a| a["content"].as_str())
                    .map(Into::into),
                images: vec![],
            })
        })
    }
}

/// Image hits often have a null page `url`, a preview in `thumbnail_src` (not
/// `thumbnail`), and — when the image proxy is on — an `img_src` of
/// `/image_proxy?url=<original>`. The browser cannot load that path on the
/// Libra origin, or on the Docker hostname `searxng`.
fn parse_results(body: &Value, limit: usize) -> Vec<SearchResult> {
    let Some(items) = body["results"].as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(parse_result).take(limit).collect()
}

fn parse_result(r: &Value) -> Option<SearchResult> {
    let mut image = media_url(r, &["img_src", "thumbnail_src", "thumbnail"]);
    let thumbnail = media_url(r, &["thumbnail_src", "thumbnail", "img_src"]);
    if image.is_none() {
        image = thumbnail.clone();
    }
    let mut url = media_url(r, &["url"]).unwrap_or_default();
    if url.is_empty() {
        url = image.clone().unwrap_or_default();
    }
    if url.is_empty() {
        return None;
    }
    let (mut width, mut height) = resolution(r["resolution"].as_str().unwrap_or(""));
    if width.is_none() {
        width = r["width"].as_i64();
    }
    if height.is_none() {
        height = r["height"].as_i64();
    }
    let source = r["engine"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| r["source"].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("searxng")
        .to_string();
    Some(SearchResult {
        title: r["title"].as_str().unwrap_or("").to_string(),
        url,
        snippet: r["content"].as_str().unwrap_or("").to_string(),
        published_at: r["publishedDate"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(Into::into),
        thumbnail,
        image,
        width,
        height,
        source,
        ..Default::default()
    })
}

fn media_url(r: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        r[*key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(absolute_http_url)
    })
}

/// Turn a SearxNG media field into a URL the browser can request directly.
fn absolute_http_url(raw: &str) -> Option<String> {
    let raw = proxy_target(raw).unwrap_or_else(|| raw.to_string());
    let raw = raw.trim();
    let absolute = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        raw.to_string()
    };
    if absolute.starts_with("http://") || absolute.starts_with("https://") {
        Some(absolute)
    } else {
        None
    }
}

/// `image_proxy` URLs carry the original address in the `url` query parameter.
fn proxy_target(raw: &str) -> Option<String> {
    let (path, query) = raw.split_once('?')?;
    if !path.contains("image_proxy") {
        return None;
    }
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key == "url" {
            let decoded = urldecode(value);
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

/// SearxNG sends image size as a string, e.g. `1920 x 1080` or `1920×1080`.
fn resolution(raw: &str) -> (Option<i64>, Option<i64>) {
    let mut parts = raw
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty());
    let width = parts.next().and_then(|s| s.parse().ok());
    let height = parts.next().and_then(|s| s.parse().ok());
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_hits_keep_a_browser_reachable_url() {
        let body = serde_json::json!({
            "results": [
                {
                    "title": "Red panda",
                    "url": null,
                    "content": "",
                    "img_src": "/image_proxy?url=https%3A%2F%2Fcdn.example%2Fpanda.jpg&h=abc",
                    "thumbnail_src": "//thumbs.example/panda.jpg",
                    "thumbnail": "",
                    "resolution": "1920 x 1080",
                    "engine": "bing images"
                },
                {
                    "title": "Page only",
                    "url": "https://example.com/article",
                    "content": "a caption",
                    "engine": "google"
                },
                {
                    "title": "nothing usable",
                    "url": "",
                    "img_src": "/relative/only.jpg"
                }
            ]
        });
        let results = parse_results(&body, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Red panda");
        assert_eq!(results[0].url, "https://cdn.example/panda.jpg");
        assert_eq!(
            results[0].image.as_deref(),
            Some("https://cdn.example/panda.jpg")
        );
        assert_eq!(
            results[0].thumbnail.as_deref(),
            Some("https://thumbs.example/panda.jpg")
        );
        assert_eq!(results[0].width, Some(1920));
        assert_eq!(results[0].height, Some(1080));
        assert_eq!(results[0].source, "bing images");
        assert_eq!(results[1].url, "https://example.com/article");
        assert_eq!(results[1].snippet, "a caption");
        assert!(results[1].image.is_none());
    }
}
