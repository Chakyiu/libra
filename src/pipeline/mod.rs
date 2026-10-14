use crate::blocks::{Block, BlockType};
use crate::config::{Config, SettingsHandle};
use crate::db::{ChatMessage, Db};
use crate::error::LibraError;
use crate::llm::{self, ChatRequest, Delta, Llm, Msg};
use crate::prompts;
use crate::search::SearchHub;
use crate::search::types::{SearchKind, SearchQuery, SearchResponse, SearchResult};
use crate::state::Event;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

/// In-flight answers. Stop can land before the pipeline has registered, or just after it
/// has finished; those two cases must not cancel the next question on the same thread.
#[derive(Clone, Default)]
pub struct Runs {
    inner: Arc<Mutex<RunBook>>,
}

#[derive(Default)]
struct RunBook {
    live: HashMap<String, Stop>,
    armed: HashMap<String, Instant>,
    ended: HashMap<String, Instant>,
}

/// Cooperative cancel flag plus a wake-up, so a stage blocked on the network
/// can be dropped immediately instead of running until it finishes.
#[derive(Clone)]
struct Stop {
    inner: Arc<StopInner>,
}

struct StopInner {
    flag: AtomicBool,
    notify: Notify,
    /// Answer-token HTTP read. Aborted on stop even if the pipeline is in a short DB write.
    stream: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Stop {
    fn new(stopped: bool) -> Self {
        Self {
            inner: Arc::new(StopInner {
                flag: AtomicBool::new(stopped),
                notify: Notify::new(),
                stream: Mutex::new(None),
            }),
        }
    }

    fn load(&self, order: Ordering) -> bool {
        self.inner.flag.load(order)
    }

    fn trip(&self) {
        self.inner.flag.store(true, Ordering::Release);
        if let Some(abort) = self
            .inner
            .stream
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            abort.abort();
        }
        // notify_one leaves a permit if the waiter has not polled yet.
        self.inner.notify.notify_waiters();
        self.inner.notify.notify_one();
    }

    fn watch_stream(&self, abort: tokio::task::AbortHandle) {
        let mut slot = self.inner.stream.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = slot.replace(abort) {
            prev.abort();
        }
    }

    fn clear_stream(&self) {
        self.inner
            .stream
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    async fn wait(&self) {
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            if self.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

struct Halted;

/// Reasoning collected for the page and for the saved thread.
struct Mind<'a> {
    tx: &'a UnboundedSender<Event>,
    text: &'a mut String,
}

impl Mind<'_> {
    fn push(&mut self, chunk: impl AsRef<str>) {
        let chunk = chunk.as_ref();
        if chunk.is_empty() {
            return;
        }
        self.text.push_str(chunk);
        let _ = self.tx.send(Event::Thought(chunk.to_string()));
    }

    /// Roll the trace back to just before the repeated line, so a thread that hit a
    /// reasoning loop keeps the thinking that came before it instead of the garbage.
    fn cut_loop(&mut self, line: &str) {
        if let Some(i) = self.text.rfind(line) {
            self.text.truncate(i);
        }
    }
}

/// A reasoning model can fall into a repetition loop inside its thinking — the same
/// `Need mention X [12]. Good.` line forever, or that whole claim with only a tail swapped —
/// and spend the whole context window before it writes a word of the answer. The answer
/// call has no token cap (a long answer must not be cut off), so a real loop has to be
/// recognised rather than bounded. A long trace that keeps adding new claims is not a
/// loop. Watched on the reasoning trace only: an answer legitimately repeats short lines
/// (table rules), thinking does not repeat long ones.
pub struct LoopGuard {
    /// the last `WINDOW` non-trivial thought lines, oldest first
    lines: std::collections::VecDeque<String>,
    /// the line still being streamed
    pending: String,
    chars: usize,
}

/// How much thinking is pathological whatever it says: roughly 10k tokens.
const BUDGET: usize = 40_000;
const WINDOW: usize = 60;
const SHORTEST: usize = 24;
/// How much of a line must be identical before two thoughts are the same claim.
/// Eighteen characters is just how a trace starts a new point ("Need maybe mention"),
/// so a shorter stem treats a long, still-moving trace as a loop.
const STEM: usize = 48;
/// How many times that claim may show up in the window before it is a loop.
const STEM_HITS: usize = 8;

/// What the user sees when the model loops. An empty answer with no provider error is
/// otherwise indistinguishable from a model that simply had nothing to say.
const LOOPING: &str = "the model got stuck repeating itself while reasoning and never wrote an answer. Try a model that thinks less, a lower Temperature, or fewer Sources per answer.";
const BUDGET_USED: &str = "the model spent its whole token budget on reasoning and wrote no answer. Try a model that thinks less, or a lower Temperature.";
const NO_TEXT: &str = "the model returned no answer text.";

impl Default for LoopGuard {
    fn default() -> Self {
        Self {
            lines: Default::default(),
            pending: String::new(),
            chars: 0,
        }
    }
}

impl LoopGuard {
    /// The line whose repetition ended the trace, when the trace has looped.
    /// `Some("")` is the character budget: the trace is long, and nothing repeated.
    pub fn feed(&mut self, chunk: &str) -> Option<String> {
        self.chars += chunk.chars().count();
        if self.chars > BUDGET {
            return Some(String::new());
        }
        self.pending.push_str(chunk);
        while let Some(i) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=i).collect();
            if let Some(hit) = self.watch(&line) {
                return Some(hit);
            }
        }
        None
    }

    fn watch(&mut self, raw: &str) -> Option<String> {
        let line = raw.trim();
        if line.chars().count() < SHORTEST {
            return None;
        }
        let line: String = line.chars().take(200).collect();
        // Three copies of the same long line inside a page of thinking is a loop, not emphasis.
        let again = self.lines.iter().filter(|l| **l == line).count() >= 2;
        self.lines.push_back(line.clone());
        if self.lines.len() > WINDOW {
            self.lines.pop_front();
        }
        if again {
            return Some(line);
        }
        // The same claim with a different tail: a citation or a number moved, the sentence
        // did not. A shared short opening is a new point, so it does not count.
        let stem = stem_of(&line);
        let hits = self.lines.iter().filter(|l| stem_of(l) == stem).count();
        if hits >= STEM_HITS {
            return self.lines.iter().find(|l| stem_of(l) == stem).cloned();
        }
        None
    }
}

/// The opening of a thought line, lowercased, so a rewritten checklist item compares equal.
fn stem_of(line: &str) -> String {
    line.chars().take(STEM).collect::<String>().to_lowercase()
}

fn store_thought(blocks: &mut Vec<Block>, thought: &str) {
    if thought.trim().is_empty() {
        return;
    }
    if let Some(block) = blocks
        .iter_mut()
        .find(|b| b.block_type == BlockType::Thought)
    {
        block.message = thought.to_string();
    } else {
        blocks.push(Block::thought(thought));
    }
}

/// Run `work` until it finishes or Stop is clicked.
/// A closed event channel is a refresh: the work keeps running and checkpoints
/// keep landing, so the thread can pick the answer up again.
/// Dropping `work` cancels the HTTP request it is currently awaiting.
async fn cut<T>(
    stop: &Stop,
    _tx: &UnboundedSender<Event>,
    work: impl Future<Output = T>,
) -> Result<T, Halted> {
    if stop.load(Ordering::Acquire) {
        return Err(Halted);
    }
    tokio::pin!(work);
    tokio::select! {
        biased;
        _ = stop.wait() => Err(Halted),
        out = &mut work => Ok(out),
    }
}

impl Runs {
    fn book(&self) -> std::sync::MutexGuard<'_, RunBook> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register this ask. The flag is already set when a stop arrived before the pipeline did.
    fn begin(&self, chat_id: &str) -> Stop {
        let mut book = self.book();
        book.armed
            .retain(|_, t| t.elapsed() < Duration::from_secs(60));
        book.ended
            .retain(|_, t| t.elapsed() < Duration::from_secs(60));
        let stopped = book
            .armed
            .remove(chat_id)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        let flag = Stop::new(stopped);
        book.live.insert(chat_id.to_string(), flag.clone());
        flag
    }

    pub fn cancel(&self, chat_id: &str) {
        let mut book = self.book();
        if let Some(flag) = book.live.get(chat_id) {
            flag.trip();
            return;
        }
        // A stop for a run that just finished must not arm the next question.
        if book
            .ended
            .get(chat_id)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        book.armed.insert(chat_id.to_string(), Instant::now());
    }

    pub fn end(&self, chat_id: &str) {
        let mut book = self.book();
        book.live.remove(chat_id);
        book.ended.insert(chat_id.to_string(), Instant::now());
    }
}

/// Coordinates or a timezone the browser reported for one question.
#[derive(Clone, Debug)]
pub struct GeoFix {
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
    pub denied: bool,
}

/// In-flight location requests. A late cancel from the previous question must not
/// drop the waiter for the next one.
#[derive(Clone, Default)]
pub struct Locations {
    inner: Arc<Mutex<LocationBook>>,
}

#[derive(Default)]
struct LocationBook {
    slots: HashMap<String, LocationSlot>,
    next: u64,
}

struct LocationSlot {
    generation: u64,
    tx: oneshot::Sender<GeoFix>,
}

struct LocationWait {
    locations: Locations,
    chat_id: String,
    generation: u64,
}

impl Drop for LocationWait {
    fn drop(&mut self) {
        self.locations.cancel(&self.chat_id, self.generation);
    }
}

impl Locations {
    fn lock(&self) -> std::sync::MutexGuard<'_, LocationBook> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn expect(&self, chat_id: &str) -> (oneshot::Receiver<GeoFix>, LocationWait) {
        let (tx, rx) = oneshot::channel();
        let mut book = self.lock();
        book.next += 1;
        let generation = book.next;
        book.slots
            .insert(chat_id.to_string(), LocationSlot { generation, tx });
        (
            rx,
            LocationWait {
                locations: self.clone(),
                chat_id: chat_id.to_string(),
                generation,
            },
        )
    }

    fn provide(&self, chat_id: &str, fix: GeoFix) {
        if let Some(slot) = self.lock().slots.remove(chat_id) {
            let _ = slot.tx.send(fix);
        }
    }

    fn cancel(&self, chat_id: &str, generation: u64) {
        let mut book = self.lock();
        if book
            .slots
            .get(chat_id)
            .is_some_and(|slot| slot.generation == generation)
        {
            book.slots.remove(chat_id);
        }
    }
}

struct LiveTurn {
    chat_id: String,
    assistant_id: String,
    model: String,
    started: Instant,
    cancel: Stop,
}

struct RunGuard {
    runs: Runs,
    chat_id: String,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.runs.end(&self.chat_id);
    }
}

pub struct Pipeline {
    pub cfg: SettingsHandle,
    pub db: Arc<Db>,
    pub http: Client,
    pub search: Arc<SearchHub>,
    pub widgets: Arc<crate::widgets::Registry>,
    pub uploads: Arc<crate::uploads::UploadStore>,
    pub runs: Runs,
    pub locations: Locations,
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
    /// "answer" (default) or "discussion" — a long-form report written section by section.
    #[serde(default)]
    pub mode: String,
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

#[derive(Default)]
struct WidgetChoice {
    requested: Vec<crate::widgets::WidgetRequest>,
    needs_location: bool,
}

impl Pipeline {
    pub fn provide_location(&self, chat_id: &str, fix: GeoFix) {
        self.locations.provide(chat_id, fix);
    }

    fn emit(tx: &UnboundedSender<Event>, event: Event) {
        let _ = tx.send(event);
    }

    fn emit_block(tx: &UnboundedSender<Event>, block: Block) {
        Self::emit(tx, Event::Block(block));
    }

    /// Persist the answer so far. Failures are logged; the stream keeps going.
    async fn checkpoint(
        &self,
        turn: &LiveTurn,
        answer: &str,
        blocks: &[Block],
        phase: &str,
        error: &str,
    ) {
        if turn.assistant_id.is_empty() {
            return;
        }
        let now = self.cfg.get().now();
        if let Err(e) = self
            .db
            .write_turn(
                &turn.chat_id,
                &turn.assistant_id,
                answer,
                blocks,
                phase,
                error,
                &turn.model,
                turn.started.elapsed().as_millis() as i64,
                &now,
            )
            .await
        {
            tracing::warn!("could not store thread state: {e}");
        }
    }

    /// Stop was clicked. A refresh only drops the event channel; the answer continues.
    async fn halt(
        &self,
        turn: &LiveTurn,
        answer: &str,
        blocks: &[Block],
        tx: &UnboundedSender<Event>,
    ) -> bool {
        if !turn.cancel.load(Ordering::Acquire) {
            return false;
        }
        tracing::debug!(chat_id = %turn.chat_id, "answer stopped");
        turn.cancel.trip();
        self.checkpoint(turn, answer, blocks, "stopped", "").await;
        Self::emit(tx, Event::Close(None));
        true
    }

    async fn give_up(
        &self,
        turn: &LiveTurn,
        answer: &str,
        blocks: &[Block],
        tx: &UnboundedSender<Event>,
    ) {
        turn.cancel.trip();
        self.halt(turn, answer, blocks, tx).await;
    }

    /// Run the whole answer pipeline. Every stage degrades instead of failing the request.
    pub async fn ask(self: &Arc<Self>, req: AskRequest, tx: UnboundedSender<Event>) {
        let cfg = self.cfg.get();
        let started = Instant::now();
        let chat_id = req.id.clone();
        let query = req.query.clone();
        let cancel = self.runs.begin(&chat_id);
        let _guard = RunGuard {
            runs: self.runs.clone(),
            chat_id: chat_id.clone(),
        };
        // The client generates the chat id, so the row may not exist yet.
        if let Err(e) = self.db.create_chat(&chat_id, &cfg.now()).await {
            tracing::warn!("could not create chat row: {e}");
        }

        let answer_model = if req.models.answer.is_empty() {
            req.model
                .clone()
                .unwrap_or_else(|| cfg.default_model.clone())
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

        // The question is stored before any model call, so a refresh already has the thread.
        let assistant_id = match self
            .db
            .open_turn(&chat_id, &query, &req.files, &answer_model, &cfg.now())
            .await
        {
            Ok((_user, assistant)) => assistant,
            Err(e) => {
                tracing::warn!("could not store thread: {e}");
                String::new()
            }
        };
        let turn = LiveTurn {
            chat_id: chat_id.clone(),
            assistant_id,
            model: answer_model.clone(),
            started,
            cancel,
        };
        Self::emit(&tx, Event::Id(chat_id.clone()));
        let mut answer = String::new();
        let mut blocks: Vec<Block> = Vec::new();
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }

        let answer_llm = match llm::resolve_chat(&cfg, &self.http, &answer_model) {
            Ok(l) => l,
            Err(e) => {
                let msg = e.to_string();
                if !self.halt(&turn, &answer, &blocks, &tx).await {
                    self.checkpoint(&turn, &answer, &blocks, "error", &msg)
                        .await;
                    Self::emit(&tx, Event::Close(Some(msg)));
                }
                return;
            }
        };
        let light_llm = llm::resolve_chat(&cfg, &self.http, &light_model)
            .unwrap_or_else(|_| answer_llm.clone());
        let rerank_llm = llm::resolve_chat(&cfg, &self.http, &rerank_model)
            .unwrap_or_else(|_| light_llm.clone());

        // 1. classify the query
        let mut cls = match cut(&turn.cancel, &tx, self.classify(&light_llm, &req)).await {
            Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
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
        // A follow-up such as "翻譯做中文" acts on the previous answer. Resolve that
        // before widgets and search so the previous text is what gets translated.
        let mut search_query = req.query.clone();
        let mut from_history = false;
        if !req.message_history.is_empty() {
            if transform_only(&req.query) {
                from_history = true;
            } else {
                match cut(&turn.cancel, &tx, self.rewrite_followup(&light_llm, &req)).await {
                    Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
                    Ok(Some(plan)) => {
                        if !plan.question.is_empty() {
                            search_query = plan.question;
                        }
                        from_history = !plan.search;
                    }
                    Ok(None) => {}
                }
            }
        }
        if from_history {
            cls.need_web_search = false;
            cls.needs_image_results = false;
            cls.needs_video_results = false;
        }

        // 2. widgets, then the browser location when the question depends on where the user is.
        // Attached files are about the file, so they do not prompt for a place.
        // "what's the weather" needs a place even when the widget model forgets to ask.
        // The clock is resolved before classification is sent, so a pure time question
        // can turn search off with the flag the UI reads.
        let mut here = if req.files.is_empty() && crate::widgets::query_needs_place(&req.query) {
            match cut(&turn.cancel, &tx, self.locate(&turn.chat_id, &tx, true)).await {
                Ok(place) => place,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            }
        } else {
            None
        };
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }
        let mut choice = if req.files.is_empty() && !from_history {
            match cut(&turn.cancel, &tx, self.choose_widgets(&light_llm, &req)).await {
                Ok(c) => c,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            }
        } else {
            WidgetChoice::default()
        };
        if req.files.is_empty() && !from_history {
            // "time in Tokyo" is that city, not a request for the browser's location.
            if crate::widgets::clock_place(&req.query).is_some_and(|place| !place.is_empty()) {
                choice.needs_location = false;
            }
            crate::widgets::bind_clock(&mut choice.requested, &req.query);
        }
        let precise = crate::widgets::location_request(
            &choice.requested,
            choice.needs_location,
            &req.disabled_widgets,
        );
        if here.is_none() {
            if let Some(precise) = precise {
                here = match cut(&turn.cancel, &tx, self.locate(&turn.chat_id, &tx, precise)).await
                {
                    Ok(place) => place,
                    Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
                };
            }
        }
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }
        let located = here.as_ref().map(place_line).unwrap_or_default();
        let widget_blocks = match cut(
            &turn.cancel,
            &tx,
            self.widgets.render(
                &choice.requested,
                &req.query,
                &req.disabled_widgets,
                here.as_ref(),
            ),
        )
        .await
        {
            Ok(found) => found,
            Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
        };
        let clocked = crate::widgets::clock_facts(&widget_blocks);
        let converted = crate::widgets::currency_facts(&widget_blocks);
        if !clocked.is_empty() && crate::widgets::pure_clock(&req.query, &choice.requested) {
            cls.need_web_search = false;
            cls.needs_image_results = false;
            cls.needs_video_results = false;
        }
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
        self.checkpoint(&turn, &answer, &blocks, "search", "").await;
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }
        for b in widget_blocks.iter() {
            blocks.push(b.clone());
            Self::emit_block(&tx, b.clone());
        }
        if !widget_blocks.is_empty() {
            self.checkpoint(&turn, &answer, &blocks, "search", "").await;
        }
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }

        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }

        // 4. documents uploaded for this chat take priority over the web
        let document_context = if !req.files.is_empty() {
            match cut(
                &turn.cancel,
                &tx,
                self.document_context(&req, &search_query),
            )
            .await
            {
                Ok(ctx) => ctx,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            }
        } else {
            None
        };
        let pictures = if req.files.is_empty() {
            Vec::new()
        } else {
            self.uploads
                .images_for(&req.files)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("could not load attached images: {e}");
                    Vec::new()
                })
        };
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }

        // A photo question such as "who?" is not a search query. Ask the vision
        // model for one, then run web and image search. Uploaded documents stay
        // on the document and do not hit the web.
        let photo_lookup = if document_context.is_none()
            && !pictures.is_empty()
            && photo_needs_lookup(&cls, &search_query, &req.query)
        {
            match cut(
                &turn.cancel,
                &tx,
                self.image_lookup_query(&answer_llm, &req.query, &pictures),
            )
            .await
            {
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
                Ok(q) => q,
            }
        } else {
            None
        };
        if let Some(q) = &photo_lookup {
            search_query = q.clone();
        }
        let run_search = document_context.is_none()
            && if pictures.is_empty() {
                cls.need_web_search
            } else {
                photo_lookup.is_some()
            };

        // 5. search. The light model decides the queries first, then runs one or more rounds.
        let mut web = SearchResponse::default();
        let mut news = SearchResponse::default();
        let mut images = SearchResponse::default();
        let mut videos = SearchResponse::default();
        let depth = Depth::parse(&req.depth);
        let mut search_queries: Vec<String> = Vec::new();
        let mut thinking = String::new();
        let mut mind = Mind {
            tx: &tx,
            text: &mut thinking,
        };
        if run_search {
            // Clone the flag so the research future can borrow `turn` while `cut` waits on stop.
            let stop = turn.cancel.clone();
            let (results, queries) = match cut(
                &stop,
                &tx,
                self.research(
                    &light_llm,
                    &req,
                    &search_query,
                    depth,
                    &located,
                    &tx,
                    &mut blocks,
                    &turn,
                    &mut mind,
                ),
            )
            .await
            {
                Ok(found) => found,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            };
            search_queries = queries;
            web.results = results;
            if self.halt(&turn, &answer, &blocks, &tx).await {
                return;
            }
            let mut base = self.base_query(&req, &search_query);
            if cfg.enable_fresh_search
                && req.filters.freshness.as_deref().unwrap_or("noLimit") != "noLimit"
            {
                news = match cut(
                    &turn.cancel,
                    &tx,
                    self.search.run(SearchKind::News, &mut base.clone()),
                )
                .await
                {
                    Ok(found) => found,
                    Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
                };
            }
            if (cls.needs_image_results || photo_lookup.is_some()) && cfg.enable_image_search {
                images = match cut(
                    &turn.cancel,
                    &tx,
                    self.search.run(SearchKind::Images, &mut base.clone()),
                )
                .await
                {
                    Ok(found) => found,
                    Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
                };
            }
            if cls.needs_video_results && cfg.enable_video_search {
                videos = match cut(
                    &turn.cancel,
                    &tx,
                    self.search.run(SearchKind::Videos, &mut base),
                )
                .await
                {
                    Ok(found) => found,
                    Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
                };
            }
            if self.halt(&turn, &answer, &blocks, &tx).await {
                return;
            }
        }

        // 6. The fast model drops sources that cannot help. The rerank model then orders what remains.
        let mut candidates = dedupe(web.results, news.results);
        if document_context.is_none() && cls.need_web_search && candidates.len() > 3 {
            let note = Block::research(
                depth.as_str(),
                depth.rounds(),
                depth.rounds(),
                &[],
                "Filtering sources",
                "filter",
            );
            blocks.push(note.clone());
            Self::emit_block(&tx, note);
            self.checkpoint(&turn, &answer, &blocks, "search", "").await;
            if self.halt(&turn, &answer, &blocks, &tx).await {
                return;
            }
            candidates = match cut(
                &turn.cancel,
                &tx,
                filter_sources(&light_llm, &search_query, candidates, &mut mind),
            )
            .await
            {
                Ok(kept) => kept,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            };
            if self.halt(&turn, &answer, &blocks, &tx).await {
                return;
            }
        }
        if cfg.enable_reranker && candidates.len() > 1 {
            let note = Block::research(
                depth.as_str(),
                depth.rounds(),
                depth.rounds(),
                &[],
                "Reranking sources",
                "rerank",
            );
            blocks.push(note.clone());
            Self::emit_block(&tx, note);
            self.checkpoint(&turn, &answer, &blocks, "search", "").await;
            if self.halt(&turn, &answer, &blocks, &tx).await {
                return;
            }
            candidates = match cut(
                &turn.cancel,
                &tx,
                self.rerank(&rerank_llm, &search_query, candidates, &mut mind),
            )
            .await
            {
                Ok(ranked) => ranked,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            };
            if self.halt(&turn, &answer, &blocks, &tx).await {
                return;
            }
        }
        candidates.truncate(cfg.rerank_limit);

        let references = if from_history {
            "No new web search. Use the previous answer below as the text to work from.".into()
        } else if candidates.is_empty() && !clocked.is_empty() {
            "No web search. The Clock line is the current time. State that time.".into()
        } else {
            build_references(&candidates)
        };
        if !from_history {
            let ref_block = Block::references(candidates.clone());
            blocks.push(ref_block.clone());
            Self::emit_block(&tx, ref_block);
        }
        self.checkpoint(&turn, &answer, &blocks, "read", "").await;
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }

        let related = Block::videos(videos.results.clone());
        if !videos.results.is_empty() {
            blocks.push(related.clone());
            Self::emit_block(&tx, related);
        }

        // 7. images
        if !images.results.is_empty() {
            let picked = match cut(
                &turn.cancel,
                &tx,
                self.select_images(&light_llm, &search_query, images.results.clone()),
            )
            .await
            {
                Ok(picked) => picked,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            };
            let block = Block::images(picked);
            blocks.push(block.clone());
            Self::emit_block(&tx, block);
            self.checkpoint(&turn, &answer, &blocks, "read", "").await;
        }
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }

        // 8. the answer itself. Every path gets today's date, including images,
        // documents, and code, where the template has no {date} placeholder.
        // A report is only worth outlining for a cited web answer: documents and photos
        // keep the single-pass writer.
        let long_form = req.mode.trim().eq_ignore_ascii_case("discussion")
            && document_context.is_none()
            && pictures.is_empty();
        let system = with_place(
            with_today(if !pictures.is_empty() && document_context.is_none() {
                if candidates.is_empty() {
                    prompts::IMAGE_READ_SYSTEM.to_string()
                } else {
                    prompts::IMAGE_READ_SOURCES_SYSTEM.to_string()
                }
            } else {
                prompts::fill(
                    if document_context.is_some() {
                        prompts::DOCUMENT_SYSTEM
                    } else if cls.r#type == "coding" {
                        prompts::CODING_SYSTEM
                    } else {
                        prompts::ANSWER_SYSTEM
                    },
                    &[("date", &cfg.today()), ("year", &cfg.year())],
                )
            }),
            &located,
        );
        let user_prompt = if let Some(chunks) = &document_context {
            prompts::fill(
                prompts::DOCUMENT_USER,
                &[("chunks", chunks), ("query", &req.query)],
            )
        } else if !pictures.is_empty() {
            if candidates.is_empty() {
                format!("User Question: {}", req.query)
            } else {
                prompts::fill(
                    prompts::IMAGE_READ_SOURCES_USER,
                    &[("references", &references), ("query", &req.query)],
                )
            }
        } else {
            prompts::fill(
                prompts::ANSWER_USER,
                &[
                    ("references", &references),
                    ("query", &req.query),
                    ("prior", &prior_block(&req.message_history)),
                ],
            )
        };
        let facts = joined_facts(&[&clocked, &converted]);
        let user_prompt = with_clock(user_prompt, &facts);

        let mut history: Vec<Msg> = Vec::new();
        for m in req.message_history.iter().rev().take(10).rev() {
            if m.role == "user" || m.role == "assistant" {
                history.push(m.clone());
            }
        }
        let temperature = Some(cfg.default_temperature);

        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }
        if !mind.text.is_empty() {
            mind.push("\n\n");
        }

        // Long-form: the light model plans the sections, then the answer model writes them
        // one at a time. Each call sees the whole reference list, so a 2 000-word report
        // never leaves the model's working memory the way one long generation would.
        let outline = if long_form {
            match cut(
                &turn.cancel,
                &tx,
                self.plan_sections(&light_llm, &req.query, &candidates),
            )
            .await
            {
                Ok(sections) => sections,
                Err(_) => return self.give_up(&turn, &answer, &blocks, &tx).await,
            }
        } else {
            Vec::new()
        };
        if !outline.is_empty() {
            let note = Block::research(
                depth.as_str(),
                1,
                outline.len(),
                &outline,
                "Report outline",
                "write",
            );
            blocks.push(note.clone());
            Self::emit_block(&tx, note);
            self.checkpoint(&turn, &answer, &blocks, "write", "").await;

            let system = with_place(
                with_today(prompts::fill(
                    prompts::DISCUSSION_SYSTEM,
                    &[("date", &cfg.today()), ("year", &cfg.year())],
                )),
                &located,
            );
            let contents = outline
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n");
            let last = outline.len() - 1;
            for (i, section) in outline.iter().enumerate() {
                if self.halt(&turn, &answer, &blocks, &tx).await {
                    return;
                }
                if i > 0 {
                    // The heading is written here so the model never has to guess it.
                    let heading = format!("\n\n## {section}\n\n");
                    answer.push_str(&heading);
                    Self::emit(&tx, Event::AnswerChunk(heading));
                }
                let brief = if i == 0 {
                    "This is the opening: lead with the answer in the first two sentences."
                } else if i == last {
                    "This is the close: what follows from it, or what is still unknown. Do not recap the sections above."
                } else {
                    "Stay inside this section — the ones before and after are written separately."
                };
                let user = with_clock(
                    prompts::fill(
                        prompts::DISCUSSION_SECTION_USER,
                        &[
                            ("query", &req.query),
                            ("outline", &contents),
                            ("section", section),
                            ("brief", brief),
                            ("references", &references),
                        ],
                    ),
                    &facts,
                );
                let request = ChatRequest {
                    messages: std::iter::once(msg("system", &system))
                        .chain(history.iter().cloned())
                        .chain(std::iter::once(msg("user", user)))
                        .collect(),
                    temperature,
                    max_tokens: None,
                    json_mode: false,
                };
                if let Err(e) = self
                    .stream_into(
                        &answer_llm,
                        request,
                        &turn,
                        &mut blocks,
                        &mut mind,
                        &mut answer,
                        &tx,
                    )
                    .await
                {
                    if answer.trim().is_empty() {
                        if !self.halt(&turn, &answer, &blocks, &tx).await {
                            self.checkpoint(&turn, &answer, &blocks, "error", &e).await;
                            Self::emit(&tx, Event::Close(Some(e)));
                        }
                        return;
                    }
                    // A section failed mid-report. Ship what is written instead of nothing.
                    tracing::warn!(section = %section, "report section failed: {e}");
                    break;
                }
            }
        } else {
            // 8b. the short answer: one generation over the whole reference list.
            let mut ask = msg("user", &user_prompt);
            ask.images = pictures
                .into_iter()
                .map(|(mime, data)| crate::llm::InlineImage { mime, data })
                .collect();
            let request = ChatRequest {
                messages: std::iter::once(msg("system", &system))
                    .chain(history)
                    .chain(std::iter::once(ask))
                    .collect(),
                temperature,
                max_tokens: None,
                json_mode: false,
            };
            if let Err(e) = self
                .stream_into(
                    &answer_llm,
                    request,
                    &turn,
                    &mut blocks,
                    &mut mind,
                    &mut answer,
                    &tx,
                )
                .await
            {
                if !self.halt(&turn, &answer, &blocks, &tx).await {
                    self.checkpoint(&turn, &answer, &blocks, "error", &e).await;
                    Self::emit(&tx, Event::Close(Some(e)));
                }
                return;
            }
        }
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }

        store_thought(&mut blocks, mind.text);
        blocks.push(Block::message(&answer));

        // 9. follow-up questions shown under the answer
        let Ok(suggestions) = cut(
            &turn.cancel,
            &tx,
            self.suggestions(&light_llm, &req.query, &answer),
        )
        .await
        else {
            return self.give_up(&turn, &answer, &blocks, &tx).await;
        };
        if self.halt(&turn, &answer, &blocks, &tx).await {
            return;
        }
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
                "search_query": search_queries.first().cloned().unwrap_or(search_query.clone()),
                "search_queries": search_queries,
                "depth": depth.as_str(),
            })),
            extra: None,
        });

        // The question was stored when the ask began. This writes the finished answer onto that row.
        // If that insert failed, fall back to appending the pair now.
        if turn.assistant_id.is_empty() {
            let now = cfg.now();
            if let Err(e) = self
                .db
                .save_message(
                    &chat_id,
                    &ChatMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        role: "user".into(),
                        content: req.query.clone(),
                        blocks: vec![],
                        files: req.files.clone(),
                        created_at: now.clone(),
                        status: "done".into(),
                        error: String::new(),
                        model: String::new(),
                        elapsed_ms: None,
                    },
                )
                .await
            {
                tracing::warn!("failed to save user message: {e}");
            }
            if let Err(e) = self
                .db
                .save_message(
                    &chat_id,
                    &ChatMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        role: "assistant".into(),
                        content: answer.clone(),
                        blocks,
                        files: vec![],
                        created_at: now,
                        status: "done".into(),
                        error: String::new(),
                        model: answer_model.clone(),
                        elapsed_ms: Some(started.elapsed().as_millis() as i64),
                    },
                )
                .await
            {
                tracing::warn!("failed to save assistant message: {e}");
            }
        } else {
            self.checkpoint(&turn, &answer, &blocks, "done", "").await;
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

    /// Stream one completion into `answer`, checkpointing as it goes. `Err` only when the
    /// model failed before writing anything: a failure part way through keeps the text that
    /// arrived, and a stop is not an error.
    async fn stream_into(
        &self,
        llm: &Llm,
        request: ChatRequest,
        turn: &LiveTurn,
        blocks: &mut Vec<Block>,
        mind: &mut Mind<'_>,
        answer: &mut String,
        tx: &UnboundedSender<Event>,
    ) -> Result<(), String> {
        let mut rx = match llm.stream(request).await {
            Ok(rx) => rx,
            Err(e) => return Err(format!("answer model failed: {e}")),
        };
        turn.cancel.watch_stream(rx.abort_handle());
        let mut last_flush: Option<Instant> = None;
        let mut guard = LoopGuard::default();
        // Did this completion write any answer text? A trace that never becomes an answer is
        // a failure, not a finished turn — without this the thread saves as "done" and empty.
        let mut body = false;
        let mut truncated = false;
        let mut looped: Option<String> = None;
        loop {
            let delta = tokio::select! {
                biased;
                _ = turn.cancel.wait() => None,
                delta = rx.recv() => delta,
            };
            let Some(delta) = delta else {
                break;
            };
            let mut wrote = false;
            match delta {
                Delta::Text(t) => {
                    answer.push_str(&t);
                    Self::emit(tx, Event::AnswerChunk(t));
                    body = true;
                    wrote = true;
                }
                Delta::Thought(t) => {
                    if let Some(line) = guard.feed(&t) {
                        looped = Some(line);
                        break;
                    }
                    mind.push(t);
                    wrote = true;
                }
                Delta::Truncated => truncated = true,
                Delta::Error(e) => {
                    tracing::warn!("answer stream error: {e}");
                    turn.cancel.clear_stream();
                    drop(rx);
                    if answer.is_empty() && !turn.cancel.load(Ordering::Acquire) {
                        return Err(e);
                    }
                    return Ok(());
                }
            }
            let due = last_flush
                .map(|t| t.elapsed() >= Duration::from_millis(400))
                .unwrap_or(true);
            if wrote && due {
                store_thought(blocks, mind.text);
                self.checkpoint(turn, answer, blocks, "write", "").await;
                last_flush = Some(Instant::now());
            }
        }
        turn.cancel.clear_stream();
        drop(rx);
        if let Some(line) = looped {
            if line.is_empty() {
                tracing::warn!(model = %llm.model, "reasoning used the character budget and wrote no answer");
            } else {
                tracing::warn!(
                    model = %llm.model,
                    line = %line,
                    "reasoning loop: the model repeated itself instead of answering"
                );
            }
            // Keep the thinking that came before the loop, drop the cycles after it.
            if !line.is_empty() {
                mind.cut_loop(&line);
            }
            store_thought(blocks, mind.text);
            return if body {
                Ok(())
            } else if line.is_empty() {
                Err(BUDGET_USED.to_string())
            } else {
                Err(LOOPING.to_string())
            };
        }
        if !body && !turn.cancel.load(Ordering::Acquire) {
            return Err(if truncated {
                BUDGET_USED.to_string()
            } else {
                NO_TEXT.to_string()
            });
        }
        Ok(())
    }

    /// The section list for a long-form report. Fewer than three sections is not a report,
    /// so an empty list means "write the normal short answer instead".
    async fn plan_sections(&self, llm: &Llm, query: &str, results: &[SearchResult]) -> Vec<String> {
        let user = prompts::fill(
            prompts::DISCUSSION_OUTLINE_USER,
            &[("query", query), ("findings", &numbered(results))],
        );
        let Ok(v) = llm
            .json_call(prompts::DISCUSSION_OUTLINE_SYSTEM, &user, None)
            .await
        else {
            return Vec::new();
        };
        let out: Vec<String> = v["sections"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                s.as_str()
                    .map(|s| s.trim().trim_start_matches('#').trim().to_string())
            })
            .filter(|s| !s.is_empty())
            .take(6)
            .collect();
        if out.len() < 3 {
            return Vec::new();
        }
        out
    }

    fn base_query(&self, req: &AskRequest, search_query: &str) -> SearchQuery {
        let cfg = self.cfg.get();
        SearchQuery {
            query: search_query.to_string(),
            kind: SearchKind::Web,
            num_results: cfg.num_web_results,
            country: Some(
                req.filters
                    .country
                    .clone()
                    .unwrap_or_else(|| cfg.country.clone()),
            ),
            language: req.filters.language.clone(),
            freshness: Some(
                req.filters
                    .freshness
                    .clone()
                    .unwrap_or_else(|| cfg.freshness.clone()),
            ),
            safe_search: cfg.safe_search,
            site: req.filters.perplexity_site.clone(),
            page: None,
        }
    }

    async fn classify(&self, llm: &Llm, req: &AskRequest) -> anyhow::Result<Classification> {
        let history = history_text(&req.message_history);
        let today = self.cfg.get().today();
        let user = prompts::fill(
            prompts::CLASSIFY_USER,
            &[
                ("date", &today),
                ("query", &req.query),
                ("history", &history),
            ],
        );
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
        place: &str,
        tx: &UnboundedSender<Event>,
        blocks: &mut Vec<Block>,
        turn: &LiveTurn,
        mind: &mut Mind<'_>,
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
            if turn.cancel.load(Ordering::Relaxed) {
                break;
            }
            let found = findings_brief(&merge_buckets(&buckets));
            let (queries, note) = plan_queries(
                llm, question, &history, depth, round, &asked, &found, place, mind,
            )
            .await;
            let queries = if queries.is_empty() && round == 1 {
                vec![question.to_string()]
            } else {
                queries
            };
            if queries.is_empty() {
                break;
            }

            let block = Block::research(
                depth.as_str(),
                round,
                depth.rounds(),
                &queries,
                &note,
                "search",
            );
            blocks.push(block.clone());
            Self::emit_block(tx, block);
            store_thought(blocks, mind.text);
            self.checkpoint(turn, "", blocks, "search", "").await;
            if turn.cancel.load(Ordering::Relaxed) {
                break;
            }

            for q in &queries {
                if turn.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let mut base = self.base_query(req, q);
                base.num_results = per_query;
                let res = self.search.run(SearchKind::Web, &mut base).await;
                buckets.push(res.results);
                asked.push(q.clone());
            }
        }

        // Keep a mix of every round. A plain truncate would keep only the first query's hits.
        // Leave room above the citation cap so filtering can drop weak hits and still fill it.
        let mut results = merge_buckets(&buckets);
        let pool = (cfg.rerank_limit + cfg.rerank_limit / 2).max(24);
        results.truncate(pool);
        (results, asked)
    }

    /// One search query for an attached photo, or nothing when the picture
    /// itself answers the question.
    async fn image_lookup_query(
        &self,
        llm: &Llm,
        question: &str,
        pictures: &[(String, Vec<u8>)],
    ) -> Option<String> {
        let mut user = msg(
            "user",
            prompts::fill(prompts::IMAGE_LOOKUP_USER, &[("query", question)]),
        );
        user.images = pictures
            .iter()
            .map(|(mime, data)| crate::llm::InlineImage {
                mime: mime.clone(),
                data: data.clone(),
            })
            .collect();
        let raw = llm
            .complete(ChatRequest {
                messages: vec![msg("system", prompts::IMAGE_LOOKUP_SYSTEM), user],
                temperature: Some(0.0),
                max_tokens: Some(1024),
                json_mode: false,
            })
            .await
            .ok()?;
        lookup_query_from(&raw)
    }

    async fn rewrite_followup(&self, llm: &Llm, req: &AskRequest) -> Option<FollowupPlan> {
        let history = history_text(&req.message_history);
        let user = prompts::fill(
            prompts::DETERMINE_FOLLOWUP_USER,
            &[
                ("date", &self.cfg.get().today()),
                ("history", &history),
                ("query", &req.query),
            ],
        );
        let v = llm
            .json_call(prompts::DETERMINE_FOLLOWUP_SYSTEM, &user, None)
            .await
            .ok()?;
        if !v["needs_follow_up"].as_bool().unwrap_or(true) {
            return None;
        }
        let question = v["question"]
            .as_str()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Some(FollowupPlan {
            question,
            search: v["search"].as_bool().unwrap_or(true),
        })
    }

    async fn choose_widgets(&self, llm: &Llm, req: &AskRequest) -> WidgetChoice {
        let cfg = self.cfg.get();
        let user = prompts::fill(
            prompts::CHOOSE_WIDGETS_USER,
            &[("query", &req.query), ("date", &cfg.today())],
        );
        let Ok(v) = llm
            .json_call(prompts::CHOOSE_WIDGETS_SYSTEM, &user, None)
            .await
        else {
            return WidgetChoice::default();
        };
        WidgetChoice {
            requested: serde_json::from_value(v["widgets"].clone()).unwrap_or_default(),
            needs_location: v["needs_location"].as_bool().unwrap_or(false),
        }
    }

    /// Ask the open page for a place, then turn coordinates into a name.
    async fn locate(
        &self,
        chat_id: &str,
        tx: &UnboundedSender<Event>,
        precise: bool,
    ) -> Option<crate::widgets::Here> {
        let (rx, _guard) = self.locations.expect(chat_id);
        Self::emit(tx, Event::NeedLocation { precise });
        let fix = tokio::select! {
            got = rx => got.ok(),
            _ = tokio::time::sleep(Duration::from_secs(60)) => None,
        }?;
        self.here_from(fix).await
    }

    async fn here_from(&self, fix: GeoFix) -> Option<crate::widgets::Here> {
        let browser_tz = valid_tz(&fix.timezone);
        let coords = if fix.denied {
            None
        } else {
            crate::widgets::Here {
                label: String::new(),
                latitude: fix.latitude,
                longitude: fix.longitude,
                timezone: String::new(),
            }
            .coords()
        };
        if coords.is_none() && browser_tz.is_empty() {
            return None;
        }
        let (label, reverse_tz) = if let Some((lat, lon)) = coords {
            crate::widgets::weather::reverse_label(&self.http, lat, lon, browser_tz.is_empty())
                .await
                .unwrap_or_default()
        } else {
            (String::new(), String::new())
        };
        let timezone = if browser_tz.is_empty() {
            valid_tz(&reverse_tz)
        } else {
            browser_tz
        };
        let (latitude, longitude) = coords.unwrap_or((f64::NAN, f64::NAN));
        if label.is_empty() && timezone.is_empty() && coords.is_none() {
            return None;
        }
        Some(crate::widgets::Here {
            label,
            latitude,
            longitude,
            timezone,
        })
    }

    async fn rerank(
        &self,
        llm: &Llm,
        query: &str,
        results: Vec<SearchResult>,
        mind: &mut Mind<'_>,
    ) -> Vec<SearchResult> {
        let cfg = self.cfg.get();
        let user = prompts::fill(
            prompts::RERANK_USER,
            &[("query", query), ("results", &numbered(&results))],
        );
        let system = prompts::fill(
            prompts::RERANK_SYSTEM,
            &[("num", &cfg.rerank_limit.to_string())],
        );
        let v = match llm
            .json_call_thinking(&system, &user, None, |chunk| mind.push(chunk))
            .await
        {
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

    async fn select_images(
        &self,
        llm: &Llm,
        query: &str,
        results: Vec<SearchResult>,
    ) -> Vec<SearchResult> {
        let _cfg = self.cfg.get();
        if results.len() <= 6 {
            return results;
        }
        let list = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "[{i}] {} — {}",
                    r.title,
                    r.image.clone().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let user = prompts::fill(
            prompts::IMAGE_SELECTION_USER,
            &[("query", query), ("images", &list)],
        );
        let Ok(v) = llm
            .json_call(prompts::IMAGE_SELECTION_SYSTEM, &user, None)
            .await
        else {
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
        let Ok(v) = llm
            .json_call(prompts::DISCOVER_QUESTIONS_SYSTEM, &user, None)
            .await
        else {
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

fn with_place(body: String, line: &str) -> String {
    if line.is_empty() {
        body
    } else {
        format!("{line}\n\n{body}")
    }
}

fn place_line(here: &crate::widgets::Here) -> String {
    let tz = valid_tz(&here.timezone);
    let clock = tz.parse::<chrono_tz::Tz>().ok().map(|zone| {
        chrono::Local::now()
            .with_timezone(&zone)
            .format("%A, %d %B %Y, %H:%M")
            .to_string()
    });
    let clock = clock
        .map(|c| format!(" Local time there is {c}."))
        .unwrap_or_default();
    if let Some((lat, lon)) = here.coords() {
        let name = if here.label.trim().is_empty() {
            format!("{lat:.4}, {lon:.4}")
        } else {
            here.label.trim().to_string()
        };
        let zone = if tz.is_empty() {
            String::new()
        } else {
            format!(", {tz}")
        };
        format!(
            "The user is in {name} ({lat:.4}, {lon:.4}{zone}). Use this when the question depends on where they are.{clock}"
        )
    } else if !tz.is_empty() {
        format!("The user's timezone is {tz}. Use it for the local time.{clock}")
    } else {
        String::new()
    }
}

fn valid_tz(name: &str) -> String {
    let name = name.trim();
    if name.len() > 64 {
        return String::new();
    }
    if name.parse::<chrono_tz::Tz>().is_ok() {
        name.to_string()
    } else {
        String::new()
    }
}

/// The converted clock sits above the question so the model states that time.
fn with_clock(body: String, facts: &str) -> String {
    if facts.is_empty() {
        body
    } else {
        format!("{facts}\n\n{body}")
    }
}

/// Widget facts the answer must copy exactly: the converted clock, the converted money.
fn joined_facts(parts: &[&String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Date skill. Answer prompts that already state the date are left as they are.
fn with_today(body: String) -> String {
    if body.contains("Today is ") {
        body
    } else {
        format!(
            "Today is {}. This is the current local date and time; use it when the question depends on today.\n\n{body}",
            crate::config::Config::today_line()
        )
    }
}

/// A photo of a person or place should still be searched. "who?" is the
/// common case the classifier files as too short to need the web.
fn photo_needs_lookup(cls: &Classification, rewritten: &str, raw: &str) -> bool {
    cls.need_web_search
        || cls.needs_image_results
        || identity_question(rewritten)
        || identity_question(raw)
}

fn identity_question(query: &str) -> bool {
    let q = query
        .trim()
        .trim_end_matches(['?', '!', '.', '。', '？'])
        .to_ascii_lowercase();
    matches!(
        q.as_str(),
        "who"
            | "who is this"
            | "who is that"
            | "who is he"
            | "who is she"
            | "who's this"
            | "who's that"
            | "whos this"
            | "identify"
            | "identify this"
            | "name this"
            | "what is this"
            | "what's this"
            | "whats this"
            | "where is this"
            | "where is that"
    )
}

fn lookup_query_from(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let line = line
        .trim_matches(|c: char| matches!(c, '"' | '`' | '*'))
        .trim();
    let line = line
        .strip_prefix("Query:")
        .or_else(|| line.strip_prefix("query:"))
        .unwrap_or(line)
        .trim();
    let bare = line.trim_end_matches(['.', '!', '。']);
    if line.is_empty()
        || bare.eq_ignore_ascii_case("none")
        || bare.eq_ignore_ascii_case("n/a")
        || line.chars().count() > 180
    {
        return None;
    }
    Some(line.to_string())
}

fn msg(role: &str, content: impl Into<String>) -> Msg {
    Msg {
        role: role.into(),
        content: content.into(),
        images: Vec::new(),
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
    place: &str,
    mind: &mut Mind<'_>,
) -> (Vec<String>, String) {
    let asked_text = if asked.is_empty() {
        "(none yet)".to_string()
    } else {
        asked
            .iter()
            .map(|q| format!("- {q}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let system = prompts::fill(
        prompts::PLAN_SEARCH_SYSTEM,
        &[
            ("date", &crate::config::Config::today_line()),
            ("n", &depth.per_round().to_string()),
        ],
    );
    let place = if place.is_empty() {
        String::new()
    } else {
        format!("{place}\n\n")
    };
    let user = prompts::fill(
        prompts::PLAN_SEARCH_USER,
        &[
            ("place", &place),
            ("query", question),
            ("history", history),
            ("round", &round.to_string()),
            ("rounds", &depth.rounds().to_string()),
            ("asked", &asked_text),
            ("found", found),
        ],
    );
    let v = match llm
        .json_call_thinking(&system, &user, Some(500), |chunk| mind.push(chunk))
        .await
    {
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
async fn filter_sources(
    llm: &Llm,
    query: &str,
    results: Vec<SearchResult>,
    mind: &mut Mind<'_>,
) -> Vec<SearchResult> {
    let user = prompts::fill(
        prompts::FILTER_SOURCES_USER,
        &[("query", query), ("results", &numbered(&results))],
    );
    let v = match llm
        .json_call_thinking(prompts::FILTER_SOURCES_SYSTEM, &user, Some(500), |chunk| {
            mind.push(chunk)
        })
        .await
    {
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
        let Some(i) = idx
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(|n| n.saturating_sub(1))
        else {
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
    q.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Keep the first `limit` new search strings. Drops blanks, repeats, and anything already run.
fn take_queries(raw: &serde_json::Value, limit: usize, already: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = already
        .iter()
        .map(|q| query_key(q))
        .filter(|k| !k.is_empty())
        .collect();
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

struct FollowupPlan {
    question: String,
    /// False when the user wants the previous answer rewritten rather than new facts.
    search: bool,
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

/// The last assistant reply, placed in the answer prompt so a follow-up can
/// translate or rewrite it without the user pasting it again.
fn prior_block(history: &[Msg]) -> String {
    let Some(text) = history
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && !m.content.trim().is_empty())
    else {
        return String::new();
    };
    let mut body: String = text.content.chars().take(12_000).collect();
    if text.content.chars().count() > body.chars().count() {
        body.push('…');
    }
    format!(
        "\nPrevious answer in this conversation:\n{body}\n\nThe user can see that answer. If they asked to translate, summarize, shorten, rewrite, or explain it, produce that result from the text above. Keep its citation numbers when you translate. If they asked a new question, answer the new question and use the previous answer as context.\n"
    )
}

/// A short request whose only job is to transform the previous answer.
/// "翻譯做中文" and "translate this into chinese" match. A request that names
/// a new subject, such as "translate the EU AI Act", does not.
fn transform_only(query: &str) -> bool {
    let q = query
        .trim()
        .trim_end_matches(['?', '!', '.', '。', '？', '！'])
        .to_lowercase();
    let n = q.chars().count();
    if !(2..=48).contains(&n) {
        return false;
    }
    const CUES: &[&str] = &[
        "translate",
        "translation",
        "summarize",
        "summarise",
        "summary",
        "shorten",
        "rewrite",
        "rephrase",
        "paraphrase",
        "翻譯",
        "翻译",
        "總結",
        "总结",
        "縮短",
        "缩短",
        "改寫",
        "改写",
        "重寫",
        "重写",
        "翻成",
        "譯成",
        "译成",
        "中譯",
        "中译",
    ];
    const SOFT: &[&str] = &[
        "make it shorter",
        "make this shorter",
        "shorter version",
        "in chinese",
        "into chinese",
        "to chinese",
        "in english",
        "into english",
        "to english",
        "用中文",
        "成中文",
        "做中文",
        "用英文",
        "成英文",
        "更短",
        "簡短一點",
        "简短一点",
        "解釋一下",
        "解释一下",
        "explain that",
        "explain this",
        "explain it",
    ];
    let has_cue = CUES.iter().any(|c| q.contains(c));
    if !has_cue && !SOFT.iter().any(|c| q.contains(c)) {
        return false;
    }
    let mut rest = q;
    let mut phrases: Vec<&str> = CUES
        .iter()
        .copied()
        .chain(SOFT.iter().copied())
        .chain(
            [
                "中文", "英文", "一下", "這個", "这个", "這段", "这段", "上面", "上文", "它", "請",
                "请", "幫我", "帮我", "把", "這", "这",
            ]
            .into_iter(),
        )
        .collect();
    phrases.sort_by_key(|s| std::cmp::Reverse(s.len()));
    phrases.dedup();
    for phrase in phrases {
        rest = rest.replace(phrase, " ");
    }
    const TOKENS: &[&str] = &[
        "this", "that", "it", "the", "a", "an", "to", "into", "in", "of", "above", "please", "me",
        "you", "can", "could", "for", "my",
    ];
    let leftover: String = rest
        .split(|c: char| c.is_whitespace() || c == ',' || c == '，' || c == '、')
        .filter(|word| !word.is_empty() && !TOKENS.contains(word))
        .collect();
    leftover.chars().count() <= 2
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
        return "(no search results — answer from what you know and say you could not verify)"
            .into();
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

pub async fn title_and_emoji(
    cfg: &Config,
    http: &reqwest::Client,
    db: &Db,
    chat_id: &str,
    query: &str,
) -> Result<(), LibraError> {
    let llm = llm::resolve_chat(cfg, http, cfg.light_model())?;
    let v = llm
        .json_call(
            prompts::QUERY_TITLE_SYSTEM,
            &prompts::fill(prompts::QUERY_TITLE_USER, &[("query", query)]),
            None,
        )
        .await
        .unwrap_or_else(|_| json!({ "title": query }));
    let generated = v["title"].as_str().unwrap_or(query);
    let title = title_in_query_language(query, generated);
    let emoji = v["emoji"].as_str().unwrap_or("🔍").to_string();
    db.rename_chat(chat_id, &title, &emoji).await?;
    Ok(())
}

/// A Chinese question should not be filed under an English title. When the
/// model translates anyway, the question itself is the title.
fn title_in_query_language(query: &str, title: &str) -> String {
    let title = title
        .trim()
        .trim_matches(|c| c == '"' || c == '“' || c == '”');
    if title.is_empty() || (uses_cjk(query) && !uses_cjk(title)) {
        return clip_title(query);
    }
    title.to_string()
}

fn uses_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        ('\u{3400}'..='\u{9FFF}').contains(&c)
            || ('\u{3040}'..='\u{30FF}').contains(&c)
            || ('\u{AC00}'..='\u{D7AF}').contains(&c)
    })
}

fn clip_title(query: &str) -> String {
    let flat = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for ch in flat.chars() {
        if out.chars().count() >= 24 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
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
    fn a_chinese_question_keeps_a_chinese_title() {
        assert_eq!(
            title_in_query_language("现在东京几点", "Current Tokyo Time"),
            "现在东京几点"
        );
        assert_eq!(
            title_in_query_language("特斯拉今天的股价是多少", "特斯拉今日股价"),
            "特斯拉今日股价"
        );
        assert_eq!(
            title_in_query_language("why is the sky blue", "Why the sky is blue"),
            "Why the sky is blue"
        );
        let long = "请用中文解释量子计算的基本原理以及它和经典计算的差别在哪里";
        assert!(title_in_query_language(long, "Quantum computing basics").ends_with('…'));
        assert!(uses_cjk(&title_in_query_language(
            long,
            "Quantum computing basics"
        )));
    }

    #[test]
    fn a_translation_follow_up_uses_the_previous_answer() {
        assert!(transform_only("翻譯做中文"));
        assert!(transform_only("翻译成中文"));
        assert!(transform_only("translate this into chinese"));
        assert!(transform_only("summarize that"));
        assert!(transform_only("用中文"));
        assert!(!transform_only("Apple Huxe reverse acqui-hire"));
        assert!(!transform_only("translate the EU AI Act"));
        assert!(!transform_only("中文新聞"));
        let history = vec![
            msg("user", "what did Apple do with Huxe?"),
            msg(
                "assistant",
                "Apple hired staff from Huxe and licensed its technology.",
            ),
        ];
        let block = prior_block(&history);
        assert!(block.contains("Apple hired staff from Huxe"));
        assert!(block.contains("produce that result"));
        assert!(prior_block(&[]).is_empty());
        let prompt = prompts::fill(
            prompts::ANSWER_USER,
            &[
                ("references", "No new web search."),
                ("query", "翻譯做中文"),
                ("prior", &block),
            ],
        );
        assert!(prompt.contains("翻譯做中文"));
        assert!(prompt.contains("Apple hired staff from Huxe"));
        assert!(!prompt.contains("{prior}"));
    }

    #[test]
    fn who_about_a_photo_is_a_lookup_and_a_color_question_is_not() {
        let quiet = Classification {
            r#type: "normal".into(),
            need_web_search: false,
            needs_image_results: false,
            needs_video_results: false,
            content_generation: None,
        };
        assert!(photo_needs_lookup(&quiet, "who?", "who?"));
        assert!(!photo_needs_lookup(
            &quiet,
            "what color is the jacket?",
            "what color is the jacket?"
        ));
        assert_eq!(
            lookup_query_from("thinking about the face\nVincent Wong actor"),
            Some("Vincent Wong actor".into())
        );
        assert_eq!(lookup_query_from("none"), None);
    }

    #[tokio::test]
    async fn browser_location_reaches_the_waiting_ask() {
        let locations = Locations::default();
        let (rx, _guard) = locations.expect("chat");
        locations.provide(
            "chat",
            GeoFix {
                latitude: 22.3,
                longitude: 114.17,
                timezone: "Asia/Hong_Kong".into(),
                denied: false,
            },
        );
        let fix = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .unwrap()
            .unwrap();
        assert!((fix.latitude - 22.3).abs() < 1e-6);
        assert_eq!(fix.timezone, "Asia/Hong_Kong");
    }

    #[tokio::test]
    async fn an_old_location_wait_does_not_drop_the_next_one() {
        let locations = Locations::default();
        let (_older, guard) = locations.expect("chat");
        let (rx, _next) = locations.expect("chat");
        drop(guard);
        locations.provide(
            "chat",
            GeoFix {
                latitude: 1.0,
                longitude: 2.0,
                timezone: String::new(),
                denied: false,
            },
        );
        let fix = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .unwrap()
            .unwrap();
        assert!((fix.latitude - 1.0).abs() < 1e-6);
    }

    #[test]
    fn place_line_uses_the_browser_fix() {
        let line = place_line(&crate::widgets::Here {
            label: "Hong Kong".into(),
            latitude: 22.3,
            longitude: 114.17,
            timezone: "Asia/Hong_Kong".into(),
        });
        assert!(line.contains("Hong Kong"), "{line}");
        assert!(line.contains("Asia/Hong_Kong"), "{line}");
        assert!(line.contains("22.3000"), "{line}");
        let clock = place_line(&crate::widgets::Here {
            label: String::new(),
            latitude: f64::NAN,
            longitude: f64::NAN,
            timezone: "Asia/Hong_Kong".into(),
        });
        assert!(clock.contains("timezone is Asia/Hong_Kong"), "{clock}");
        let plan = prompts::fill(
            prompts::PLAN_SEARCH_USER,
            &[
                ("place", "The user is in Hong Kong.\n\n"),
                ("query", "weather"),
                ("history", ""),
                ("round", "1"),
                ("rounds", "1"),
                ("asked", ""),
                ("found", ""),
            ],
        );
        assert!(plan.contains("The user is in Hong Kong."));
        assert!(!plan.contains("{place}"));
    }

    #[test]
    fn date_skill_reaches_prompts_that_had_no_date() {
        let image = with_today(prompts::IMAGE_READ_SYSTEM.into());
        assert!(image.starts_with("Today is "));
        assert!(image.contains(prompts::IMAGE_READ_SYSTEM));
        let already = with_today("Today is Monday. Rules.".into());
        assert_eq!(already, "Today is Monday. Rules.");
        let clocked = with_clock(
            "Answer the question.".into(),
            "Clock:\nTokyo is 11:57 UTC+09:00.",
        );
        assert!(clocked.starts_with("Clock:\nTokyo is 11:57"), "{clocked}");
        assert!(clocked.contains("Answer the question."));
        let plan = prompts::fill(
            prompts::PLAN_SEARCH_SYSTEM,
            &[
                ("date", "Monday, 12 October 2026, 01:46 UTC+08:00"),
                ("n", "3"),
            ],
        );
        assert!(plan.contains("Monday, 12 October 2026"));
        assert!(!plan.contains("{date}"));
    }

    #[test]
    fn pick_indices_keeps_order_and_skips_unknowns() {
        let results = vec![
            hit("https://a.example"),
            hit("https://b.example"),
            hit("https://c.example"),
        ];
        let kept = pick_indices(&results, &vec![json!(3), json!(1), json!(9), json!(1)]);
        let urls: Vec<_> = kept.into_iter().map(|r| r.url).collect();
        assert_eq!(
            urls,
            vec![
                "https://c.example".to_string(),
                "https://a.example".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn stop_drops_the_request_in_flight() {
        let stop = Stop::new(false);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let started = Arc::new(Notify::new());
        let started2 = started.clone();

        struct Bomb(Arc<AtomicBool>);
        impl Drop for Bomb {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let work = async move {
            let _bomb = Bomb(flag);
            started2.notify_one();
            std::future::pending::<()>().await;
        };
        let stop2 = stop.clone();
        let task = tokio::spawn(async move {
            let _ = cut(&stop2, &tx, work).await;
        });

        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("stage should be waiting");
        assert!(!dropped.load(Ordering::SeqCst));
        stop.trip();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("stop should finish the stage")
            .unwrap();
        assert!(
            dropped.load(Ordering::SeqCst),
            "in-flight work must be dropped"
        );
        drop(rx);
    }

    #[tokio::test]
    async fn a_closed_stream_does_not_cancel_the_work() {
        let stop = Stop::new(false);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        drop(rx);
        let out = cut(&stop, &tx, async { 7 }).await;
        assert!(matches!(out, Ok(7)));
    }

    #[test]
    fn stop_before_start_cancels_and_a_late_stop_does_not_poison() {
        let runs = Runs::default();
        runs.cancel("c");
        let flag = runs.begin("c");
        assert!(flag.load(Ordering::Relaxed));
        runs.end("c");
        runs.cancel("c");
        let next = runs.begin("c");
        assert!(!next.load(Ordering::Relaxed));
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

    /// The trace from a real stuck answer: the same citation checklist, cycled forever.
    fn stuck_trace(cycles: usize) -> String {
        let cycle = "Need maybe mention 30B about 24GB at Q4 [2]. Good.\n\
             Need maybe mention used RTX 3090 $800-1050 87% throughput [12]. Good.\n\
             Need maybe mention Tesla P40 $300 caveats [20]. Good.\n\
             Need maybe mention RTX 4090 $1500-2200 best-value NVIDIA [10]. Good.\n\
             Need maybe mention single 24GB practical floor [17]. Good.\n\
             Need maybe mention 2x used 3090 48GB running 70B [15]. Good.\n";
        cycle.repeat(cycles)
    }

    #[test]
    fn a_repeating_reasoning_trace_is_caught() {
        let mut guard = LoopGuard::default();
        // Streamed in small pieces, the way a provider actually sends them.
        for piece in stuck_trace(20).as_bytes().chunks(7) {
            let piece = std::str::from_utf8(piece).expect("ascii");
            if let Some(line) = guard.feed(piece) {
                assert!(
                    line.starts_with("Need maybe mention 30B about 24GB"),
                    "{line}"
                );
                return;
            }
        }
        panic!("the loop ran 20 cycles without being caught");
    }

    /// The trace from a stuck answer that never repeats a whole line: one checklist
    /// item, with a different ending every time. Exact equality does not see it.
    #[test]
    fn a_rewritten_checklist_is_a_loop() {
        let mut guard = LoopGuard::default();
        let mut caught = None;
        for i in 0..20 {
            let line =
                format!("Need maybe include if using `Raspberry Pi` `dhcpcd` `item{i}`? no.\n");
            if let Some(hit) = guard.feed(&line) {
                caught = Some((i, hit));
                break;
            }
        }
        let (at, hit) = caught.expect("the rewritten checklist was not caught");
        assert_eq!(at, STEM_HITS - 1, "caught too early, at line {at}");
        assert!(
            hit.starts_with("Need maybe include if using `Raspberry Pi` `dhcpcd` `item0`"),
            "{hit}"
        );
    }

    #[test]
    fn ordinary_reasoning_is_not_a_loop() {
        let mut guard = LoopGuard::default();
        let lines = [
            "The user wants Traditional Chinese, so the answer must be in that script.",
            "Source 2 says a 30B model is about 24GB at Q4, which fits one card.",
            "Source 12 gives the used 3090 price and the throughput share.",
            "A table reads better here than prose: price, VRAM, why it fits.",
            "I should not cite a source I did not use, and not invent a reference number.",
            "The close should say what is still unknown rather than recap the sections.",
        ];
        let text = lines.iter().flat_map(|l| [*l, "\n"]).collect::<String>();
        for piece in text.as_bytes().chunks(11) {
            let piece = std::str::from_utf8(piece).expect("ascii");
            assert!(guard.feed(piece).is_none());
        }
        // Two copies of a long line, and any number of short ones, are not a stuck model.
        let mut twice = LoopGuard::default();
        let long = format!("{}\n{}\n", lines[0], lines[0]);
        assert!(twice.feed(&long).is_none());
        let mut bullets = LoopGuard::default();
        for _ in 0..40 {
            assert!(bullets.feed("- 24 GB\n").is_none());
        }
        // Many different claims that only share a short opening are still thinking.
        let mut checklist = LoopGuard::default();
        for i in 0..40 {
            let line = format!(
                "Need maybe mention source {i} adds a fact the earlier lines did not.\n"
            );
            assert!(checklist.feed(&line).is_none(), "{line}");
        }
    }

    #[test]
    fn an_endless_single_line_hits_the_budget_instead() {
        let mut guard = LoopGuard::default();
        for _ in 0..4_000 {
            if let Some(hit) = guard.feed("thinking without any line break at all ") {
                assert!(hit.is_empty(), "a long line with no repeat is the budget, not a loop");
                return;
            }
        }
        panic!("4000 chunks of one line stayed under the budget");
    }

    #[tokio::test]
    async fn the_saved_thread_loses_the_cycles_not_the_thinking() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut text = String::new();
        {
            let mut mind = Mind {
                tx: &tx,
                text: &mut text,
            };
            mind.push("First I worked out the VRAM.\n");
            mind.push(&stuck_trace(1));
            mind.cut_loop("Need maybe mention 30B about 24GB at Q4 [2]. Good.");
        }
        assert_eq!(text, "First I worked out the VRAM.\n");
    }
}
