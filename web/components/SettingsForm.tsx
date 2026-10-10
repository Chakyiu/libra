"use client";

import { useEffect, useState } from "react";
import { api } from "@/lib/api";

type Provider = {
  id: string;
  name: string;
  type: string;
  base_url: string;
  api_key: string;
  models: string[];
  embedding_models: string[];
  built_in?: boolean;
};

const PROVIDER_TYPES = ["ollama", "open_ai", "open_router", "anthropic", "gemini", "open_ai_compatible", "custom"];
const WIDGETS = ["calculator", "time", "weather", "stocks"];

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="block text-xs uppercase tracking-wide text-mute mb-1">{label}</span>
      {children}
    </label>
  );
}

function Toggle({
  label,
  on,
  onChange,
}: {
  label: string;
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="flex items-center gap-2 text-sm py-1">
      <input type="checkbox" checked={on} onChange={(e) => onChange(e.target.checked)} />
      {label}
    </label>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="border border-edge bg-panel/50 rounded-lg p-5">
      <h2 className="font-semibold mb-3">{title}</h2>
      {children}
    </section>
  );
}

export default function SettingsForm() {
  const [s, setS] = useState<any>(null);
  const [models, setModels] = useState<{ chat: string[]; embedding: string[] }>({ chat: [], embedding: [] });
  const [providers, setProviders] = useState<Provider[]>([]);
  const [status, setStatus] = useState("");
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    api("/api/settings").then((v) => {
      setS(v);
      setProviders(v.providers ?? []);
    });
    api("/api/models").then(setModels);
  }, []);

  if (!s) return <p className="text-sm text-mute">Loading settings…</p>;

  const set = (key: string, value: any) => {
    setS({ ...s, [key]: value });
    setDirty(true);
  };

  const setProvider = (i: number, patch: Partial<Provider>) => {
    setProviders(providers.map((p, j) => (j === i ? { ...p, ...patch } : p)));
    setDirty(true);
  };

  const order = (key: string, value: string) =>
    set("searchOrder", { ...(s.searchOrder ?? {}), [key]: value.split(",").map((x) => x.trim()).filter(Boolean) });

  async function save() {
    setStatus("saving…");
    try {
      const saved = await api("/api/settings", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...s, providers }),
      });
      setS(saved);
      setProviders(saved.providers ?? []);
      setDirty(false);
      setStatus("saved");
    } catch (e: any) {
      setStatus(`error: ${e.message}`);
    }
    setTimeout(() => setStatus(""), 4000);
  }

  return (
    <div className="max-w-3xl mx-auto px-6 py-8 space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Settings</h1>
        <div className="flex items-center gap-3">
          <span className="text-xs text-mute">{status}</span>
          <button
            onClick={save}
            disabled={!dirty}
            className="px-4 py-2 rounded bg-accent text-bg text-sm font-medium disabled:opacity-40"
          >
            Save
          </button>
        </div>
      </div>

      <Section title="Models">
        <div className="grid sm:grid-cols-2 gap-4">
          <Field label="Answer model">
            <select value={s.defaultModel} onChange={(e) => set("defaultModel", e.target.value)}>
              {models.chat.map((m) => (
                <option key={m}>{m}</option>
              ))}
            </select>
          </Field>
          <Field label="Fast model (classify, rerank, titles)">
            <select value={s.defaultLightModel} onChange={(e) => set("defaultLightModel", e.target.value)}>
              {models.chat.map((m) => (
                <option key={m}>{m}</option>
              ))}
            </select>
          </Field>
          <Field label="Rerank model">
            <select value={s.defaultRerankModel} onChange={(e) => set("defaultRerankModel", e.target.value)}>
              {models.chat.map((m) => (
                <option key={m}>{m}</option>
              ))}
            </select>
          </Field>
          <Field label="Embedding model (uploads)">
            <select
              value={s.defaultEmbeddingModel}
              onChange={(e) => set("defaultEmbeddingModel", e.target.value)}
            >
              {models.embedding.map((m) => (
                <option key={m}>{m}</option>
              ))}
            </select>
          </Field>
          <Field label="Temperature">
            <input
              type="number"
              step="0.1"
              min="0"
              max="2"
              value={s.defaultTemperature}
              onChange={(e) => set("defaultTemperature", Number(e.target.value))}
            />
          </Field>
        </div>
      </Section>

      <Section title="Model providers">
        <div className="space-y-4">
          {providers.map((p, i) => (
            <div key={p.id} className="border border-edge rounded p-3 grid sm:grid-cols-2 gap-3">
              <Field label="Name">
                <input value={p.name} onChange={(e) => setProvider(i, { name: e.target.value })} />
              </Field>
              <Field label="Type">
                <select value={p.type} onChange={(e) => setProvider(i, { type: e.target.value })}>
                  {PROVIDER_TYPES.map((t) => (
                    <option key={t}>{t}</option>
                  ))}
                </select>
              </Field>
              <Field label="Base URL">
                <input value={p.base_url} onChange={(e) => setProvider(i, { base_url: e.target.value })} />
              </Field>
              <Field label="API key">
                <input
                  type="password"
                  value={p.api_key}
                  placeholder="not set"
                  onChange={(e) => setProvider(i, { api_key: e.target.value })}
                />
              </Field>
              <Field label="Chat models (comma separated)">
                <input
                  value={p.models.join(", ")}
                  onChange={(e) => setProvider(i, { models: e.target.value.split(",").map((x) => x.trim()).filter(Boolean) })}
                />
              </Field>
              <Field label="Embedding models">
                <input
                  value={p.embedding_models.join(", ")}
                  onChange={(e) =>
                    setProvider(i, { embedding_models: e.target.value.split(",").map((x) => x.trim()).filter(Boolean) })
                  }
                />
              </Field>
              <div className="sm:col-span-2 flex items-center justify-between text-xs text-mute">
                <span>id: {p.id}</span>
                {!p.built_in && (
                  <button
                    onClick={() => setProviders(providers.filter((_, j) => j !== i))}
                    className="text-red-400 hover:underline"
                  >
                    remove
                  </button>
                )}
              </div>
            </div>
          ))}
          <button
            onClick={() =>
              setProviders([
                ...providers,
                {
                  id: `custom${providers.length + 1}`,
                  name: "Custom",
                  type: "open_ai_compatible",
                  base_url: "http://127.0.0.1:8080/v1",
                  api_key: "",
                  models: [],
                  embedding_models: [],
                },
              ])
            }
            className="text-sm text-accent hover:underline"
          >
            + add provider
          </button>
        </div>
      </Section>

      <Section title="Search">
        <div className="grid sm:grid-cols-2 gap-4">
          <Field label="Web provider order">
            <input value={(s.searchOrder?.web ?? []).join(", ")} onChange={(e) => order("web", e.target.value)} />
          </Field>
          <Field label="News provider order">
            <input value={(s.searchOrder?.news ?? []).join(", ")} onChange={(e) => order("news", e.target.value)} />
          </Field>
          <Field label="Images provider order">
            <input value={(s.searchOrder?.images ?? []).join(", ")} onChange={(e) => order("images", e.target.value)} />
          </Field>
          <Field label="Videos provider order">
            <input value={(s.searchOrder?.videos ?? []).join(", ")} onChange={(e) => order("videos", e.target.value)} />
          </Field>
          <Field label="SearxNG URL">
            <input value={s.searxngUrl} onChange={(e) => set("searxngUrl", e.target.value)} />
          </Field>
          <Field label="SearxNG JSON API password">
            <input
              type="password"
              value={s.searxngPassword}
              onChange={(e) => set("searxngPassword", e.target.value)}
            />
          </Field>
          <Field label="Brave API key">
            <input type="password" value={s.braveApiKey} onChange={(e) => set("braveApiKey", e.target.value)} />
          </Field>
          <Field label="Exa API key">
            <input type="password" value={s.exaApiKey} onChange={(e) => set("exaApiKey", e.target.value)} />
          </Field>
          <Field label="Tavily API key">
            <input type="password" value={s.tavilyApiKey} onChange={(e) => set("tavilyApiKey", e.target.value)} />
          </Field>
          <Field label="Firecrawl API key">
            <input type="password" value={s.firecrawlApiKey} onChange={(e) => set("firecrawlApiKey", e.target.value)} />
          </Field>
          <Field label="Results per search">
            <input
              type="number"
              min="1"
              max="50"
              value={s.numWebResults}
              onChange={(e) => set("numWebResults", Number(e.target.value))}
            />
          </Field>
          <Field label="Rerank keep (top N)">
            <input
              type="number"
              min="1"
              max="20"
              value={s.rerankLimit}
              onChange={(e) => set("rerankLimit", Number(e.target.value))}
            />
          </Field>
          <Field label="Country">
            <input value={s.country} onChange={(e) => set("country", e.target.value)} />
          </Field>
          <Field label="Language">
            <input value={s.language} onChange={(e) => set("language", e.target.value)} />
          </Field>
        </div>
        <div className="grid sm:grid-cols-2 gap-x-6 mt-3">
          <Toggle label="Rerank results" on={s.enableReranker} onChange={(v) => set("enableReranker", v)} />
          <Toggle label="Image search" on={s.enableImageSearch} onChange={(v) => set("enableImageSearch", v)} />
          <Toggle label="Video search" on={s.enableVideoSearch} onChange={(v) => set("enableVideoSearch", v)} />
          <Toggle
            label="Prefer fresh results"
            on={s.enableFreshSearch}
            onChange={(v) => set("enableFreshSearch", v)}
          />
        </div>
      </Section>

      <Section title="Widgets & personalization">
        <p className="text-xs text-mute mb-2">Uncheck to disable a widget for every answer.</p>
        <div className="grid sm:grid-cols-2 gap-x-6">
          {WIDGETS.map((w) => (
            <Toggle
              key={w}
              label={w}
              on={!(s.disabledWidgets ?? []).includes(w)}
              onChange={(v) =>
                set("disabledWidgets", v ? (s.disabledWidgets ?? []).filter((x: string) => x !== w) : [...(s.disabledWidgets ?? []), w])
              }
            />
          ))}
          <Toggle
            label="AI image generation"
            on={s.imageGenerationEnabled}
            onChange={(v) => set("imageGenerationEnabled", v)}
          />
        </div>
        <div className="mt-4">
          <Field label="Custom instructions (appended to the answer prompt)">
            <textarea
              rows={3}
              value={s.customInstructions}
              onChange={(e) => set("customInstructions", e.target.value)}
            />
          </Field>
        </div>
      </Section>
    </div>
  );
}
