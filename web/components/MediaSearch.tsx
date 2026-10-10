"use client";

import { useState } from "react";
import { SearchResult, api, host } from "@/lib/api";

export default function MediaSearch({ kind, title, hint }: { kind: string; title: string; hint: string }) {
  const [q, setQ] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [provider, setProvider] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function run(query: string) {
    if (!query.trim() || busy) return;
    setBusy(true);
    setError("");
    try {
      const r = await api<{ provider: string; results: SearchResult[] }>(
        `/api/search?q=${encodeURIComponent(query)}&kind=${kind}&num_results=30`,
      );
      setResults(r.results);
      setProvider(r.provider);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="max-w-5xl mx-auto px-6 py-8">
      <h1 className="text-2xl font-semibold">{title}</h1>
      <p className="text-sm text-mute mt-1">{hint}</p>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          run(q);
        }}
        className="flex gap-2 mt-6"
      >
        <input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={kind === "videos" ? "Search videos…" : "Search images…"}
          className="flex-1"
        />
        <button className="px-4 py-2 rounded bg-accent text-bg text-sm font-medium">
          {busy ? "…" : "Search"}
        </button>
      </form>

      {error && <p className="text-sm text-red-400 mt-4">{error}</p>}
      {provider && <p className="text-xs text-mute mt-3">via {provider}</p>}

      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3 mt-4">
        {results.map((r, i) => (
          <a
            key={r.url + i}
            href={r.url}
            target="_blank"
            rel="noreferrer"
            className="border border-edge rounded overflow-hidden hover:border-accent"
          >
            {r.image || r.thumbnail ? (
              // eslint-disable-next-line @next/next/no-img-element
              <img
                src={(r.image || r.thumbnail)!}
                alt={r.title}
                loading="lazy"
                referrerPolicy="no-referrer"
                className="w-full h-36 object-cover"
              />
            ) : (
              <div className="w-full h-36 bg-edge flex items-center justify-center text-3xl">
                {kind === "videos" ? "▶" : "🖼"}
              </div>
            )}
            <div className="p-2">
              <div className="text-xs truncate">{r.title}</div>
              <div className="text-[11px] text-mute truncate">{host(r.url)}</div>
            </div>
          </a>
        ))}
      </div>
    </div>
  );
}
