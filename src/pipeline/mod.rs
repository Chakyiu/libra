use crate::blocks::{Block, BlockType};
use crate::config::{Config, SettingsHandle};
use crate::db::{ChatMessage, Db};
use crate::error::VaneError;
use crate::llm::{self, ChatRequest, Delta, Llm, Msg};
use crate::prompts;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::search::SearchHub;
use crate::state::Event;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

pub struct Pipeline {
    pub cfg: SettingsHandle,
    pub db: Arc<Db>,
    pub http: Client,
    pub search: Arc<SearchHub>,
    pub widgets: Arc<crate::widgets::Registry>,
    pub uploads: Arc<crate::uploads::UploadStore>,
}

#[derive(Debug, Deserialize)]
pub struct AskRequest {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub models: Models,
    #[serde(default)]
    pub message_history: Vec<Msg>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub filters: Filters,
    #[serde(default)]
    pub disabled_widgets: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Models {
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub light: String,
    #[serde(default)]
    pub rerank: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct Filters {
    #[serde(default)]
    pub perplexity_site: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub freshness: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Classification {
    #[serde(default = "default_type")]
    r#type: String,
    #[serde(default)]
    need_web_search: bool,
    #[serde(default)]
    needs_image_results: bool,
    #[serde(default)]
    needs_video_results: bool,
    #[serde(default)]
    content_generation: Option<String>,
}

fn default_type() -> String {
    "normal".into()
}

impl Pipeline {
    fn emit(tx: &UnboundedSender<Event>, event: Event) {
        let _ = tx.send(event);
    }

    fn emit_block(tx: &UnboundedSender<Event>, block: Block) {
        Self::emit(tx, Event::Block(block));
    }

    /// Run the whole answer pipeline. Every stage degrades instead of failing the request.
    pub async fn ask(self: &Arc<Self>, req: AskRequest, tx: UnboundedSender<Event>) {
        let cfg = self.cfg.get();
        let started = std::time::Instant::now();
        let chat_id = req.id.clone();
        let query = req.query.clone();
        Self::emit(&tx, Event::Id(chat_id.clone()));
        // The client generates the chat id, so the row may not exist yet.
        if let Err(e) = self.db.create_chat(&chat_id, &cfg.now()).await {
            tracing::warn!("could not create chat row: {e}");
        }

        let answer_model = if req.models.answer.is_empty() {
            req.model.clone().unwrap_or_else(|| cfg.default_model.clone())
        } else {
            req.models.answer.clone()
        };
        let light_model = if req.models.light.is_empty() {
            cfg.default_light_model.clone()
        } else {
            req.models.light.clone()
        };
        let rerank_model = if req.models.rerank.is_empty() {
            cfg.default_rerank_model.clone()
        } else {
            req.models.rerank.clone()
        };

        let answer_llm = match llm::resolve_chat(&cfg, &self.http, &answer_model) {
            Ok(l) => l,
            Err(e) => return Self::emit(&tx, Event::Close(Some(e.to_string()))),
        };
        let light_llm = llm::resolve_chat(&cfg, &self.http, &light_model).unwrap_or_else(|_| answer_llm.clone());
        let rerank_llm = llm::resolve_chat(&cfg, &self.http, &rerank_model).unwrap_or_else(|_| light_llm.clone());

        let mut blocks: Vec<Block> = Vec::new();

        // 1. classify the query
        let cls = match self.classify(&light_llm, &req).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("classification failed, assuming web search: {e}");
                Classification {
                    r#type: "normal".into(),
                    need_web_search: true,
                    needs_image_results: false,
                    needs_video_results: false,
                    content_generation: None,
                }
            }
        };
        let classification = Block {
            block_type: BlockType::Answered,
            message: format!(
                "type={} web_search={} images={} videos={}",
                cls.r#type, cls.need_web_search, cls.needs_image_results, cls.needs_video_results
            ),
            data: Some(json!({
                "type": cls.r#type,
                "need_web_search": cls.need_web_search,
                "needs_image_results": cls.needs_image_results,
                "needs_video_results": cls.needs_video_results,
                "content_generation": cls.content_generation,
            })),
            extra: None,
        };
        blocks.push(classification.clone());
        Self::emit_block(&tx, classification);

        // 2. widgets (calculator / time / weather / stocks)
        let widget_blocks = self.widgets_for(&light_llm, &req, &cls).await;
        for b in widget_blocks.iter() {
            blocks.push(b.clone());
            Self::emit_block(&tx, b.clone());
        }

        // 3. resolve the search query (follow-up rewriting)
        let search_query = if cls.r#type == "followup" && !req.message_history.is_empty() {
            match self.rewrite_followup(&light_llm, &req).await {
                Some(q) if !q.is_empty() => q,
                _ => req.query.clone(),
            }
        } else {
            req.query.clone()
        };

        // 4. documents uploaded for this chat take priority over the web
        let document_context = if !req.files.is_empty() {
            self.document_context(&req, &search_query).await
        } else {
            None
        };

        // 5. search
        let mut web = SearchResponse::default();
        let mut news = SearchResponse::default();
        let mut images = SearchResponse::default();
        let mut videos = SearchResponse::default();
        if document_context.is_none() && cls.need_web_search {
            let mut base = self.base_query(&req, &search_query);
            web = self.search.run(SearchKind::Web, &mut base.clone()).await;
            if cfg.enable_fresh_search && req.filters.freshness.as_deref().unwrap_or("noLimit") != "noLimit" {
                news = self.search.run(SearchKind::News, &mut base.clone()).await;
            }
            if cls.needs_image_results && cfg.enable_image_search {
                images = self.search.run(SearchKind::Images, &mut base.clone()).await;
            }
            if cls.needs_video_results && cfg.enable_video_search {
                videos = self.search.run(SearchKind::Videos, &mut base).await;
            }
        }

        // 6. dedupe, merge, rerank
        let mut candidates = dedupe(web.results, news.results);
        let candidates_count = candidates.len();
        if candidates_count > cfg.rerank_limit && cfg.enable_reranker {
            candidates = self.rerank(&rerank_llm, &search_query, candidates).await;
        }
        candidates.truncate(cfg.rerank_limit);

        let references = build_references(&candidates);
        let ref_block = Block::references(candidates.clone());
        blocks.push(ref_block.clone());
        Self::emit_block(&tx, ref_block);

        let related = Block::videos(videos.results.clone());
        if !videos.results.is_empty() {
            blocks.push(related.clone());
            Self::emit_block(&tx, related);
        }

        // 7. images
        if !images.results.is_empty() {
            let picked = self.select_images(&light_llm, &search_query, images.results.clone()).await;
            let block = Block::images(picked);
            blocks.push(block.clone());
            Self::emit_block(&tx, block);
        }

        // 8. the answer itself
        let system = prompts::fill(
            if document_context.is_some() {
                prompts::DOCUMENT_SYSTEM
            } else if cls.r#type == "coding" {
                prompts::CODING_SYSTEM
            } else {
                prompts::ANSWER_SYSTEM
            },
            &[
                ("date", &cfg.today()),
                ("year", &cfg.year()),
            ],
        );
        let user_prompt = if let Some(chunks) = &document_context {
            prompts::fill(prompts::DOCUMENT_USER, &[("chunks", chunks), ("query", &req.query)])
        } else {
            prompts::fill(
                prompts::ANSWER_USER,
                &[("references", &references), ("query", &req.query)],
            )
        };

        let mut messages: Vec<Msg> = Vec::new();
        for m in req.message_history.iter().rev().take(10).rev() {
            if m.role == "user" || m.role == "assistant" {
                messages.push(m.clone());
            }
        }
        messages.push(msg("user", &user_prompt));

        let mut rx = match answer_llm.stream(ChatRequest {
            messages: std::iter::once(msg("system", &system))
                .chain(messages.into_iter())
                .collect(),
            temperature: Some(cfg.default_temperature),
            max_tokens: None,
            json_mode: false,
        }).await {
            Ok(rx) => rx,
            Err(e) => return Self::emit(&tx, Event::Close(Some(format!("answer model failed: {e}")))),
        };
        let mut answer = String::new();
        while let Some(delta) = rx.recv().await {
            match delta {
                Delta::Text(t) => {
                    answer.push_str(&t);
                    Self::emit(&tx, Event::AnswerChunk(t));
                }
                Delta::Error(e) => {
                    tracing::warn!("answer stream error: {e}");
                    if answer.is_empty() {
                        Self::emit(&tx, Event::Close(Some(e)));
                        return;
                    }
                    break;
                }
            }
        }

        blocks.push(Block::message(&answer));

        // 9. follow-up questions shown under the answer
        let suggestions = self.suggestions(&light_llm, &req.query, &answer).await;
        if !suggestions.is_empty() {
            let block = Block::related(suggestions);
            blocks.push(block.clone());
            Self::emit_block(&tx, block);
        }

        blocks.push(Block {
            block_type: BlockType::Answered,
            message: format!("answered in {}ms", started.elapsed().as_millis()),
            data: Some(json!({
                "model": answer_model,
                "light_model": light_model,
                "rerank_model": rerank_model,
                "elapsed_ms": started.elapsed().as_millis(),
                "sources": candidates.len(),
                "search_query": search_query,
            })),
            extra: None,
        });

        // 9. persist
        if let Err(e) = self
            .db
            .save_message(&chat_id, &ChatMessage {
                id: uuid::Uuid::new_v4().to_string(),
                role: "user".into(),
                content: req.query.clone(),
                blocks: vec![],
                files: req.files.clone(),
                created_at: cfg.now(),
            })
            .await
        {
            tracing::warn!("failed to save user message: {e}");
        }
        if let Err(e) = self
            .db
            .save_message(&chat_id, &ChatMessage {
                id: uuid::Uuid::new_v4().to_string(),
                role: "assistant".into(),
                content: answer.clone(),
                blocks,
                files: vec![],
                created_at: cfg.now(),
            })
            .await
        {
            tracing::warn!("failed to save assistant message: {e}");
        }

        // 10. name the conversation from its first question
        if req.message_history.is_empty() {
            let (cfg, http, db) = (self.cfg.get(), self.http.clone(), self.db.clone());
            tokio::spawn(async move {
                if let Err(e) = title_and_emoji(&cfg, &http, &db, &chat_id, &query).await {
                    tracing::warn!("chat titling failed: {e}");
                }
            });
        }

        Self::emit(&tx, Event::Close(None));
    }

    fn base_query(&self, req: &AskRequest, search_query: &str) -> SearchQuery {
        let cfg = self.cfg.get();
        SearchQuery {
            query: search_query.to_string(),
            kind: SearchKind::Web,
            num_results: cfg.num_web_results,
            country: Some(req.filters.country.clone().unwrap_or_else(|| cfg.country.clone())),
            language: req.filters.language.clone(),
            freshness: Some(req.filters.freshness.clone().unwrap_or_else(|| cfg.freshness.clone())),
            safe_search: cfg.safe_search,
            site: req.filters.perplexity_site.clone(),
            page: None,
        }
    }

    async fn classify(&self, llm: &Llm, req: &AskRequest) -> anyhow::Result<Classification> {
        let history = history_text(&req.message_history);
        let user = prompts::fill(prompts::CLASSIFY_USER, &[("query", &req.query), ("history", &history)]);
        let v = llm.json_call(prompts::CLASSIFY_SYSTEM, &user, None).await?;
        Ok(serde_json::from_value(v)?)
    }

    async fn rewrite_followup(&self, llm: &Llm, req: &AskRequest) -> Option<String> {
        let history = history_text(&req.message_history);
        let user = prompts::fill(
            prompts::DETERMINE_FOLLOWUP_USER,
            &[("history", &history), ("query", &req.query)],
        );
        let v = llm
            .json_call(prompts::DETERMINE_FOLLOWUP_SYSTEM, &user, None)
            .await
            .ok()?;
        v["question"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    async fn widgets_for(&self, llm: &Llm, req: &AskRequest, cls: &Classification) -> Vec<Block> {
        let cfg = self.cfg.get();
        // Follow-ups and document questions never need a widget.
        if cls.r#type == "followup" || !req.files.is_empty() {
            return vec![];
        }
        let user = prompts::fill(prompts::CHOOSE_WIDGETS_USER, &[("query", &req.query), ("date", &cfg.today())]);
        let Ok(v) = llm.json_call(prompts::CHOOSE_WIDGETS_SYSTEM, &user, None).await else {
            return vec![];
        };
        let requested: Vec<crate::widgets::WidgetRequest> = serde_json::from_value(v["widgets"].clone()).unwrap_or_default();
        self.widgets.render(&requested, &req.query, &req.disabled_widgets).await
    }

    async fn rerank(&self, llm: &Llm, query: &str, results: Vec<SearchResult>) -> Vec<SearchResult> {
        let cfg = self.cfg.get();
        let list = results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("[{}] {} — {}\n{}", i + 1, r.title, r.url, r.snippet))
            .collect::<Vec<_>>()
            .join("\n\n");
        let user = prompts::fill(
            prompts::RERANK_USER,
            &[("query", query), ("results", &list)],
        );
        let system = prompts::fill(prompts::RERANK_SYSTEM, &[("num", &cfg.rerank_limit.to_string())]);
        let Ok(v) = llm.json_call(&system, &user, None).await else {
            return results;
        };
        let Some(indices) = v["selected"].as_array() else {
            return results;
        };
        let mut out = Vec::with_capacity(indices.len());
        for idx in indices {
            if let Some(i) = idx.as_u64().and_then(|i| usize::try_from(i).ok()).map(|i| i.saturating_sub(1)) {
                if let Some(r) = results.get(i) {
                    if !out.iter().any(|x: &SearchResult| x.url == r.url) {
                        out.push(r.clone());
                    }
                }
            }
        }
        if out.is_empty() {
            results
        } else {
            out
        }
    }

    async fn select_images(&self, llm: &Llm, query: &str, results: Vec<SearchResult>) -> Vec<SearchResult> {
        let _cfg = self.cfg.get();
        if results.len() <= 6 {
            return results;
        }
        let list = results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("[{i}] {} — {}", r.title, r.image.clone().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n");
        let user = prompts::fill(prompts::IMAGE_SELECTION_USER, &[("query", query), ("images", &list)]);
        let Ok(v) = llm.json_call(prompts::IMAGE_SELECTION_SYSTEM, &user, None).await else {
            return results.into_iter().take(6).collect();
        };
        let mut out = Vec::new();
        for idx in v["selected"].as_array().cloned().unwrap_or_default() {
            if let Some(i) = idx.as_u64().and_then(|i| usize::try_from(i).ok()) {
                if let Some(r) = results.get(i) {
                    out.push(r.clone());
                }
            }
        }
        if out.is_empty() {
            results.into_iter().take(6).collect()
        } else {
            out
        }
    }

    async fn suggestions(&self, llm: &Llm, query: &str, answer: &str) -> Vec<String> {
        let excerpt: String = answer.chars().take(1500).collect();
        let user = prompts::fill(
            prompts::DISCOVER_QUESTIONS_USER,
            &[("history", &format!("Q: {query}\nA: {excerpt}"))],
        );
        let Ok(v) = llm.json_call(prompts::DISCOVER_QUESTIONS_SYSTEM, &user, None).await else {
            return vec![];
        };
        v["questions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|q| q.as_str().map(|s| s.trim().to_string()))
            .filter(|q| !q.is_empty())
            .take(3)
            .collect()
    }

    async fn document_context(&self, req: &AskRequest, query: &str) -> Option<String> {
        let cfg = self.cfg.get();
        let chat_id = req.id.clone();
        let chunks = self
            .uploads
            .retrieve(&chat_id, &req.files, query, cfg.document_chunk_limit)
            .await
            .map_err(|e| tracing::warn!("document retrieval failed: {e}"))
            .ok()?;
        if chunks.is_empty() {
            return None;
        }
        Some(
            chunks
                .iter()
                .enumerate()
                .map(|(i, c)| format!("[{}] ({})\n{}", i + 1, c.filename, c.content))
                .collect::<Vec<_>>()
                .join("\n\n---\n\n"),
        )
    }
}

fn msg(role: &str, content: impl Into<String>) -> Msg {
    Msg {
        role: role.into(),
        content: content.into(),
    }
}

fn history_text(history: &[Msg]) -> String {
    if history.is_empty() {
        return "(none)".into();
    }
    history
        .iter()
        .rev()
        .take(6)
        .rev()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Merge web + news results, dropping duplicate URLs and tracking the reference index.
fn dedupe(primary: Vec<SearchResult>, secondary: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(primary.len() + secondary.len());
    for r in primary.into_iter().chain(secondary) {
        let key = normalize_url(&r.url);
        if key.is_empty() || seen.insert(key) {
            out.push(r);
        }
    }
    out
}

fn normalize_url(url: &str) -> String {
    let mut u = url.trim();
    if let Some(rest) = u.strip_prefix("https://") {
        u = rest;
    } else if let Some(rest) = u.strip_prefix("http://") {
        u = rest;
    }
    u = u.split('#').next().unwrap_or(u);
    u = u.split('?').next().unwrap_or(u);
    u.trim_end_matches('/').to_lowercase()
}

/// The numbered reference block the answer prompt cites against.
fn build_references(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "(no search results — answer from what you know and say you could not verify)".into();
    }
    results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let body = r
                .content
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .unwrap_or(&r.snippet);
            let body: String = body.chars().take(1200).collect();
            format!(
                "[{}] {}\nURL: {}\n{}\n{}",
                i + 1,
                r.title,
                r.url,
                r.published_at
                    .as_deref()
                    .map(|p| format!("Published: {p}"))
                    .unwrap_or_default(),
                body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub async fn title_and_emoji(cfg: &Config, http: &reqwest::Client, db: &Db, chat_id: &str, query: &str) -> Result<(), VaneError> {
    let llm = llm::resolve_chat(cfg, http, &cfg.default_light_model)?;
    let v = llm
        .json_call(prompts::QUERY_TITLE_SYSTEM, &prompts::fill(prompts::QUERY_TITLE_USER, &[("query", query)]), None)
        .await
        .unwrap_or_else(|_| json!({ "title": query }));
    let title = v["title"].as_str().unwrap_or(query).to_string();
    let emoji = v["emoji"].as_str().unwrap_or("🔍").to_string();
    db.rename_chat(chat_id, &title, &emoji).await?;
    Ok(())
}

