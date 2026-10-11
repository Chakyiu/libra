use crate::blocks::{Block, BlockType};
use crate::config::{Config, SettingsHandle};
use crate::db::{ChatMessage, Db};
use crate::error::LibraError;
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
    /// "fast" | "balance" | "deep". Empty means balance.
    #[serde(default)]
    pub depth: String,
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

        // 5. search. The light model decides the queries first, then runs one or more rounds.
        let mut web = SearchResponse::default();
        let mut news = SearchResponse::default();
        let mut images = SearchResponse::default();
        let mut videos = SearchResponse::default();
        let depth = Depth::parse(&req.depth);
        let mut search_queries: Vec<String> = Vec::new();
        if document_context.is_none() && cls.need_web_search {
            let (results, queries) = self
                .research(&light_llm, &req, &search_query, depth, &tx, &mut blocks)
                .await;
            search_queries = queries;
            web.results = results;
            let mut base = self.base_query(&req, &search_query);
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

        // 6. The fast model drops sources that cannot help. The rerank model then orders what remains.
        let mut candidates = dedupe(web.results, news.results);
        if document_context.is_none() && cls.need_web_search && candidates.len() > 3 {
            let note = Block::research(depth.as_str(), depth.rounds(), depth.rounds(), &[], "Filtering sources", "filter");
            blocks.push(note.clone());
            Self::emit_block(&tx, note);
            candidates = filter_sources(&light_llm, &search_query, candidates).await;
        }
        if cfg.enable_reranker && candidates.len() > 1 {
            let note = Block::research(depth.as_str(), depth.rounds(), depth.rounds(), &[], "Reranking sources", "rerank");
            blocks.push(note.clone());
            Self::emit_block(&tx, note);
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
                "search_query": search_queries.first().cloned().unwrap_or(search_query),
                "search_queries": search_queries,
                "depth": depth.as_str(),
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

    /// Think, then search. Fast is one round; balance is two; deep is three.
    /// Each later round sees what the earlier searches returned and either stops or fills a gap.
    async fn research(
        &self,
        llm: &Llm,
        req: &AskRequest,
        question: &str,
        depth: Depth,
        tx: &UnboundedSender<Event>,
        blocks: &mut Vec<Block>,
    ) -> (Vec<SearchResult>, Vec<String>) {
        let cfg = self.cfg.get();
        let per_query = match depth {
            Depth::Fast => cfg.num_web_results,
            Depth::Balance => cfg.num_web_results.min(12),
            Depth::Deep => cfg.num_web_results.min(8),
        }
        .max(1);
        let mut buckets: Vec<Vec<SearchResult>> = Vec::new();
        let mut asked: Vec<String> = Vec::new();
        let history = history_text(&req.message_history);

        for round in 1..=depth.rounds() {
            let found = findings_brief(&merge_buckets(&buckets));
            let (queries, note) = plan_queries(llm, question, &history, depth, round, &asked, &found).await;
            let queries = if queries.is_empty() && round == 1 {
                vec![question.to_string()]
            } else {
                queries
            };
            if queries.is_empty() {
                break;
            }

            let block = Block::research(depth.as_str(), round, depth.rounds(), &queries, &note, "search");
            blocks.push(block.clone());
            Self::emit_block(tx, block);

            for q in &queries {
                let mut base = self.base_query(req, q);
                base.num_results = per_query;
                let res = self.search.run(SearchKind::Web, &mut base).await;
                buckets.push(res.results);
                asked.push(q.clone());
            }
        }

        // Keep a mix of every round. A plain truncate would keep only the first query's hits.
        let mut results = merge_buckets(&buckets);
        results.truncate(24);
        (results, asked)
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
        let user = prompts::fill(prompts::RERANK_USER, &[("query", query), ("results", &numbered(&results))]);
        let system = prompts::fill(prompts::RERANK_SYSTEM, &[("num", &cfg.rerank_limit.to_string())]);
        let v = match llm.json_call(&system, &user, None).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    model = %llm.model,
                    error = %crate::llm::error_detail(e.as_ref()),
                    "rerank failed; keeping source order"
                );
                return results;
            }
        };
        let Some(indices) = v["selected"].as_array() else {
            return results;
        };
        let out = pick_indices(&results, indices);
        if out.is_empty() { results } else { out }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    Fast,
    Balance,
    Deep,
}

impl Depth {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "fast" => Self::Fast,
            "deep" => Self::Deep,
            _ => Self::Balance,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balance => "balance",
            Self::Deep => "deep",
        }
    }

    /// How many times the engine may think, then search.
    fn rounds(self) -> usize {
        match self {
            Self::Fast => 1,
            Self::Balance => 2,
            Self::Deep => 3,
        }
    }

    fn per_round(self) -> usize {
        match self {
            Self::Fast => 1,
            Self::Balance => 2,
            Self::Deep => 3,
        }
    }
}

async fn plan_queries(
    llm: &Llm,
    question: &str,
    history: &str,
    depth: Depth,
    round: usize,
    asked: &[String],
    found: &str,
) -> (Vec<String>, String) {
    let asked_text = if asked.is_empty() {
        "(none yet)".to_string()
    } else {
        asked.iter().map(|q| format!("- {q}")).collect::<Vec<_>>().join("\n")
    };
    let system = prompts::fill(prompts::PLAN_SEARCH_SYSTEM, &[("n", &depth.per_round().to_string())]);
    let user = prompts::fill(
        prompts::PLAN_SEARCH_USER,
        &[
            ("query", question),
            ("history", history),
            ("round", &round.to_string()),
            ("rounds", &depth.rounds().to_string()),
            ("asked", &asked_text),
            ("found", found),
        ],
    );
    let v = match llm.json_call(&system, &user, Some(500)).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                model = %llm.model,
                provider = ?llm.kind,
                error = %crate::llm::error_detail(e.as_ref()),
                "search plan failed; the light model could not choose queries, so round {round} searches the question itself"
            );
            return (Vec::new(), String::new());
        }
    };
    let note = v["note"].as_str().unwrap_or("").trim().to_string();
    if round > 1 && v["enough"].as_bool().unwrap_or(false) {
        return (Vec::new(), note);
    }
    (take_queries(&v, depth.per_round(), asked), note)
}

/// The fast model throws away sources that cannot help, before the rerank model orders the rest.
async fn filter_sources(llm: &Llm, query: &str, results: Vec<SearchResult>) -> Vec<SearchResult> {
    let user = prompts::fill(
        prompts::FILTER_SOURCES_USER,
        &[("query", query), ("results", &numbered(&results))],
    );
    let v = match llm.json_call(prompts::FILTER_SOURCES_SYSTEM, &user, Some(500)).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                model = %llm.model,
                provider = ?llm.kind,
                error = %crate::llm::error_detail(e.as_ref()),
                "source filter failed; keeping every result"
            );
            return results;
        }
    };
    let Some(indices) = v["keep"].as_array() else {
        tracing::warn!(model = %llm.model, "source filter returned no keep list; keeping every result");
        return results;
    };
    let kept = pick_indices(&results, indices);
    if kept.is_empty() { results } else { kept }
}

fn numbered(results: &[SearchResult]) -> String {
    results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("[{}] {} — {}\n{}", i + 1, r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn pick_indices(results: &[SearchResult], indices: &[serde_json::Value]) -> Vec<SearchResult> {
    let mut out = Vec::with_capacity(indices.len());
    for idx in indices {
        let Some(i) = idx.as_u64().and_then(|n| usize::try_from(n).ok()).map(|n| n.saturating_sub(1)) else {
            continue;
        };
        let Some(r) = results.get(i) else { continue };
        if !out.iter().any(|x: &SearchResult| x.url == r.url) {
            out.push(r.clone());
        }
    }
    out
}

fn query_key(q: &str) -> String {
    q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Keep the first `limit` new search strings. Drops blanks, repeats, and anything already run.
fn take_queries(raw: &serde_json::Value, limit: usize, already: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = already.iter().map(|q| query_key(q)).filter(|k| !k.is_empty()).collect();
    let mut out = Vec::new();
    let Some(arr) = raw.get("queries").and_then(|v| v.as_array()) else {
        return out;
    };
    for q in arr {
        let Some(s) = q.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let s: String = s.chars().take(180).collect();
        let key = query_key(&s);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        out.push(s);
        if out.len() == limit {
            break;
        }
    }
    out
}

/// Round-robin across queries so a later round is not dropped when the list is capped.
fn merge_buckets(buckets: &[Vec<SearchResult>]) -> Vec<SearchResult> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let max = buckets.iter().map(|b| b.len()).max().unwrap_or(0);
    for i in 0..max {
        for bucket in buckets {
            let Some(r) = bucket.get(i) else { continue };
            let key = normalize_url(&r.url);
            if key.is_empty() || seen.insert(key) {
                out.push(r.clone());
            }
        }
    }
    out
}

fn findings_brief(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "(nothing yet)".into();
    }
    results
        .iter()
        .take(8)
        .enumerate()
        .map(|(i, r)| {
            let snip: String = r.snippet.chars().take(160).collect();
            format!("[{}] {} — {}\n{snip}", i + 1, r.title, r.url)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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

pub async fn title_and_emoji(cfg: &Config, http: &reqwest::Client, db: &Db, chat_id: &str, query: &str) -> Result<(), LibraError> {
    let llm = llm::resolve_chat(cfg, http, cfg.light_model())?;
    let v = llm
        .json_call(prompts::QUERY_TITLE_SYSTEM, &prompts::fill(prompts::QUERY_TITLE_USER, &[("query", query)]), None)
        .await
        .unwrap_or_else(|_| json!({ "title": query }));
    let title = v["title"].as_str().unwrap_or(query).to_string();
    let emoji = v["emoji"].as_str().unwrap_or("🔍").to_string();
    db.rename_chat(chat_id, &title, &emoji).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hit(url: &str) -> SearchResult {
        SearchResult {
            title: url.into(),
            url: url.into(),
            ..SearchResult::default()
        }
    }

    #[test]
    fn pick_indices_keeps_order_and_skips_unknowns() {
        let results = vec![hit("https://a.example"), hit("https://b.example"), hit("https://c.example")];
        let kept = pick_indices(&results, &vec![json!(3), json!(1), json!(9), json!(1)]);
        let urls: Vec<_> = kept.into_iter().map(|r| r.url).collect();
        assert_eq!(urls, vec!["https://c.example".to_string(), "https://a.example".to_string()]);
    }

    #[test]
    fn depth_defaults_to_balance() {
        assert_eq!(Depth::parse(""), Depth::Balance);
        assert_eq!(Depth::parse("FAST"), Depth::Fast);
        assert_eq!(Depth::parse("deep"), Depth::Deep);
        assert_eq!(Depth::Fast.rounds(), 1);
        assert_eq!(Depth::Balance.rounds(), 2);
        assert_eq!(Depth::Deep.per_round(), 3);
    }

    #[test]
    fn take_queries_caps_and_skips_repeats() {
        let raw = json!({
            "queries": ["nginx 1.30.1", "  ", "Nginx   1.30.1", "rhel 9 nginx", "extra", "fourth"]
        });
        let got = take_queries(&raw, 2, &["nginx 1.30.1".into()]);
        assert_eq!(got, vec!["rhel 9 nginx".to_string(), "extra".to_string()]);
    }

    #[test]
    fn merge_buckets_interleaves_rounds() {
        let buckets = vec![
            vec![hit("https://a.example/1"), hit("https://a.example/2")],
            vec![hit("https://b.example/1"), hit("https://a.example/1")],
        ];
        let urls: Vec<_> = merge_buckets(&buckets).into_iter().map(|r| r.url).collect();
        assert_eq!(
            urls,
            vec![
                "https://a.example/1".to_string(),
                "https://b.example/1".to_string(),
                "https://a.example/2".to_string(),
            ]
        );
    }
}

