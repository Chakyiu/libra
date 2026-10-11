"use client";

import { useEffect, useState } from "react";
import Topbar from "@/components/Topbar";
import { ArrowIcon } from "@/components/icons";
import { SearchResult, api } from "@/lib/api";
import { host, plural } from "@/lib/format";

type Kind = "images" | "videos";

export default function MediaSearch({ kind, title, hint }: { kind: Kind; title: string; hint: string }) {
  const [q, setQ] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [provider, setProvider] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [searched, setSearched] = useState(false);

  useEffect(() => {
    document.title = `${title} — Libra`;
  }, [title]);

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
      setSearched(true);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Topbar title={title} />
      <div className="page">
        <div className="page__inner page__inner--wide">
          <header className="page__head">
            <p className="kicker">Search</p>
            <h2>{title}</h2>
            <p>{hint}</p>
          </header>

          <form
            className="searchbar"
            role="search"
            onSubmit={(e) => {
              e.preventDefault();
              run(q);
            }}
          >
            <input
              type="search"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder={kind === "videos" ? "Search videos…" : "Search images…"}
              aria-label={`Search ${kind}`}
              autoFocus
            />
            <button className="btn btn--primary" type="submit" disabled={busy}>
              {busy ? "Searching…" : "Search"}
              <ArrowIcon />
            </button>
          </form>

          {error && (
            <p className="callout callout--error" style={{ marginTop: 20 }} role="alert">
              <b>Search failed.</b> {error}
            </p>
          )}
          {searched && !error && (
            <p className="meta results-meta">
              {results.length ? `${plural(results.length, "result")} via ${provider}` : `Nothing found${provider && provider !== "none" ? ` via ${provider}` : " — no provider returned results"}.`}
            </p>
          )}

          <div className="gallery">
            {results.map((r, i) => {
              const src = r.thumbnail || r.image;
              return (
                <a key={r.url + i} href={r.url} target="_blank" rel="noreferrer noopener">
                  <span className="gallery__img">
                    {/* eslint-disable-next-line @next/next/no-img-element */}
                    {src && <img src={src} alt={r.title} loading="lazy" referrerPolicy="no-referrer" />}
                    {kind === "videos" && <span className="media__play">Play</span>}
                  </span>
                  <span className="gallery__title">{r.title || host(r.url)}</span>
                  <span className="gallery__domain">{host(r.url)}</span>
                </a>
              );
            })}
          </div>
        </div>
      </div>
    </>
  );
}
