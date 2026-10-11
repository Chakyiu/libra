# Libra

A self-hosted answer engine, and a self-hosted Perplexity: ask in plain language and get a cited
answer from the open web, on your own hardware. It is a rebuild of [Vane](https://github.com/ItzCrazyKns/Vane)
with a **Rust + Redis + SQLite** backend and a **Next.js** frontend. One binary serves the API and
the pre-built UI; Redis is a cache, not a requirement.

```
question → classify → widgets ┐
         → plan searches      ├→ SSE stream → answer with [n] citations → follow-ups
         → search rounds / RAG┘
```

## Why this shape

| | original | Libra |
|---|---|---|
| backend | Next.js (Node) | Rust (axum), single static binary |
| storage | SQLite (drizzle) | SQLite (sqlx, WAL) |
| cache | none | Redis: search results, embeddings, discover, single-flight |
| research loop | native tool-calling, 2–25 iterations | plan the queries, then 1, 2, or 3 search rounds (fast, balance, deep) |
| providers | OpenAI/Anthropic/Gemini/Groq/Ollama/… | OpenAI-compatible (incl. OpenRouter, Ollama, LM Studio), Anthropic, Gemini |
| search | SearxNG only | SearxNG, Brave, Tavily, Exa, DuckDuckGo, Bing HTML scrape, Firecrawl — per-kind ordered fallback |
| UI | full React app | lean Next.js app, statically exported and served by the Rust binary |

The research loop is the main behavioural deviation: upstream drives a multi-iteration agent
through native tool calls, which needs a strong model. Libra asks the fast model what to search first, runs those queries, then asks it what to search
next. Balance does one follow-up round and Deep does two; Fast stops after the first. The fast
model then drops sources that cannot help, the rerank model orders what remains, and a stronger
model writes. Pages are not scraped one by one.

## Quickstart

```bash
docker compose up --build      # UI + API on http://localhost:3001
```

A fresh instance has no model providers. Open `http://localhost:3001/settings/?tab=providers`, add
one (Ollama, OpenAI, OpenRouter, Anthropic, Gemini or any OpenAI-compatible server) with the models
it serves, and ask something.

### Without Docker

```bash
cp .env.example .env            # edit keys
cargo run                       # API + UI on :3001 (expects web/out)

# frontend development (hot reload, proxies /api to :3001)
cd web && npm install && npm run dev   # http://localhost:3000
```

Redis is optional: if `REDIS_URL` is unreachable the service logs a warning and runs with
caching disabled. Set `REDIS_DISABLED=1` to skip the connection attempt entirely.

## Layout

```
src/
  main.rs            startup, config, graceful shutdown
  api.rs             axum router: /api/* + static UI
  config.rs          env + settings.json, hot-swappable SettingsHandle
  db.rs              sqlx pool, migrations, chats/messages/documents/chunks
  cache.rs           Redis wrapper; degrades to no-op
  blocks.rs          block protocol streamed to the UI
  llm/               openai-compatible, anthropic, gemini; SSE + JSON extraction
  search/            provider trait + 7 providers + fallback router with Redis cache
  pipeline/          ask(): classify → widgets → plan searches → search rounds → rerank → answer → suggestions
  uploads/           text/pdf/docx extraction, chunking, embedding, cosine retrieval
  widgets/           calculator, time, weather (Open-Meteo), stocks (stooq)
  prompts/           every LLM prompt, all returning strict JSON
web/                 Next.js app router + plain CSS from the design/ mockups (static export)
docker/              searxng settings
```

## HTTP API

| | |
|---|---|
| `POST /api/ask` | SSE stream. Body `{id, query, model, models:{answer,light,rerank}, depth:fast\|balance\|deep, files:[id], message_history:[{role,content}], filters:{country,language,freshness,perplexity_site}, disabled_widgets:[]}`. `depth` defaults to balance. |
| `GET /api/chats` · `POST /api/chats` | list / create. Each chat includes `status`, the phase of its latest answer |
| `GET /api/chats/{id}` | `{chat, messages:[{id,role,content,blocks,files,status,error,model,elapsed_ms,created_at}]}` |
| `PATCH /api/chats/{id}` | rename `{title, emoji}` · `DELETE` removes it |
| `POST /api/chats/{id}/stop` | stop the answer in progress; the partial answer stays stored |
| `POST /api/chats/{id}/location` | browser fix `{latitude, longitude, timezone, denied}` while an ask is waiting |
| `POST /api/upload` | multipart `chatId` + `files[]` → `{files:[{id,name,chunks}]}` |
| `GET /api/search?q=&kind=web\|news\|images\|videos` | raw provider results (Redis cached) |
| `GET /api/suggestions?q=` | search autocomplete |
| `GET /api/discover?topic=tech\|finance\|art\|sports\|entertainment` | recent news for that topic; a story opens a thread that summarizes its URL |
| `POST /api/generate/image` | `{prompt}` → base64 image (needs an image model) |
| `GET /api/settings` · `PUT /api/settings` | read/patch the whole config, applied hot |
| `GET /api/models` · `GET /api/health` | model ids · status + redis + providers |

SSE frames are `data: {...}` with `type`: `id` (chat id), `b` (block), `t` (reasoning delta), `a` (answer text delta),
`loc` (ask the browser for a location; `precise` true means coordinates), `c` (error), `e` (end). Blocks are `{type, message, data}` — `answered`, `widget`, `weather`,
`stocks`, `calculation`, `time`, `research`, `references`, `images`, `videos`, `related`, `message`.
The question is written to SQLite as soon as the ask starts, and the answer's phase
(`search`, `read`, `write`, then `done`, `stopped`, or `error`), blocks, and text are
updated as they arrive. Reloading the page restores that row immediately and keeps
polling while the phase is still in progress. A process restart marks any in-progress
turn stopped, so a thread is never stuck on "answering".

## Configuration

Everything in `.env.example` can also be changed at runtime from `/settings` (written to
`$LIBRA_DATA_DIR/settings.json`, applied to the live `SettingsHandle` without a restart).
Env vars seed defaults; `settings.json` overrides them; the API overrides both. Model providers and
the models chosen from them are the exception: they only come from Settings, never from env vars.

Search fallback order is per kind (`WEB_SEARCH_ORDER`, …). A provider is skipped when it has no
key, is disabled, or returns nothing — the first provider with results wins.

## Tests

```bash
cargo test                     # unit tests: prompts, json extraction, chunking, docx, config
python3 scripts/mock-llm.py &  # OpenAI-compatible stub on :8899, no API keys needed
cargo run &                    # then PUT /api/settings with a "mock" provider and POST /api/ask
```

`scripts/mock-llm.py` answers every prompt with fixed JSON and streams a fake answer, which is
enough to exercise the whole pipeline end to end.

## Not implemented (yet)

- Per-result page extraction: Firecrawl's scrape endpoint is implemented but not wired into the answer path.
- Currency conversion in the calculator, and the upstream "discussion mode" long-form writer.
- Auth: the service is assumed to sit behind a reverse proxy. CORS is wide open for dev.
- Embedding search is brute-force cosine over SQLite BLOBs — fine up to a few thousand chunks.

## License

[MIT](LICENSE).
