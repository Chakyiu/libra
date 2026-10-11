# Libra

A self-hosted answer engine — a rebuild of [Vane](https://github.com/ItzCrazyKns/Vane) with a
**Rust + Redis + SQLite** backend and a **Next.js** frontend. One binary serves the API and the
pre-built UI; Redis is a cache, not a requirement.

```
question → classify → widgets ┐
         → rewrite follow-up  ├→ SSE stream → answer with [n] citations → follow-ups
         → search / doc RAG   ┘
```

## Why this shape

| | original | Libra |
|---|---|---|
| backend | Next.js (Node) | Rust (axum), single static binary |
| storage | SQLite (drizzle) | SQLite (sqlx, WAL) |
| cache | none | Redis: search results, embeddings, discover, single-flight |
| research loop | native tool-calling, 2–25 iterations | one structured-JSON plan per question |
| providers | OpenAI/Anthropic/Gemini/Groq/Ollama/… | OpenAI-compatible (incl. OpenRouter, Ollama, LM Studio), Anthropic, Gemini |
| search | SearxNG only | SearxNG, Brave, Tavily, Exa, DuckDuckGo, Bing HTML scrape, Firecrawl — per-kind ordered fallback |
| UI | full React app | lean Next.js app, statically exported and served by the Rust binary |

The research loop is the main behavioural deviation: upstream drives a multi-iteration agent
through native tool calls, which needs a strong model. Libra classifies, picks widgets,
searches once across providers, reranks with a cheap model, then writes. That keeps latency low
and works with small local models. It is a deliberate trade-off, not a missing feature.

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
  pipeline/          ask(): classify → widgets → search → rerank → answer → suggestions
  uploads/           text/pdf/docx extraction, chunking, embedding, cosine retrieval
  widgets/           calculator, time, weather (Open-Meteo), stocks (stooq)
  prompts/           every LLM prompt, all returning strict JSON
web/                 Next.js app router + plain CSS from the design/ mockups (static export)
docker/              searxng settings
```

## HTTP API

| | |
|---|---|
| `POST /api/ask` | SSE stream. Body `{id, query, model, models:{answer,light,rerank}, files:[id], message_history:[{role,content}], filters:{country,language,freshness,perplexity_site}, disabled_widgets:[]}` |
| `GET /api/chats` · `POST /api/chats` | list / create |
| `GET /api/chats/{id}` | `{chat, messages:[{id,role,content,blocks,files,created_at}]}` |
| `PATCH /api/chats/{id}` | rename `{title, emoji}` · `DELETE` removes it |
| `POST /api/upload` | multipart `chatId` + `files[]` → `{files:[{id,name,chunks}]}` |
| `GET /api/search?q=&kind=web\|news\|images\|videos` | raw provider results (Redis cached) |
| `GET /api/suggestions?q=` | search autocomplete |
| `GET /api/discover?chat_id=` | follow-up questions for a conversation |
| `POST /api/generate/image` | `{prompt}` → base64 image (needs an image model) |
| `GET /api/settings` · `PUT /api/settings` | read/patch the whole config, applied hot |
| `GET /api/models` · `GET /api/health` | model ids · status + redis + providers |

SSE frames are `data: {...}` with `type`: `id` (chat id), `b` (block), `a` (answer text delta),
`c` (error), `e` (end). Blocks are `{type, message, data}` — `answered`, `widget`, `weather`,
`stocks`, `calculation`, `time`, `references`, `images`, `videos`, `related`, `message`.
The same blocks are persisted, so reloading a chat replays it exactly.

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

- Multi-iteration agentic research (plan → search → scrape → plan again) and per-result page
  extraction: Firecrawl's scrape endpoint is implemented but not wired into the answer path.
- Currency conversion in the calculator, and the upstream "discussion mode" long-form writer.
- Auth: the service is assumed to sit behind a reverse proxy. CORS is wide open for dev.
- Embedding search is brute-force cosine over SQLite BLOBs — fine up to a few thousand chunks.
# libra
