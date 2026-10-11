"use client";

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import Scope, { useAttachments } from "@/components/Scope";
import { KindFilter, SheetState, SourcesPanel, SourcesSheet } from "@/components/Sources";
import Topbar from "@/components/Topbar";
import TurnView from "@/components/Turn";
import { ArrowIcon, ChevronUpIcon, RailIcon, RerunIcon } from "@/components/icons";
import { useAnswerModel, useInstance } from "@/components/instance";
import { isTyping } from "@/components/shell-context";
import { useThreads } from "@/components/threads";
import { ApiError, Chat, ChatMessage, api, ask, uuid } from "@/lib/api";
import { citationsIn } from "@/lib/cite";
import { cleanTitle, sameQuestionKey } from "@/lib/format";
import { clearHandoff, peekHandoff } from "@/lib/handoff";
import { flash, hover } from "@/lib/lit";
import { useMedia, usePref } from "@/lib/prefs";
import { kindOf } from "@/lib/sources";
import { Turn, fromMessages, hasReferences, isStreaming, sourcesOf } from "@/lib/turn";

type Attachments = ReturnType<typeof useAttachments>;

function Composer({
  busy,
  onSend,
  att,
  pill,
}: {
  busy: boolean;
  onSend: (question: string) => void;
  att: Attachments;
  pill: React.ReactNode;
}) {
  const [text, setText] = useState("");
  const box = useRef<HTMLTextAreaElement>(null);
  const refocus = useRef(false);
  const phone = useMedia("(max-width: 760px)");
  const { needsProvider } = useInstance();

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

  const submit = () => {
    if (busy || att.uploading || needsProvider || !text.trim()) return;
    refocus.current = document.activeElement === box.current && matchMedia("(pointer: fine)").matches;
    onSend(text);
    setText("");
  };

  return (
    <form
      className={`composer${busy ? " is-busy" : ""}`}
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      {pill}
      <div className="composer__inner">
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
            disabled={busy}
            placeholder={
              phone
                ? busy
                  ? "Answering…"
                  : "Ask a follow-up"
                : busy
                  ? "Libra is answering — follow-ups open when it finishes"
                  : "Ask a follow-up — the thread stays in context"
            }
            aria-label="Ask a follow-up"
          />
          <button className="btn btn--primary" type="submit" disabled={busy || needsProvider} aria-label="Ask">
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
            disabled={busy}
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
  const [status, setStatus] = useState<Status>(handoff ? "ready" : "loading");
  const [loadError, setLoadError] = useState("");
  const [chat, setChat] = useState<Chat | null>(null);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [busy, setBusy] = useState(false);
  const [active, setActive] = useState(0);
  const [kind, setKind] = useState<KindFilter>("all");
  const [railOpen, setRailOpen] = useState(true);
  const [sheet, setSheet] = useState<SheetState>("closed");
  const [readerHeight, setReaderHeight] = useState(0);
  const [scrollTarget, setScrollTarget] = useState<string | null>(null);

  const scroller = useRef<HTMLDivElement>(null);
  const railScroller = useRef<HTMLDivElement>(null);
  const turnEls = useRef(new Map<string, HTMLElement>());
  const abort = useRef<AbortController | null>(null);
  const seeded = useRef(false);
  const turnsRef = useRef(turns);
  turnsRef.current = turns;
  const activeRef = useRef(active);
  activeRef.current = active;
  const selectedRef = useRef(att.selected);
  selectedRef.current = att.selected;

  useEffect(() => setRailOpen(railDefault), [railDefault]);

  const send = useCallback(
    async (q: string, fileIds: string[]) => {
      const question = q.trim();
      if (!question || abort.current) return;
      const prior = turnsRef.current;
      const history = prior
        .filter((t) => t.answer.trim())
        .slice(-3)
        .flatMap((t) => [
          { role: "user", content: t.question },
          { role: "assistant", content: t.answer },
        ]);
      const key = uuid();
      const started = performance.now();
      setTurns((ts) => [
        ...ts,
        { key, question, askedAt: new Date().toISOString(), answer: "", blocks: [], files: fileIds, phase: "search", startedAt: started, model },
      ]);
      setActive(prior.length);
      setScrollTarget(key);
      setSheet("closed");
      setBusy(true);
      const first = prior[0]?.question ?? question;
      setLive(chatId, { question: first, answering: true });

      const ctrl = new AbortController();
      abort.current = ctrl;
      const patch = (fn: (t: Turn) => Turn) => setTurns((ts) => ts.map((t) => (t.key === key ? fn(t) : t)));

      // answer text arrives in many small deltas; render it at most once a frame
      let pending = "";
      let frame = 0;
      const flush = () => {
        frame = 0;
        if (!pending) return;
        const chunk = pending;
        pending = "";
        patch((t) => ({ ...t, answer: t.answer + chunk, phase: t.phase === "error" ? t.phase : "write" }));
      };

      try {
        await ask(
          {
            id: chatId,
            query: question,
            ...(model ? { model, models: { answer: model } } : {}),
            message_history: history,
            files: fileIds,
          },
          (e) => {
            if (e.type === "a") {
              pending += e.c;
              if (!frame) frame = requestAnimationFrame(flush);
            } else if (e.type === "b") {
              patch((t) => ({
                ...t,
                blocks: [...t.blocks, e.block],
                phase: e.block.type === "references" && t.phase === "search" ? "read" : t.phase,
              }));
            } else if (e.type === "c") {
              patch((t) => ({ ...t, error: e.message, phase: "error" }));
            } else if (e.type === "id") {
              reload();
            }
          },
          ctrl.signal,
        );
      } catch (err: any) {
        if (err?.name !== "AbortError") patch((t) => ({ ...t, error: String(err?.message || err), phase: "error" }));
      } finally {
        cancelAnimationFrame(frame);
        flush();
        const stopped = ctrl.signal.aborted;
        patch((t) => ({
          ...t,
          phase: t.phase === "error" ? "error" : stopped ? "stopped" : "done",
          elapsedMs: performance.now() - started,
        }));
        abort.current = null;
        setBusy(false);
        setLive(chatId, { question: first, answering: false });
        settle();
      }
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
        setTurns(fromMessages(r.messages));
        setStatus("ready");
      })
      .catch((e) => {
        if (cancelled) return;
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

  useEffect(() => () => abort.current?.abort(), []);

  // A question carried in the URL (from home or discover) is asked once the thread is ready.
  // Deferred a tick so a StrictMode remount cancels the timer, not the stream.
  useEffect(() => {
    if (status !== "ready" || !seed || seeded.current) return;
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
  const stop = useCallback(() => abort.current?.abort(), []);
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
            <button className="btn btn--quiet" onClick={() => last && send(last.question, last.files)} disabled={busy || !last} title="Ask the last question again">
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
