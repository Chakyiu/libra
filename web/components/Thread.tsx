"use client";

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import Scope, { useAttachments, useFileDrop } from "@/components/Scope";
import { KindFilter, SheetState, SourcesPanel, SourcesSheet } from "@/components/Sources";
import Topbar from "@/components/Topbar";
import TurnView from "@/components/Turn";
import { ArrowIcon, ChevronUpIcon, RailIcon, RerunIcon } from "@/components/icons";
import { useAnswerModel, useInstance } from "@/components/instance";
import { isTyping } from "@/components/shell-context";
import { useThreads } from "@/components/threads";
import { ApiError, Chat, ChatMessage, api } from "@/lib/api";
import { citationsIn } from "@/lib/cite";
import { cleanTitle, sameQuestionKey } from "@/lib/format";
import { clearHandoff, peekHandoff } from "@/lib/handoff";
import { flash, hover } from "@/lib/lit";
import { getPref, useMedia, usePref } from "@/lib/prefs";
import { adoptHistory, getRun, startRun, stopRun, subscribeRun } from "@/lib/runs";
import { kindOf } from "@/lib/sources";
import { Turn, fromMessages, hasReferences, isStreaming, sourcesOf } from "@/lib/turn";

/** A run still in this tab can be ahead of the last row written to SQLite. */
function keepLocal(local: Turn[], server: Turn[]) {
  if (local.length > server.length) return true;
  const a = local[local.length - 1];
  const b = server[server.length - 1];
  if (!a) return false;
  if (!b) return true;
  if (a.answer.length > b.answer.length) return true;
  if (!isStreaming(a.phase) && isStreaming(b.phase)) return true;
  return false;
}

type Attachments = ReturnType<typeof useAttachments>;

function Composer({
  busy,
  held,
  onSend,
  att,
  pill,
}: {
  busy: boolean;
  /** The thread is still being read back; the box stays closed without claiming an answer is in progress. */
  held?: boolean;
  onSend: (question: string) => void;
  att: Attachments;
  pill: React.ReactNode;
}) {
  const [text, setText] = useState("");
  const box = useRef<HTMLTextAreaElement>(null);
  const refocus = useRef(false);
  const phone = useMedia("(max-width: 760px)");
  const { needsProvider } = useInstance();
  const drop = useFileDrop(att.attach, busy || !!held || att.uploading);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 140)}px`;
  }, [text]);

  // the field is disabled while answering; hand focus back if the question was typed here
  useEffect(() => {
    if (!busy && refocus.current) {
      refocus.current = false;
      box.current?.focus();
    }
  }, [busy]);

  const closed = busy || held;
  const submit = () => {
    if (closed || att.uploading || needsProvider || !text.trim()) return;
    refocus.current = document.activeElement === box.current && matchMedia("(pointer: fine)").matches;
    onSend(text);
    setText("");
  };

  return (
    <form
      className={`composer${closed ? " is-busy" : ""}${drop.over ? " is-drop" : ""}`}
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
      onDragEnter={drop.onDragEnter}
      onDragOver={drop.onDragOver}
      onDragLeave={drop.onDragLeave}
      onDrop={drop.onDrop}
    >
      {pill}
      <div className="composer__inner">
        {drop.over && <p className="drop-hint">Drop to attach</p>}
        <div className="composer__field">
          <textarea
            ref={box}
            data-composer
            rows={1}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                submit();
              }
            }}
            disabled={closed}
            placeholder={
              phone
                ? busy
                  ? "Answering…"
                  : held
                    ? "Opening…"
                    : "Ask a follow-up"
                : busy
                  ? "Libra is answering — follow-ups open when it finishes"
                  : held
                    ? "Opening thread…"
                    : "Ask a follow-up — the thread stays in context"
            }
            aria-label="Ask a follow-up"
          />
          <button className="btn btn--primary" type="submit" disabled={closed || needsProvider} aria-label="Ask">
            <span className="btn__label">Ask</span>
            <ArrowIcon />
          </button>
        </div>
        <div className="composer__foot">
          <Scope
            files={att.files}
            selected={att.selected}
            onSelect={att.setSelected}
            onAttach={att.attach}
            uploading={att.uploading}
            disabled={closed}
          />
          <span className="topbar__spacer" />
          <span className="meta composer__hint">
            Press <span className="kbd">/</span> from anywhere
          </span>
        </div>
        {att.error && <p className="composer__error">{att.error}</p>}
        {needsProvider && (
          <p className="composer__error">
            No model provider is set up. <Link href="/settings/?tab=providers">Add one in Settings</Link> to keep asking.
          </p>
        )}
      </div>
    </form>
  );
}

type Status = "loading" | "ready" | "missing" | "error";

export default function Thread({ chatId, seed }: { chatId: string; seed: string | null }) {
  const router = useRouter();
  const { chats, reload, settle, setLive } = useThreads();
  const [model] = useAnswerModel();
  const att = useAttachments(chatId);
  const narrow = useMedia("(max-width: 1100px)");
  const railDefault = usePref("rail");

  // a question asked from home arrives with its files and no history to load
  const [handoff] = useState(() => peekHandoff(chatId));
  const [resumed] = useState(() => getRun(chatId));
  const [status, setStatus] = useState<Status>(handoff || resumed?.turns.length ? "ready" : "loading");
  const [loadError, setLoadError] = useState("");
  const [chat, setChat] = useState<Chat | null>(null);
  const [turns, setTurns] = useState<Turn[]>(resumed?.turns ?? []);
  const [busy, setBusy] = useState(resumed?.busy ?? false);
  const [active, setActive] = useState(resumed?.turns.length ? resumed.turns.length - 1 : 0);
  const [kind, setKind] = useState<KindFilter>("all");
  const [railOpen, setRailOpen] = useState(true);
  const [sheet, setSheet] = useState<SheetState>("closed");
  const [readerHeight, setReaderHeight] = useState(0);
  const [scrollTarget, setScrollTarget] = useState<string | null>(null);

  const scroller = useRef<HTMLDivElement>(null);
  const railScroller = useRef<HTMLDivElement>(null);
  const turnEls = useRef(new Map<string, HTMLElement>());
  const seeded = useRef(false);
  const turnsRef = useRef(turns);
  turnsRef.current = turns;
  const activeRef = useRef(active);
  activeRef.current = active;
  const selectedRef = useRef(att.selected);
  selectedRef.current = att.selected;

  useEffect(() => setRailOpen(railDefault), [railDefault]);

  const send = useCallback(
    (q: string, fileIds: string[]) => {
      const prior = turnsRef.current.length;
      const key = startRun({
        chatId,
        question: q,
        fileIds,
        model,
        depth: getPref("depth"),
        prior: turnsRef.current,
        reload,
        settle,
        setLive,
      });
      if (!key) return;
      setActive(prior);
      setScrollTarget(key);
      setSheet("closed");
    },
    [chatId, model, reload, settle, setLive],
  );

  // history
  useEffect(() => {
    if (handoff) {
      att.setFiles(handoff.files);
      att.setSelected(handoff.selected);
      return;
    }
    let cancelled = false;
    api<{ chat: Chat; messages: ChatMessage[] }>(`/api/chats/${chatId}`)
      .then((r) => {
        if (cancelled) return;
        setChat(r.chat);
        att.setFiles(r.chat.files ?? []);
        const serverTurns = fromMessages(r.messages);
        const run = getRun(chatId);
        // SQLite already has the question and whatever has been written. A run
        // still in this tab can be a few tokens ahead of that row.
        if (run?.busy) {
          const before = run.turns.length;
          adoptHistory(chatId, serverTurns);
          setTurns(() => getRun(chatId)?.turns ?? serverTurns);
          setBusy(() => !!getRun(chatId)?.busy);
          const next = getRun(chatId);
          if (next?.busy && next.turns.length > before) setActive(next.turns.length - 1);
        } else if (run && keepLocal(run.turns, serverTurns)) {
          setTurns(run.turns);
          setBusy(false);
        } else {
          setTurns(serverTurns);
          setBusy(serverTurns.some((t) => isStreaming(t.phase)));
        }
        setStatus("ready");
      })
      .catch((e) => {
        if (cancelled) return;
        if (getRun(chatId)?.turns.length) {
          setStatus("ready");
          return;
        }
        if (e instanceof ApiError && e.status === 404) {
          setStatus(seed ? "ready" : "missing");
        } else {
          setLoadError(e.message);
          setStatus(seed ? "ready" : "error");
        }
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatId]);

  // The stream lives in the run store, so leaving this thread does not cancel it.
  useEffect(
    () =>
      subscribeRun(chatId, (run) => {
        setTurns(run.turns);
        setBusy(run.busy);
        if (run.turns.length) setStatus("ready");
      }),
    [chatId],
  );

  // After a refresh the stream is gone, but the server is still writing the turn.
  // Poll until that row reaches a terminal phase.
  const watch = turns.some((t) => isStreaming(t.phase));
  useEffect(() => {
    if (!watch || getRun(chatId)?.busy) return;
    let cancelled = false;
    const tick = async () => {
      if (getRun(chatId)?.busy) return;
      try {
        const r = await api<{ chat: Chat; messages: ChatMessage[] }>(`/api/chats/${chatId}`);
        if (cancelled || getRun(chatId)?.busy) return;
        const next = fromMessages(r.messages);
        setChat(r.chat);
        setTurns(next);
        const still = next.some((t) => isStreaming(t.phase));
        setBusy(still);
        if (!still) settle();
      } catch {
        // keep the last snapshot; the next tick tries again
      }
    };
    const id = window.setInterval(tick, 500);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [watch, chatId, settle]);

  // A question carried in the URL (from home or discover) is asked once the thread is ready.
  // Deferred a tick so a StrictMode remount cancels the timer, not the stream.
  useEffect(() => {
    if (status !== "ready" || !seed || seeded.current) return;
    const run = getRun(chatId);
    if (run?.busy || run?.turns.some((t) => t.question === seed)) {
      seeded.current = true;
      clearHandoff(chatId);
      router.replace(`/?chat=${chatId}`, { scroll: false });
      return;
    }
    const timer = window.setTimeout(() => {
      seeded.current = true;
      clearHandoff(chatId);
      router.replace(`/?chat=${chatId}`, { scroll: false });
      send(seed, handoff?.selected ?? []);
    }, 0);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status, seed]);

  // bring a newly asked question to the top of the column
  useLayoutEffect(() => {
    if (!scrollTarget) return;
    const head = turnEls.current.get(scrollTarget)?.querySelector<HTMLElement>(".ask-head");
    const box = scroller.current;
    if (head && box) box.scrollTo({ top: Math.max(0, head.offsetTop - 32), behavior: "smooth" });
    setScrollTarget(null);
  }, [scrollTarget, turns.length]);

  useEffect(() => {
    const box = scroller.current;
    if (!box) return;
    const ro = new ResizeObserver(() => setReaderHeight(box.clientHeight));
    ro.observe(box);
    return () => ro.disconnect();
  }, []);

  // the rail follows whichever question is being read
  useEffect(() => {
    const root = scroller.current;
    if (!root || turns.length < 2) return;
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          const i = Number((e.target as HTMLElement).dataset.turn);
          if (Number.isFinite(i)) setActive(i);
        }
      },
      { root, rootMargin: "-35% 0px -60% 0px" },
    );
    turnEls.current.forEach((el) => io.observe(el));
    return () => io.disconnect();
  }, [turns.length]);

  useEffect(() => setKind("all"), [active]);

  // light each source as the answer cites it
  const live = turns.find((t) => isStreaming(t.phase));
  const seen = useRef(0);
  useEffect(() => {
    if (!live) {
      seen.current = 0;
      return;
    }
    const refs = citationsIn(live.answer);
    for (const n of refs.slice(seen.current)) flash(`${live.key}:${n}`);
    seen.current = refs.length;
  }, [live?.key, live?.answer]); // eslint-disable-line react-hooks/exhaustive-deps

  const revealSource = useCallback((turnKey: string, n: number) => {
    const box = railScroller.current;
    const card = box?.querySelector<HTMLElement>(`[data-source="${turnKey}:${n}"]`);
    if (!box || !card) return;
    // the sheet at half height shows only the top of its list
    const rect = box.getBoundingClientRect();
    const visible = Math.max(80, Math.min(rect.bottom, window.innerHeight) - rect.top);
    const top = card.offsetTop - visible / 2 + card.offsetHeight / 2;
    box.scrollTo({ top: Math.max(0, Math.min(top, box.scrollHeight - box.clientHeight)), behavior: "smooth" });
  }, []);

  const onCite = useCallback(
    (index: number, n: number) => {
      const turn = turnsRef.current[index];
      if (!turn) return;
      setActive(index);
      const source = sourcesOf(turn.blocks)[n - 1];
      setKind((k) => (k !== "all" && source && kindOf(source.url) !== k ? "all" : k));
      flash(`${turn.key}:${n}`, 1600);
      if (narrow) {
        const opening = sheet === "closed";
        if (opening) setSheet("half");
        window.setTimeout(() => revealSource(turn.key, n), opening ? 320 : 30);
      } else {
        setRailOpen(true);
        window.setTimeout(() => revealSource(turn.key, n), 30);
      }
    },
    [narrow, sheet, revealSource],
  );

  const onHover = useCallback((index: number, n: number | null) => {
    if (n == null) return hover(null);
    const turn = turnsRef.current[index];
    if (!turn) return;
    if (index !== activeRef.current) setActive(index);
    hover(`${turn.key}:${n}`);
  }, []);

  const register = useCallback((key: string, el: HTMLElement | null) => {
    if (el) turnEls.current.set(key, el);
    else turnEls.current.delete(key);
  }, []);

  const followUp = useCallback((q: string) => send(q, selectedRef.current), [send]);
  const stop = useCallback(() => {
    if (stopRun(chatId)) return;
    // The stream died with the page. The server is still writing; Stop ends that.
    setTurns((ts) => ts.map((t) => (isStreaming(t.phase) ? { ...t, phase: "stopped" } : t)));
    setBusy(false);
  }, [chatId]);
  const retry = useCallback(
    (index: number) => {
      const t = turnsRef.current[index];
      if (t) send(t.question, t.files);
    },
    [send],
  );

  // `]` toggles the rail (or the sheet), Escape closes the sheet
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "Escape") setSheet("closed");
      if (e.key !== "]" || isTyping()) return;
      e.preventDefault();
      if (narrow) setSheet((s) => (s === "closed" ? "half" : "closed"));
      else setRailOpen((o) => !o);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [narrow]);

  const listed = chats.find((c) => c.id === chatId);
  const title = cleanTitle(listed?.title || chat?.title) || turns[0]?.question || (status === "loading" ? "" : "New thread");
  useEffect(() => {
    document.title = title ? `${title} — Libra` : "Libra";
  }, [title]);

  const fileNames = useMemo(() => new Map(att.files.map((f) => [f.id, f.name])), [att.files]);
  const questions = turns.map((t) => t.question).join("\n");
  const asked = useMemo(() => new Set(questions.split("\n").map(sameQuestionKey)), [questions]);
  const current = turns[Math.min(active, turns.length - 1)];
  const sourceCount = current ? sourcesOf(current.blocks).length : 0;
  const retrieving = !!current && isStreaming(current.phase) && !hasReferences(current.blocks);
  const showRail = !narrow && railOpen && !!current;

  const panel = current && (
    <SourcesPanel
      turn={current}
      index={Math.min(active, turns.length - 1)}
      total={turns.length}
      kind={kind}
      onKind={setKind}
      scrollRef={railScroller}
      fileNames={fileNames}
    />
  );

  const last = turns[turns.length - 1];

  return (
    <>
      <Topbar
        title={title}
        actions={
          <>
            {!narrow && turns.length > 0 && (
              <button className="btn btn--quiet" aria-pressed={railOpen} onClick={() => setRailOpen((o) => !o)} title="Sources  ]">
                <RailIcon />
                <span className="btn__label">Sources</span>
              </button>
            )}
            <button className="btn btn--quiet" onClick={() => last && send(last.question, last.files)} disabled={busy || status === "loading" || !last} title="Ask the last question again">
              <RerunIcon />
              <span className="btn__label">Re-run</span>
            </button>
          </>
        }
      />
      <div className="workspace" data-rail={showRail ? "open" : "closed"}>
        <section className="reader" aria-label="Answer">
          <div className="reader__scroll" ref={scroller}>
            <div className="reader__inner">
              {status === "loading" && <p className="meta">Opening thread…</p>}
              {status === "missing" && (
                <header className="ask-head">
                  <p className="kicker">Not found</p>
                  <h1>This thread isn’t here anymore</h1>
                  <p className="meta">
                    It may have been deleted. <Link href="/">Start a new thread</Link>.
                  </p>
                </header>
              )}
              {status === "error" && !turns.length && (
                <p className="callout callout--error" role="alert">
                  <b>Couldn’t open this thread.</b> {loadError}
                </p>
              )}
              {turns.map((t, i) => (
                <TurnView
                  key={t.key}
                  turn={t}
                  index={i}
                  last={i === turns.length - 1}
                  busy={busy}
                  fileNames={fileNames}
                  asked={asked}
                  minHeight={i > 0 && i === turns.length - 1 && readerHeight ? readerHeight - 24 : undefined}
                  register={register}
                  onCite={onCite}
                  onHover={onHover}
                  onFollowUp={followUp}
                  onStop={stop}
                  onRetry={retry}
                />
              ))}
            </div>
          </div>

          <Composer
            busy={busy}
            held={status === "loading"}
            onSend={(q) => send(q, att.selected)}
            att={att}
            pill={
              narrow && current && sheet === "closed" ? (
                <button type="button" className="sources-pill" onClick={() => setSheet("half")}>
                  Sources <b>{retrieving ? "…" : sourceCount}</b>
                  <ChevronUpIcon />
                </button>
              ) : null
            }
          />
        </section>

        {showRail && (
          <aside className="rail" aria-label="Sources">
            {panel}
          </aside>
        )}
      </div>

      {narrow && current && (
        <SourcesSheet state={sheet} onState={setSheet}>
          {panel}
        </SourcesSheet>
      )}
    </>
  );
}
