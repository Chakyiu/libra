"use client";

import { memo, useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import Answer from "@/components/Answer";
import { MediaStrip, Widget } from "@/components/Widgets";
import { CopyIcon, DownloadIcon } from "@/components/icons";
import type { SearchResult } from "@/lib/api";
import { answerMarkdown } from "@/lib/cite";
import { askedAt, modelLabel, plural, sameQuestionKey, seconds, slug, toDate } from "@/lib/format";
import { Turn, hasReferences, isStreaming, mediaOf, relatedOf, researchOf, sourcesOf, statsOf, widgetsOf } from "@/lib/turn";

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // clipboard API needs a secure context; fall back for plain-http installs
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
}

function download(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/markdown;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function Thinking({ text, live }: { text: string; live: boolean }) {
  const box = useRef<HTMLPreElement>(null);
  const [open, setOpen] = useState(live);
  const wasLive = useRef(live);
  useLayoutEffect(() => {
    if (wasLive.current !== live) {
      wasLive.current = live;
      setOpen(live);
    }
  }, [live]);
  useLayoutEffect(() => {
    const el = box.current;
    if (el && open) el.scrollTop = el.scrollHeight;
  }, [text, open]);
  if (!text.trim()) return null;
  return (
    <details
      className="think"
      open={open}
      onToggle={(e) => {
        const next = e.currentTarget.open;
        if (next !== open) setOpen(next);
      }}
    >
      <summary>Thinking</summary>
      <pre ref={box}>{text}</pre>
    </details>
  );
}

function Stages({ turn, count, onStop }: { turn: Turn; count: number; onStop: () => void }) {
  const p = turn.phase;
  const docs = turn.files.length > 0;
  const found = hasReferences(turn.blocks);
  const skipped = turn.blocks.some((b) => b.type === "answered" && b.data?.need_web_search === false);

  const research = researchOf(turn.blocks);
  const planning = p === "search" && !docs && !skipped && research.queries.length === 0;

  let reading = "Reading sources";
  if (docs) reading = p === "write" ? "Read your files" : "Reading your files";
  else if (found && count === 0) reading = skipped ? "No search needed" : "No sources found";
  else if (found) reading = p === "write" ? `Read ${plural(count, "source")}` : `Reading ${plural(count, "source")}`;

  return (
    <div className="stages" aria-live="polite">
      <span className="stage" data-state={p === "search" ? "active" : "done"}>
        <i className="stage__led" />
        {turn.locating
          ? "Getting your location"
          : docs
            ? "Searching your files"
            : planning
              ? "Deciding what to search"
              : !found && research.step === "filter"
                ? "Filtering sources"
                : !found && research.step === "rerank"
                  ? "Reranking sources"
                  : "Searching the web"}
      </span>
      <span className="stage__sep">/</span>
      <span className="stage" data-state={p === "search" ? "idle" : p === "read" ? "active" : "done"}>
        <i className="stage__led" />
        {reading}
      </span>
      <span className="stage__sep">/</span>
      <span className="stage" data-state={p === "write" ? "active" : "idle"}>
        <i className="stage__led" />
        Writing
      </span>
      <span className="topbar__spacer" />
      <button className="btn btn--quiet" onClick={onStop}>
        Stop
      </button>
    </div>
  );
}

function Actions({ turn, sources }: { turn: Turn; sources: SearchResult[] }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="answer-actions">
      <button
        className="btn btn--quiet"
        onClick={async () => {
          await copyText(answerMarkdown(turn.answer, sources));
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1600);
        }}
      >
        <CopyIcon />
        {copied ? "Copied" : "Copy with citations"}
      </button>
      <button className="btn btn--quiet" onClick={() => download(`${slug(turn.question)}.md`, answerMarkdown(turn.answer, sources, turn.question))}>
        <DownloadIcon />
        Export .md
      </button>
    </div>
  );
}

type TurnProps = {
  turn: Turn;
  index: number;
  last: boolean;
  busy: boolean;
  fileNames: Map<string, string>;
  /** questions already asked in this thread (sameQuestionKey), so suggestions don't repeat them */
  asked: Set<string>;
  minHeight?: number;
  register: (key: string, el: HTMLElement | null) => void;
  onCite: (index: number, n: number) => void;
  onHover: (index: number, n: number | null) => void;
  onFollowUp: (question: string) => void;
  onStop: () => void;
  onRetry: (index: number) => void;
};

function TurnView({ turn, index, last, busy, fileNames, asked, minHeight, register, onCite, onHover, onFollowUp, onStop, onRetry }: TurnProps) {
  const sources = useMemo(() => sourcesOf(turn.blocks), [turn.blocks]);
  const streaming = isStreaming(turn.phase);
  const stats = statsOf(turn);
  const related = relatedOf(turn.blocks).filter((q) => !asked.has(sameQuestionKey(q)));
  const widgets = widgetsOf(turn.blocks);
  const images = mediaOf(turn.blocks, "images");
  const videos = mediaOf(turn.blocks, "videos");
  const docs = turn.files.length > 0;
  const research = researchOf(turn.blocks);
  const depthName = research.depth === "fast" ? "Fast" : research.depth === "deep" ? "Deep" : research.depth ? "Balance" : "";
  const Question = index === 0 ? "h1" : "h2";

  const cite = useCallback((n: number) => onCite(index, n), [onCite, index]);
  const hoverCite = useCallback((n: number | null) => onHover(index, n), [onHover, index]);
  const ref = useCallback((el: HTMLElement | null) => register(turn.key, el), [register, turn.key]);

  const searched =
    !docs && stats.searchQuery && sameQuestionKey(stats.searchQuery) !== sameQuestionKey(turn.question) ? stats.searchQuery : "";
  const fileList = turn.files.map((id) => fileNames.get(id)).filter(Boolean).join(", ");

  return (
    <section className="turn" data-turn={index} ref={ref} style={minHeight ? { minHeight } : undefined}>
      <header className="ask-head">
        <p className="kicker">{streaming ? "Asked just now" : askedAt(toDate(turn.askedAt))} · you</p>
        <Question className={index ? "ask-head__q" : undefined}>{turn.question}</Question>
        {!streaming && turn.phase !== "error" && (
          <div className="ask-head__meta">
            {docs ? (
              <span className="meta" title={fileList}>
                {plural(turn.files.length, "file")}
              </span>
            ) : (
              hasReferences(turn.blocks) && <span className="meta">{plural(sources.length, "source")}</span>
            )}
            {stats.elapsedMs != null && (
              <span className="meta">
                {seconds(stats.elapsedMs)}
                {stats.model ? ` · ${modelLabel(stats.model)}` : ""}
              </span>
            )}
            {research.queries.length > 0 ? (
              <span className="meta" title={research.queries.join("\n")}>
                {depthName} · {plural(research.queries.length, "search")}
              </span>
            ) : (
              searched && (
                <span className="meta" title="What the engine searched for">
                  searched “{searched}”
                </span>
              )
            )}
            {turn.phase === "stopped" && <span className="meta">stopped early</span>}
          </div>
        )}
      </header>

      {streaming && <Stages turn={turn} count={sources.length} onStop={onStop} />}
      {streaming && research.queries.length > 0 && (
        <div className="research">
          <p className="kicker">
            {depthName}
            {research.rounds > 1 ? ` · round ${research.round} of ${research.rounds}` : ""}
          </p>
          <ul>
            {research.queries.map((q) => (
              <li key={q}>{q}</li>
            ))}
          </ul>
        </div>
      )}
      <Thinking text={turn.thought || ""} live={streaming} />

      {widgets.map((b, i) => (
        <Widget key={i} block={b} />
      ))}
      {images.length > 0 && <MediaStrip items={images} kind="images" />}
      {videos.length > 0 && <MediaStrip items={videos} kind="videos" />}

      {turn.answer && (
        <Answer text={turn.answer} writing={turn.phase === "write"} turnKey={turn.key} count={sources.length} onCite={cite} onHover={hoverCite} />
      )}

      {turn.phase === "error" && (
        <div className="callout callout--error" role="alert">
          <b>This answer didn’t finish.</b> {turn.error}
          {last && (
            <div>
              <button className="btn" onClick={() => onRetry(index)} disabled={busy}>
                Try again
              </button>
            </div>
          )}
        </div>
      )}
      {turn.phase === "done" && !turn.answer && !widgets.length && <p className="callout">No answer came back for this question.</p>}

      {!streaming && turn.answer && <Actions turn={turn} sources={sources} />}

      {last && !streaming && related.length > 0 && (
        <nav className="followups" aria-label="Follow-up questions">
          <p className="kicker">Keep pulling the thread</p>
          <ul>
            {related.map((q) => (
              <li key={q}>
                <button type="button" onClick={() => onFollowUp(q)} disabled={busy}>
                  {q}
                </button>
              </li>
            ))}
          </ul>
        </nav>
      )}
    </section>
  );
}

export default memo(TurnView);
