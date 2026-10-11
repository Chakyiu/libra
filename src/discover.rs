//! Discover: a topic's recent news, the same job as Vane's Discover page.
//! Each story opens a new thread that summarizes that URL.

use crate::cache::Cache;
use crate::search::SearchHub;
use crate::search::types::{SearchKind, SearchQuery, SearchResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

const TTL_SECS: u64 = 3600;
const SHOW: usize = 24;

struct Topic {
    key: &'static str,
    queries: &'static [&'static str],
    sites: &'static [&'static str],
}

const TOPICS: &[Topic] = &[
    Topic {
        key: "tech",
        queries: &[
            "technology news",
            "latest tech",
            "AI",
            "science and innovation",
        ],
        sites: &["techcrunch.com", "wired.com", "theverge.com"],
    },
    Topic {
        key: "finance",
        queries: &["finance news", "economy", "stock market", "investing"],
        sites: &["bloomberg.com", "cnbc.com", "marketwatch.com"],
    },
    Topic {
        key: "art",
        queries: &["art news", "culture", "modern art", "cultural events"],
        sites: &["artnews.com", "hyperallergic.com", "theartnewspaper.com"],
    },
    Topic {
        key: "sports",
        queries: &["sports news", "latest sports", "cricket football tennis"],
        sites: &["espn.com", "bbc.com", "skysports.com"],
    },
    Topic {
        key: "entertainment",
        queries: &["entertainment news", "movies", "TV shows", "celebrities"],
        sites: &["hollywoodreporter.com", "variety.com", "deadline.com"],
    },
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Article {
    title: String,
    content: String,
    url: String,
    thumbnail: String,
}

pub async fn articles(search: &SearchHub, cache: &Cache, topic: &str) -> Value {
    let topic = resolve_topic(topic);
    let key = format!("discover:news:{}", topic.key);
    let mut blogs = if let Some(cached) = cache.get_json::<Vec<Article>>(&key).await {
        cached
    } else {
        let found = fetch(search, topic).await;
        if !found.is_empty() {
            cache.set_json(&key, &found, TTL_SECS).await;
        }
        found
    };
    shuffle(&mut blogs);
    blogs.truncate(SHOW);
    json!({ "blogs": blogs })
}

fn resolve_topic(key: &str) -> &'static Topic {
    TOPICS
        .iter()
        .find(|t| t.key.eq_ignore_ascii_case(key))
        .unwrap_or(&TOPICS[0])
}

async fn fetch(search: &SearchHub, topic: &Topic) -> Vec<Article> {
    let queries: Vec<String> = topic
        .sites
        .iter()
        .flat_map(|site| {
            topic
                .queries
                .iter()
                .map(move |q| format!("site:{site} {q}"))
        })
        .collect();
    let batches = futures_util::future::join_all(queries.into_iter().map(|query| async move {
        let mut q = SearchQuery {
            query,
            num_results: 5,
            ..Default::default()
        };
        search.run(SearchKind::News, &mut q).await.results
    }))
    .await;
    assemble(batches)
}

fn assemble(batches: Vec<Vec<SearchResult>>) -> Vec<Article> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for batch in batches {
        for result in batch {
            let url = result.url.trim();
            if url.is_empty() || !seen.insert(url.to_ascii_lowercase()) {
                continue;
            }
            let thumbnail = result
                .thumbnail
                .clone()
                .filter(|s| !s.is_empty())
                .or(result.image.filter(|s| !s.is_empty()))
                .unwrap_or_default();
            let title = if result.title.trim().is_empty() {
                url.to_string()
            } else {
                result.title
            };
            out.push(Article {
                title,
                content: result.snippet,
                url: url.to_string(),
                thumbnail,
            });
        }
    }
    out
}

fn shuffle<T>(items: &mut [T]) {
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    for i in (1..items.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let j = (state as usize) % (i + 1);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(url: &str, title: &str, thumbnail: &str, image: &str) -> SearchResult {
        SearchResult {
            title: title.into(),
            url: url.into(),
            snippet: "snippet".into(),
            thumbnail: if thumbnail.is_empty() {
                None
            } else {
                Some(thumbnail.into())
            },
            image: if image.is_empty() {
                None
            } else {
                Some(image.into())
            },
            ..Default::default()
        }
    }

    #[test]
    fn unknown_topic_falls_back_to_tech() {
        assert_eq!(resolve_topic("sports").key, "sports");
        assert_eq!(resolve_topic("nope").key, "tech");
        assert_eq!(resolve_topic("").key, "tech");
    }

    #[test]
    fn assemble_keeps_one_row_per_url_and_a_picture() {
        let out = assemble(vec![
            vec![
                hit("https://Example.com/a", "One", "", "https://img/a.jpg"),
                hit("", "Dropped", "x", ""),
            ],
            vec![
                hit("https://example.com/a", "Duplicate", "other", ""),
                hit("https://example.com/b", "  ", "https://img/b.jpg", ""),
            ],
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "One");
        assert_eq!(out[0].thumbnail, "https://img/a.jpg");
        assert_eq!(out[1].url, "https://example.com/b");
        assert_eq!(out[1].title, "https://example.com/b");
        assert_eq!(out[1].thumbnail, "https://img/b.jpg");
    }
}
