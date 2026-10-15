"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "next/navigation";
import Topbar, { VERSION, useHost } from "@/components/Topbar";
import { CodeIcon, FlameIcon, GlobeIcon, PlusIcon, SearchIcon, ShieldIcon } from "@/components/icons";
import { useInstance } from "@/components/instance";
import { useThreads } from "@/components/threads";
import { Chat, Provider, Settings, api, deleteChat, putJson } from "@/lib/api";
import { modelLabel, plural } from "@/lib/format";
import { LEADING_MAX, LEADING_MIN, PREF_DEFAULTS, SIZE_MAX, SIZE_MIN, setPref, usePref } from "@/lib/prefs";

type Tab = "search" | "answering" | "providers" | "appearance" | "data";

const TABS: { id: Tab; label: string }[] = [
  { id: "search", label: "Search & sources" },
  { id: "answering", label: "Answering" },
  { id: "providers", label: "Model providers" },
  { id: "appearance", label: "Reading & appearance" },
  { id: "data", label: "Threads & data" },
];

const SEARCH_PROVIDERS: {
  id: string;
  label: string;
  help: string;
  Icon: React.ComponentType;
  key?: string;
}[] = [
  { id: "searxng", label: "SearxNG", help: "Self-hosted metasearch across dozens of engines. No key — point it at your instance.", Icon: GlobeIcon },
  { id: "brave", label: "Brave Search", help: "Independent index for web, news, image and video results.", Icon: ShieldIcon, key: "braveApiKey" },
  { id: "tavily", label: "Tavily", help: "Search API built for answer engines; returns cleaned page text.", Icon: SearchIcon, key: "tavilyApiKey" },
  { id: "exa", label: "Exa", help: "Neural search over its own web index.", Icon: SearchIcon, key: "exaApiKey" },
  { id: "firecrawl", label: "Firecrawl", help: "Search that also scrapes the pages it finds.", Icon: FlameIcon, key: "firecrawlApiKey" },
  { id: "duckduckgo", label: "DuckDuckGo", help: "No key. Reads the HTML results page, so it can be rate-limited.", Icon: GlobeIcon },
  { id: "html-scrape", label: "Bing (HTML)", help: "No key. Scrapes Bing’s results pages — the fallback of last resort.", Icon: CodeIcon },
];

const ORDER: { kind: "web" | "news" | "images" | "videos"; label: string }[] = [
  { kind: "web", label: "Web" },
  { kind: "news", label: "News" },
  { kind: "images", label: "Images" },
  { kind: "videos", label: "Videos" },
];

const WIDGETS = [
  { id: "calculator", label: "Calculator", help: "Works out arithmetic and currency conversions in the question and shows them above the answer." },
  { id: "time", label: "Time", help: "The current time in a city or timezone." },
  { id: "weather", label: "Weather", help: "Current conditions and a five-day forecast from Open-Meteo. No key needed." },
  { id: "stocks", label: "Stocks", help: "Quote, session chart, and key stats for a US-listed company or ticker. No key needed." },
];

const PROVIDER_TYPES: Record<string, string> = {
  ollama: "Ollama",
  open_ai_compatible: "OpenAI-compatible",
  open_ai: "OpenAI",
  open_router: "OpenRouter",
  anthropic: "Anthropic",
  gemini: "Gemini",
  custom: "Custom",
};

/**
 * Connection details for each kind of provider. Model names are only ever placeholders:
 * which models a provider serves is the user's to list.
 */
type Preset = { type: string; id: string; name: string; base: string; key: boolean; models: string; embedding: string; note?: string };

const PRESETS: Preset[] = [
  {
    type: "ollama",
    id: "ollama",
    name: "Ollama",
    base: "http://127.0.0.1:11434/v1",
    key: false,
    models: "e.g. llama3.1:8b, qwen2.5:7b",
    embedding: "e.g. nomic-embed-text",
    note: "Runs on your hardware, so questions stay on this host. From inside Docker the base URL is usually http://host.docker.internal:11434/v1.",
  },
  {
    type: "open_ai_compatible",
    id: "local",
    name: "OpenAI-compatible",
    base: "http://127.0.0.1:1234/v1",
    key: false,
    models: "the model ids your server lists",
    embedding: "optional",
    note: "LM Studio, vLLM, llama.cpp, LocalAI or a hosted gateway — anything that speaks the OpenAI chat API.",
  },
  { type: "open_ai", id: "openai", name: "OpenAI", base: "https://api.openai.com/v1", key: true, models: "e.g. gpt-4.1-mini", embedding: "e.g. text-embedding-3-small" },
  { type: "open_router", id: "openrouter", name: "OpenRouter", base: "https://openrouter.ai/api/v1", key: true, models: "e.g. anthropic/claude-sonnet-4.5", embedding: "none" },
  {
    type: "anthropic",
    id: "anthropic",
    name: "Anthropic",
    base: "https://api.anthropic.com",
    key: true,
    models: "e.g. claude-sonnet-4-5",
    embedding: "none — Anthropic has no embedding models",
  },
  {
    type: "gemini",
    id: "gemini",
    name: "Gemini",
    base: "https://generativelanguage.googleapis.com",
    key: true,
    models: "e.g. gemini-2.5-flash",
    embedding: "e.g. gemini-embedding-001",
  },
];

const presetFor = (type: string) => PRESETS.find((p) => p.type === type) ?? PRESETS[1];

const chatModels = (providers: Provider[]) => providers.flatMap((p) => p.models.map((m) => `${p.id}/${m}`));
const embeddingModels = (providers: Provider[]) => providers.flatMap((p) => p.embedding_models.map((m) => `${p.id}/${m}`));

const MODEL_KEYS = ["defaultModel", "defaultLightModel", "defaultRerankModel", "defaultEmbeddingModel", "defaultImageModel"] as const;

/** Keep model choices pointing at the same models when a provider's id is edited. */
function renameRefs(s: Settings, from: string, to: string): Settings {
  const next = { ...s };
  for (const key of MODEL_KEYS) {
    const ref: string = next[key] ?? "";
    if (from && ref.startsWith(`${from}/`)) next[key] = `${to}/${ref.slice(from.length + 1)}`;
  }
  return next;
}

/**
 * After providers change, drop choices that no longer exist and fill the ones a first provider makes
 * possible, so the answer model and file embeddings work without a second trip to Answering.
 */
function reconcileModels(s: Settings): Settings {
  const chat = chatModels(s.providers);
  const embedding = embeddingModels(s.providers);
  const keep = (ref: string, options: string[], fallback = "") => (ref && options.includes(ref) ? ref : fallback);
  return {
    ...s,
    defaultModel: keep(s.defaultModel, chat, chat[0] ?? ""),
    defaultLightModel: keep(s.defaultLightModel, chat),
    defaultRerankModel: keep(s.defaultRerankModel, chat),
    defaultEmbeddingModel: keep(s.defaultEmbeddingModel, embedding, embedding[0] ?? ""),
  };
}

const parseList = (s: string) =>
  s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);

/** Comma-separated list field that keeps what you type until you leave it. */
function ListInput({
  value,
  onChange,
  label,
  mono,
  placeholder,
  autoFocus,
}: {
  value: string[];
  onChange: (v: string[]) => void;
  label: string;
  mono?: boolean;
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const joined = value.join(", ");
  const [text, setText] = useState(joined);
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setText(joined);
  }, [joined]);
  return (
    <input
      type="text"
      className={mono ? "is-mono" : undefined}
      value={text}
      aria-label={label}
      placeholder={placeholder}
      autoFocus={autoFocus}
      spellCheck={false}
      onFocus={() => (focused.current = true)}
      onBlur={() => {
        focused.current = false;
        setText(joined);
      }}
      onChange={(e) => {
        setText(e.target.value);
        onChange(parseList(e.target.value));
      }}
    />
  );
}

function Row({ label, help, children, last, danger }: { label: string; help?: React.ReactNode; children: React.ReactNode; last?: boolean; danger?: boolean }) {
  return (
    <div className={`srow${last ? " srow--last" : ""}`}>
      <div>
        <p className={`srow__label${danger ? " danger" : ""}`}>{label}</p>
        {help && <p className="srow__help">{help}</p>}
      </div>
      <div className="srow__control">{children}</div>
    </div>
  );
}

function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return <input className="switch" type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} aria-label={label} disabled={disabled} />;
}

function Seg<T extends string>({ value, options, onChange, label }: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; label: string }) {
  return (
    <div className="seg" role="group" aria-label={label}>
      {options.map((o) => (
        <button key={o.value} type="button" aria-pressed={value === o.value} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

function ModelSelect({
  value,
  options,
  onChange,
  label,
  unset,
}: {
  value: string;
  options: string[];
  onChange: (v: string) => void;
  label: string;
  /** what leaving it unset means; without one, unset is only a prompt to choose */
  unset?: string;
}) {
  const all = !value || options.includes(value) ? options : [value, ...options];
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)} aria-label={label}>
      {unset != null ? (
        <option value="">{unset}</option>
      ) : (
        !value && (
          <option value="" disabled>
            Choose a model
          </option>
        )
      )}
      {all.map((m) => (
        <option key={m} value={m}>
          {options.includes(m) ? modelLabel(m) : `${modelLabel(m)} — not configured`}
        </option>
      ))}
    </select>
  );
}

type SetField = <K extends keyof Settings>(key: K, value: Settings[K]) => void;

function SearchPanel({ s, set, providerNames, redis }: { s: Settings; set: SetField; providerNames: string[]; redis?: boolean }) {
  const enabled = s.enabledProviders?.length ? s.enabledProviders : providerNames;
  const on = (id: string) => enabled.includes(id);
  const onCount = providerNames.filter(on).length;

  const toggle = (id: string, value: boolean) => {
    let next = value ? Array.from(new Set([...enabled, id])) : enabled.filter((x) => x !== id);
    // an empty list means "all of them", so turning every provider back on clears it
    if (providerNames.every((p) => next.includes(p))) next = [];
    set("enabledProviders", next);
  };

  return (
    <>
      <header>
        <p className="kicker">
          {onCount} of {providerNames.length} providers on · {redis ? "results cached in Redis" : "no result cache"}
        </p>
        <h2>Search &amp; sources</h2>
        <p>Libra asks these providers in the order below and keeps the first one that comes back with results. Keys stay on this host.</p>
      </header>

      {SEARCH_PROVIDERS.filter((p) => providerNames.includes(p.id)).map(({ id, label, help, Icon, key }) => (
        <div className="srow srow--connector" key={id}>
          <span className="conn-icon">
            <Icon />
          </span>
          <div>
            <p className="srow__label">{label}</p>
            <p className="srow__help">{help}</p>
          </div>
          <div className="srow__control">
            {id === "searxng" && (
              <div className="srow__stack">
                <input type="url" value={s.searxngUrl ?? ""} onChange={(e) => set("searxngUrl", e.target.value)} placeholder="http://searxng:8080" aria-label="SearxNG URL" />
                <input
                  type="password"
                  value={s.searxngPassword ?? ""}
                  onChange={(e) => set("searxngPassword", e.target.value)}
                  placeholder="Password, if protected"
                  aria-label="SearxNG password"
                  autoComplete="off"
                />
              </div>
            )}
            {key && (
              <input type="password" value={s[key] ?? ""} onChange={(e) => set(key, e.target.value)} placeholder="API key" aria-label={`${label} API key`} autoComplete="off" />
            )}
            <Switch checked={on(id)} onChange={(v) => toggle(id, v)} label={`Use ${label}`} disabled={on(id) && onCount === 1} />
          </div>
        </div>
      ))}

      <p className="kicker panel__sub">Fallback order</p>
      {ORDER.map(({ kind, label }) => (
        <Row key={kind} label={label} help={kind === "web" ? "Provider ids, tried left to right." : undefined}>
          <ListInput
            mono
            label={`${label} search order`}
            value={s.searchOrder?.[kind] ?? []}
            onChange={(v) => set("searchOrder", { ...s.searchOrder, [kind]: v })}
          />
        </Row>
      ))}

      <p className="kicker panel__sub">Results</p>
      <Row label="Results per search" help={`How many results each search asks for before reranking. ${s.numWebResults} currently.`}>
        <input type="range" min={1} max={50} value={s.numWebResults ?? 20} onChange={(e) => set("numWebResults", Number(e.target.value))} aria-label="Results per search" />
      </Row>
      <Row label="Region" help="Country code passed to providers that support one, or auto.">
        <input type="text" value={s.country ?? ""} onChange={(e) => set("country", e.target.value)} aria-label="Region" style={{ width: "8rem" }} />
      </Row>
      <Row label="Safe search" help="Ask providers to filter explicit results.">
        <Switch checked={!!s.safeSearch} onChange={(v) => set("safeSearch", v)} label="Safe search" />
      </Row>
      <Row label="Image results" help="Search images when a question would be clearer with them.">
        <Switch checked={!!s.enableImageSearch} onChange={(v) => set("enableImageSearch", v)} label="Image results" />
      </Row>
      <Row label="Video results" help="Search videos when a question asks for them.">
        <Switch checked={!!s.enableVideoSearch} onChange={(v) => set("enableVideoSearch", v)} label="Video results" />
      </Row>
      <Row label="News for time-limited requests" help="When an API request sets a freshness window, also search news and merge what it finds." last>
        <Switch checked={!!s.enableFreshSearch} onChange={(v) => set("enableFreshSearch", v)} label="News for time-limited requests" />
      </Row>
    </>
  );
}

function AnsweringPanel({ s, set, onAddProvider }: { s: Settings; set: SetField; onAddProvider: () => void }) {
  const chat = useMemo(() => chatModels(s.providers), [s.providers]);
  const embedding = useMemo(() => embeddingModels(s.providers), [s.providers]);
  const disabled: string[] = s.disabledWidgets ?? [];

  return (
    <>
      <header>
        <p className="kicker">Generation</p>
        <h2>Answering</h2>
        <p>Which models read, rank and write — and what they’re allowed to reach for besides search.</p>
      </header>

      {chat.length === 0 ? (
        <div className="callout panel__callout">
          <b>No models to choose from yet.</b> Add a model provider and list the models it serves — they show up here.
          <div>
            <button type="button" className="btn" onClick={onAddProvider}>
              Add a model provider
            </button>
          </div>
        </div>
      ) : (
        <>
          <Row label="Answer model" help="Writes every answer. The composer can switch models for a single question.">
            <ModelSelect value={s.defaultModel} options={chat} onChange={(v) => set("defaultModel", v)} label="Answer model" />
          </Row>
          <Row label="Fast model" help="Classifies questions, rewrites follow-ups, picks widgets and names threads. Small and quick is fine.">
            <ModelSelect
              value={s.defaultLightModel}
              options={chat}
              onChange={(v) => set("defaultLightModel", v)}
              label="Fast model"
              unset="Same as answer model"
            />
          </Row>
          <Row label="Rerank model" help="Chooses the most relevant results before the answer model reads them.">
            <ModelSelect
              value={s.defaultRerankModel}
              options={chat}
              onChange={(v) => set("defaultRerankModel", v)}
              label="Rerank model"
              unset="Same as fast model"
            />
          </Row>
          <Row
            label="Embedding model"
            help={
              embedding.length
                ? "Embeds uploaded files so questions can be answered from them."
                : "None of your providers lists an embedding model, so files can’t be attached yet."
            }
          >
            <ModelSelect
              value={s.defaultEmbeddingModel}
              options={embedding}
              onChange={(v) => set("defaultEmbeddingModel", v)}
              label="Embedding model"
              unset="Not set — file uploads off"
            />
          </Row>
        </>
      )}
      <Row label="Temperature" help={`Lower keeps answers closer to their sources. ${Number(s.defaultTemperature ?? 0).toFixed(1)} currently.`}>
        <input
          type="range"
          min={0}
          max={2}
          step={0.1}
          value={s.defaultTemperature ?? 0.3}
          onChange={(e) => set("defaultTemperature", Number(e.target.value))}
          aria-label="Temperature"
        />
      </Row>
      <Row label="Rerank results" help="Let the rerank model pick which results the answer is written from, instead of taking them in provider order.">
        <Switch checked={!!s.enableReranker} onChange={(v) => set("enableReranker", v)} label="Rerank results" />
      </Row>
      <Row label="Sources per answer" help={`How many results the answer model reads and can cite. ${s.rerankLimit} currently.`}>
        <input type="range" min={1} max={50} value={s.rerankLimit ?? 7} onChange={(e) => set("rerankLimit", Number(e.target.value))} aria-label="Sources per answer" />
      </Row>

      <p className="kicker panel__sub">Widgets</p>
      {WIDGETS.map((w) => (
        <Row key={w.id} label={w.label} help={w.help}>
          <Switch
            checked={!disabled.includes(w.id)}
            onChange={(v) => set("disabledWidgets", v ? disabled.filter((x) => x !== w.id) : [...disabled, w.id])}
            label={`${w.label} widget`}
          />
        </Row>
      ))}
      <Row label="Image generation" help="Allow the image generation endpoint to call the configured image model." last>
        <Switch checked={!!s.imageGenerationEnabled} onChange={(v) => set("imageGenerationEnabled", v)} label="Image generation" />
      </Row>
    </>
  );
}

function ProvidersPanel({ providers, onChange }: { providers: Provider[]; onChange: (p: Provider[], rename?: [string, string]) => void }) {
  const [added, setAdded] = useState<number | null>(null);
  const update = (i: number, patch: Partial<Provider>, rename?: [string, string]) =>
    onChange(
      providers.map((p, j) => (j === i ? { ...p, ...patch } : p)),
      rename,
    );
  const usable = providers.filter((p) => p.models.length > 0).length;
  const taken = (id: string) => providers.filter((p) => p.id === id).length;

  const add = (preset: Preset) => {
    let id = preset.id;
    for (let n = 2; providers.some((p) => p.id === id); n++) id = `${preset.id}-${n}`;
    setAdded(providers.length);
    onChange([...providers, { id, name: preset.name, type: preset.type, base_url: preset.base, api_key: "", models: [], embedding_models: [] }]);
  };

  // switching type moves the base URL along unless it was edited by hand
  const changeType = (i: number, type: string) => {
    const p = providers[i];
    const base = !p.base_url || p.base_url === presetFor(p.type).base ? presetFor(type).base : p.base_url;
    update(i, { type, base_url: base });
  };

  return (
    <>
      <header>
        <p className="kicker">{providers.length ? `${plural(providers.length, "provider")} · ${usable} with models` : "None added yet"}</p>
        <h2>Model providers</h2>
        <p>Libra writes answers with the models you add here, addressed as provider/model. Local providers keep question text on this host; remote ones send it to that service.</p>
      </header>

      {!providers.length && (
        <p className="callout panel__callout">
          <b>No model provider yet.</b> Add one below to start asking — a local server such as Ollama or LM Studio, or an API such as OpenAI or OpenRouter.
        </p>
      )}

      {providers.map((p, i) => {
        const preset = presetFor(p.type);
        const clash = !p.id ? "Needed — it prefixes this provider’s model names." : taken(p.id) > 1 ? "Another provider already uses this id." : "";
        const fresh = i === added;
        return (
          <div className="provider" key={i}>
            <div className="provider__head">
              <b>{p.name || p.id || "New provider"}</b>
              {(PROVIDER_TYPES[p.type] ?? p.type) !== p.name && <span className="tag">{PROVIDER_TYPES[p.type] ?? p.type}</span>}
              <span className="topbar__spacer" />
              <button
                type="button"
                className="btn btn--quiet btn--danger"
                onClick={() => {
                  setAdded(null);
                  onChange(providers.filter((_, j) => j !== i));
                }}
              >
                Remove
              </button>
            </div>
            {preset.note && <p className="provider__note">{preset.note}</p>}
            <div className="fields">
              <label className="field-label">
                <span>Name</span>
                <input type="text" value={p.name} onChange={(e) => update(i, { name: e.target.value })} />
              </label>
              <label className="field-label">
                <span>Id — prefixes its model names</span>
                <input
                  type="text"
                  value={p.id}
                  spellCheck={false}
                  aria-invalid={!!clash}
                  onChange={(e) => {
                    const id = e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, "");
                    update(i, { id }, [p.id, id]);
                  }}
                />
                {clash && <em className="field-error">{clash}</em>}
              </label>
              <label className="field-label">
                <span>Type</span>
                <select value={p.type} onChange={(e) => changeType(i, e.target.value)}>
                  {Object.entries(PROVIDER_TYPES).map(([value, label]) => (
                    <option key={value} value={value}>
                      {label}
                    </option>
                  ))}
                </select>
              </label>
              <label className="field-label">
                <span>Base URL</span>
                <input type="url" value={p.base_url} placeholder={preset.base} onChange={(e) => update(i, { base_url: e.target.value })} spellCheck={false} />
              </label>
              <label className="field-label">
                <span>API key</span>
                <input
                  type="password"
                  value={p.api_key}
                  placeholder={preset.key ? "required" : "not needed"}
                  autoComplete="off"
                  autoFocus={fresh && preset.key}
                  onChange={(e) => update(i, { api_key: e.target.value })}
                />
              </label>
              <label className="field-label">
                <span>Chat models</span>
                <ListInput
                  label={`${p.name} chat models`}
                  value={p.models}
                  placeholder={preset.models}
                  autoFocus={fresh && !preset.key}
                  onChange={(v) => update(i, { models: v })}
                />
              </label>
              <label className="field-label">
                <span>Embedding models</span>
                <ListInput
                  label={`${p.name} embedding models`}
                  value={p.embedding_models}
                  placeholder={preset.embedding}
                  onChange={(v) => update(i, { embedding_models: v })}
                />
              </label>
            </div>
            {!p.models.length && <p className="provider__note is-warn">List the chat models it serves, comma-separated — there’s nothing to answer with until you do.</p>}
          </div>
        );
      })}

      <div className="srow srow--last">
        <div>
          <p className="srow__label">Add a provider</p>
          <p className="srow__help">Picks the connection details for you; you list the models.</p>
        </div>
        <div className="srow__control chips provider-presets">
          {PRESETS.map((preset) => (
            <button key={preset.type} type="button" className="chip chip--action" onClick={() => add(preset)}>
              <PlusIcon size={11} />
              {preset.name}
            </button>
          ))}
        </div>
      </div>
    </>
  );
}

function AppearancePanel() {
  const theme = usePref("theme");
  const col = usePref("col");
  const face = usePref("face");
  const size = usePref("size");
  const leading = usePref("leading");
  const font = usePref("font");
  const rail = usePref("rail");
  const readingChanged =
    col !== PREF_DEFAULTS.col ||
    face !== PREF_DEFAULTS.face ||
    size !== PREF_DEFAULTS.size ||
    leading !== PREF_DEFAULTS.leading ||
    font !== PREF_DEFAULTS.font ||
    theme !== PREF_DEFAULTS.theme ||
    rail !== PREF_DEFAULTS.rail;

  return (
    <>
      <header>
        <p className="kicker">This browser only</p>
        <h2>Reading &amp; appearance</h2>
        <p>Libra is a reading tool before it is a chat tool. These apply as you change them — open a thread and look.</p>
      </header>

      <Row label="Theme" help="Light is the default; dark keeps the same paper-and-ink contrast.">
        <Seg
          label="Theme"
          value={theme}
          onChange={(v) => setPref("theme", v)}
          options={[
            { value: "light", label: "Light" },
            { value: "dark", label: "Dark" },
            { value: "system", label: "System" },
          ]}
        />
      </Row>
      <Row label="Answer column width" help={col ? `${col}rem. The right end fills the page.` : "Full width of the page."}>
        <input
          type="range"
          min={28}
          max={80}
          value={col || 80}
          onChange={(e) => {
            const n = Number(e.target.value);
            setPref("col", n >= 80 ? 0 : n);
          }}
          aria-label="Answer column width"
        />
      </Row>
      <Row label="Font" help="Used for the whole app. Default keeps serif titles and a sans interface.">
        <Seg
          label="Font"
          value={face}
          onChange={(v) => setPref("face", v)}
          options={[
            { value: "default", label: "Default" },
            { value: "serif", label: "Serif" },
            { value: "sans", label: "Sans" },
            { value: "mono", label: "Mono" },
          ]}
        />
      </Row>
      <Row label="Custom font" help="Optional. A family installed on this computer, such as Palatino. It replaces the typeface everywhere, including titles and the sidebar.">
        <input
          type="text"
          value={font}
          placeholder="Palatino, Georgia, serif"
          aria-label="Custom font family"
          spellCheck={false}
          onChange={(e) => setPref("font", e.target.value)}
        />
      </Row>
      <Row label="Font size" help={`${size}px. The base size for the whole app.`}>
        <input
          type="range"
          min={SIZE_MIN}
          max={SIZE_MAX}
          step={0.5}
          value={size}
          onChange={(e) => setPref("size", Number(e.target.value))}
          aria-label="Font size"
        />
      </Row>
      <Row label="Line spacing" help={`${leading.toFixed(2)}. Space between lines across the app.`}>
        <input
          type="range"
          min={LEADING_MIN}
          max={LEADING_MAX}
          step={0.05}
          value={leading}
          onChange={(e) => setPref("leading", Number(e.target.value))}
          aria-label="Line spacing"
        />
      </Row>
      <Row label="Open the sources rail by default" help="On narrow windows the rail becomes a sheet you pull up from the bottom." last>
        <Switch checked={rail} onChange={(v) => setPref("rail", v)} label="Open the sources rail by default" />
      </Row>

      <p className="meta" style={{ marginTop: 16 }}>
        Preview — the answer column uses your current settings.
      </p>
      <div className="preview">
        <div className="answer">
          <p>
            Restore time stops scaling with the guest’s configured memory and starts scaling with its working set. The second line is here so size and spacing are visible before you leave this page.
            <span className="cite">3</span>
          </p>
        </div>
      </div>
      {readingChanged && (
        <p style={{ marginTop: 12 }}>
          <button
            type="button"
            className="linkish meta"
            onClick={() => {
              setPref("theme", PREF_DEFAULTS.theme);
              setPref("col", PREF_DEFAULTS.col);
              setPref("face", PREF_DEFAULTS.face);
              setPref("size", PREF_DEFAULTS.size);
              setPref("leading", PREF_DEFAULTS.leading);
              setPref("font", PREF_DEFAULTS.font);
              setPref("rail", PREF_DEFAULTS.rail);
            }}
          >
            Reset to defaults
          </button>
        </p>
      )}
    </>
  );
}

function DataPanel() {
  const { chats, reload } = useThreads();
  const { health } = useInstance();
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");

  async function deleteAll() {
    if (!window.confirm("Delete every thread on this instance? Their answers and uploaded files go too. Settings stay as they are.")) return;
    setBusy(true);
    setNote("");
    let deleted = 0;
    try {
      // the list endpoint pages at 100
      for (let round = 0; round < 100; round++) {
        const list = await api<Chat[]>("/api/chats");
        if (!list.length) break;
        for (const c of list) {
          await deleteChat(c.id);
          deleted++;
        }
      }
      setNote(`Deleted ${plural(deleted, "thread")}.`);
    } catch (e: any) {
      setNote(`Stopped after ${plural(deleted, "thread")}: ${e.message}`);
    } finally {
      setBusy(false);
      reload();
    }
  }

  return (
    <>
      <header>
        <p className="kicker">
          {chats.length >= 100 ? "100+ threads" : plural(chats.length, "thread")} · stored in SQLite
        </p>
        <h2>Threads &amp; data</h2>
        <p>Questions, answers, their sources and uploaded files live in SQLite on this host. Nothing leaves it except the questions you send to search and model providers.</p>
      </header>

      <Row label="Result cache" help="Search results and embeddings are cached in Redis when it’s reachable. Without it everything still works, just slower.">
        <span className="meta">{health ? (health.redis ? "Redis connected" : "Off") : "—"}</span>
      </Row>
      <Row label="Version" help="The web UI build served by this instance.">
        <span className="meta">{VERSION ? `v${VERSION}` : "—"}</span>
      </Row>
      <Row label="Delete all threads" help={note || "Removes every thread, its answers and its uploaded files from this instance."} last danger>
        <button type="button" className="btn" onClick={deleteAll} disabled={busy || !chats.length}>
          {busy ? "Deleting…" : "Delete threads…"}
        </button>
      </Row>
    </>
  );
}

export default function SettingsForm() {
  const host = useHost();
  const instance = useInstance();
  const params = useSearchParams();
  const asked = params.get("tab");
  const tab: Tab = TABS.find((t) => t.id === asked)?.id ?? "search";
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [loadError, setLoadError] = useState("");
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);

  const load = useCallback(() => {
    setLoadError("");
    api<Settings>("/api/settings")
      .then((s) => {
        setSaved(s);
        setDraft(s);
      })
      .catch((e) => setLoadError(e.message));
  }, []);

  useEffect(() => {
    document.title = "Settings — Libra";
    load();
  }, [load]);

  // the tab lives in the URL so other screens can link straight to it (?tab=providers)
  const choose = (next: Tab) => window.history.replaceState(null, "", `?tab=${next}`);

  const set: SetField = useCallback((key, value) => setDraft((d) => (d ? { ...d, [key]: value } : d)), []);
  const setProviders = useCallback(
    (providers: Provider[], rename?: [string, string]) =>
      setDraft((d) => {
        if (!d) return d;
        const next = { ...d, providers };
        return reconcileModels(rename ? renameRefs(next, rename[0], rename[1]) : next);
      }),
    [],
  );
  const dirty = !!draft && !!saved && JSON.stringify(draft) !== JSON.stringify(saved);

  async function save() {
    if (!draft) return;
    setSaving(true);
    setMessage(null);
    try {
      const next = await putJson<Settings>("/api/settings", draft);
      setSaved(next);
      setDraft(next);
      instance.setSettings(next);
      instance.refresh();
      setMessage({ text: "Saved — applied without a restart." });
      window.setTimeout(() => setMessage(null), 3000);
    } catch (e: any) {
      setMessage({ text: e.message, error: true });
    } finally {
      setSaving(false);
    }
  }

  const providerNames = instance.health?.searchProviders ?? SEARCH_PROVIDERS.map((p) => p.id);
  const serverTab = tab !== "appearance" && tab !== "data";

  return (
    <>
      <Topbar
        title="Settings"
        settingsLink={false}
        actions={
          <span className="meta topbar__host">
            {host}
            {VERSION ? ` · v${VERSION}` : ""}
          </span>
        }
      />
      <div className="settings">
        <div className="settings__inner">
          <nav className="settings__nav" aria-label="Settings sections">
            <p className="kicker">Instance</p>
            <ul role="tablist" aria-orientation="vertical">
              {TABS.map((t, i) => (
                <li key={t.id}>
                  <button
                    role="tab"
                    id={`tab-${t.id}`}
                    aria-selected={tab === t.id}
                    aria-controls={`panel-${t.id}`}
                    tabIndex={tab === t.id ? 0 : -1}
                    onClick={() => choose(t.id)}
                    onKeyDown={(e) => {
                      const step = e.key === "ArrowDown" || e.key === "ArrowRight" ? 1 : e.key === "ArrowUp" || e.key === "ArrowLeft" ? -1 : 0;
                      if (!step) return;
                      e.preventDefault();
                      const next = TABS[(i + step + TABS.length) % TABS.length].id;
                      choose(next);
                      document.getElementById(`tab-${next}`)?.focus();
                    }}
                  >
                    {t.label}
                  </button>
                </li>
              ))}
            </ul>
          </nav>

          <div>
            <section className="panel" role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
              {serverTab && !draft && !loadError && <p className="meta">Loading settings…</p>}
              {serverTab && loadError && (
                <div className="callout callout--error" role="alert">
                  <b>Couldn’t load settings.</b> {loadError}
                  <div>
                    <button className="btn" onClick={load}>
                      Try again
                    </button>
                  </div>
                </div>
              )}
              {tab === "search" && draft && (
                <SearchPanel s={draft} set={set} providerNames={providerNames} redis={instance.health?.redis} />
              )}
              {tab === "answering" && draft && <AnsweringPanel s={draft} set={set} onAddProvider={() => choose("providers")} />}
              {tab === "providers" && draft && <ProvidersPanel providers={draft.providers} onChange={setProviders} />}
              {tab === "appearance" && <AppearancePanel />}
              {tab === "data" && <DataPanel />}
            </section>

            {(dirty || message) && (
              <div className="savebar" role="status">
                <span className={`savebar__msg${message?.error ? " is-error" : ""}`}>{message?.text ?? "Unsaved changes to this instance."}</span>
                <span className="topbar__spacer" />
                {dirty && (
                  <>
                    <button
                      type="button"
                      className="btn btn--quiet"
                      onClick={() => {
                        setDraft(saved);
                        setMessage(null);
                      }}
                      disabled={saving}
                    >
                      Discard
                    </button>
                    <button type="button" className="btn btn--primary" onClick={save} disabled={saving}>
                      {saving ? "Saving…" : "Save changes"}
                    </button>
                  </>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
