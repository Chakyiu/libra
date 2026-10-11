"use client";

import { memo, useMemo, useRef } from "react";
import type { SearchResult } from "@/lib/api";
import { host, plural, shortDate } from "@/lib/format";
import { hover, useLit } from "@/lib/lit";
import { KIND_CLASS, KIND_LABEL, KIND_TAG, SourceKind, kindOf, kindsIn } from "@/lib/sources";
import { Turn, hasReferences, isStreaming, sourcesOf } from "@/lib/turn";

export type KindFilter = SourceKind | "all";
export type SheetState = "closed" | "half" | "full";

const SourceCard = memo(function SourceCard({
  source,
  n,
  turnKey,
  delay,
}: {
  source: SearchResult;
  n: number;
  turnKey: string;
  /** stagger (ms) for cards arriving while the answer streams */
  delay?: number;
}) {
  const key = `${turnKey}:${n}`;
  const lit = useLit(key);
  const kind = kindOf(source.url);
  const snippet = (source.snippet || source.content || "").replace(/\s+/g, " ").trim();
  const date = shortDate(source.published_at);

  return (
    <a
      className={`source${lit ? " is-lit" : ""}${delay != null ? " reveal" : ""}`}
      style={delay != null ? { animationDelay: `${delay}ms` } : undefined}
      data-source={key}
      href={source.url}
      target="_blank"
      rel="noreferrer noopener"
      onMouseEnter={() => hover(key)}
      onMouseLeave={() => hover(null)}
      onFocus={() => hover(key)}
      onBlur={() => hover(null)}
    >
      <div className="source__top">
        <span className="source__n">{n}</span>
        <h3 className="source__title">{source.title || host(source.url)}</h3>
      </div>
      {snippet && <p className="source__snip">{snippet}</p>}
      <div className="source__foot">
        <span className={KIND_CLASS[kind]}>{KIND_TAG[kind]}</span>
        <span className="source__domain">
          {host(source.url)}
          {date ? ` · ${date}` : ""}
        </span>
      </div>
    </a>
  );
});

function Empty({ turn, fileNames }: { turn: Turn; fileNames: Map<string, string> }) {
  if (turn.files.length) {
    const names = turn.files.map((id) => fileNames.get(id)).filter(Boolean);
    return (
      <p className="rail__empty">
        Answered from {names.length ? <b>{names.join(", ")}</b> : "your files"}. Passages from uploaded files are cited by number but aren’t
        listed here.
      </p>
    );
  }
  if (turn.phase === "error") return <p className="rail__empty">The answer stopped before any sources came back.</p>;
  const skipped = turn.blocks.some((b) => b.type === "answered" && b.data?.need_web_search === false);
  return (
    <p className="rail__empty">
      {skipped
        ? "This question didn’t need a search, so there are no sources to check it against."
        : "No sources came back for this question. The answer was written from the model alone — treat it as unverified."}
    </p>
  );
}

type PanelProps = {
  turn: Turn;
  index: number;
  total: number;
  kind: KindFilter;
  onKind: (kind: KindFilter) => void;
  scrollRef: React.Ref<HTMLDivElement>;
  fileNames: Map<string, string>;
};

/** Head + list for one turn's sources; the same object sits in the rail and in the narrow-window sheet. */
export function SourcesPanel({ turn, index, total, kind, onKind, scrollRef, fileNames }: PanelProps) {
  const sources = useMemo(() => sourcesOf(turn.blocks), [turn.blocks]);
  const kinds = useMemo(() => kindsIn(sources.map((s) => s.url)), [sources]);
  const streaming = isStreaming(turn.phase);
  const retrieving = streaming && !hasReferences(turn.blocks);
  const shown = sources.map((s, i) => ({ s, n: i + 1 })).filter(({ s }) => kind === "all" || kindOf(s.url) === kind);

  return (
    <>
      <div className="rail__head">
        <div className="rail__headline">
          <p className="kicker">Sources</p>
          <span className="meta">{retrieving ? "retrieving…" : plural(shown.length, "source")}</span>
        </div>
        {total > 1 && (
          <p className="rail__for" title={turn.question}>
            Q{index + 1} · {turn.question}
          </p>
        )}
        {kinds.length > 1 && (
          <div className="chips" role="group" aria-label="Filter sources">
            <button type="button" className="chip" aria-pressed={kind === "all"} onClick={() => onKind("all")}>
              All
            </button>
            {kinds.map((k) => (
              <button key={k} type="button" className="chip" aria-pressed={kind === k} onClick={() => onKind(k)}>
                {KIND_LABEL[k]}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="rail__scroll" ref={scrollRef}>
        {retrieving &&
          [0, 1, 2, 3].map((i) => (
            <div className="skeleton" key={i}>
              <i />
              <i />
              <i />
            </div>
          ))}
        {!retrieving && !sources.length && <Empty turn={turn} fileNames={fileNames} />}
        {shown.map(({ s, n }) => (
          <SourceCard key={`${turn.key}:${n}`} source={s} n={n} turnKey={turn.key} delay={streaming ? (n - 1) * 90 : undefined} />
        ))}
      </div>
    </>
  );
}

const DETENT: Record<SheetState, number> = { full: 0, half: 0.46, closed: 1 };

/** On narrow windows the rail folds into a sheet with three detents: closed (just the pill), half, full. */
export function SourcesSheet({
  state,
  onState,
  children,
}: {
  state: SheetState;
  onState: (s: SheetState) => void;
  children: React.ReactNode;
}) {
  const sheet = useRef<HTMLElement>(null);
  const drag = useRef<{ y: number; start: number; moved: boolean } | null>(null);
  const dragged = useRef(false);

  const offset = (s: SheetState, h: number) => DETENT[s] * h;

  const onDown = (e: React.PointerEvent<HTMLButtonElement>) => {
    const el = sheet.current;
    if (!el) return;
    drag.current = { y: e.clientY, start: offset(state, el.offsetHeight), moved: false };
    el.classList.add("is-dragging");
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    const el = sheet.current;
    if (!d || !el) return;
    const dy = e.clientY - d.y;
    if (Math.abs(dy) > 4) d.moved = true;
    el.style.transform = `translateY(${Math.max(0, Math.min(el.offsetHeight, d.start + dy))}px)`;
  };

  const onUp = (e: React.PointerEvent) => {
    const d = drag.current;
    const el = sheet.current;
    drag.current = null;
    if (!d || !el) return;
    el.classList.remove("is-dragging");
    el.style.transform = "";
    if (!d.moved) return;
    dragged.current = true;
    const h = el.offsetHeight;
    const y = Math.max(0, Math.min(h, d.start + (e.clientY - d.y)));
    const states: SheetState[] = ["full", "half", "closed"];
    onState(states.reduce((best, s) => (Math.abs(offset(s, h) - y) < Math.abs(offset(best, h) - y) ? s : best)));
  };

  // taps and the keyboard cycle half → full → closed; a drag already chose its detent
  const onClick = () => {
    if (dragged.current) {
      dragged.current = false;
      return;
    }
    onState(state === "half" ? "full" : state === "full" ? "closed" : "half");
  };

  return (
    <>
      <div className={`sheet-scrim${state !== "closed" ? " is-on" : ""}`} onClick={() => onState("closed")} />
      <section className="sheet" data-state={state} ref={sheet} aria-label="Sources" inert={state === "closed"}>
        <button
          type="button"
          className="sheet__grab"
          aria-label={state === "full" ? "Close sources" : "Expand sources"}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={onUp}
          onClick={onClick}
        >
          <i />
        </button>
        {children}
      </section>
    </>
  );
}
