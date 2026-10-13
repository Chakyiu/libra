// An ask keeps running after its thread unmounts. The thread reads this
// store when it mounts again, so leaving and coming back still shows the
// question and whatever has streamed so far.

import { ask, sendLocation, stopChat, uuid } from "@/lib/api";
import { sameQuestionKey } from "@/lib/format";
import { isStreaming, type Turn } from "@/lib/turn";

export type RunSnapshot = {
  turns: Turn[];
  busy: boolean;
};

type Run = RunSnapshot & { abort: AbortController; release: () => void };

type Live = { question: string; answering: boolean };

const runs = new Map<string, Run>();
const listeners = new Map<string, Set<(run: RunSnapshot) => void>>();

function emit(chatId: string) {
  const run = runs.get(chatId);
  if (!run) return;
  const snap = { turns: run.turns, busy: run.busy };
  listeners.get(chatId)?.forEach((fn) => fn(snap));
}

export function getRun(chatId: string): RunSnapshot | null {
  const run = runs.get(chatId);
  return run ? { turns: run.turns, busy: run.busy } : null;
}

export function subscribeRun(chatId: string, onChange: (run: RunSnapshot) => void) {
  let set = listeners.get(chatId);
  if (!set) {
    set = new Set();
    listeners.set(chatId, set);
  }
  set.add(onChange);
  const run = runs.get(chatId);
  if (run) onChange({ turns: run.turns, busy: run.busy });
  return () => {
    set!.delete(onChange);
  };
}

/** Stop the answer. Returns true when this tab still held the live stream.
 * A refresh has no stream, but Stop still tells the server to finish the turn. */
export function stopRun(chatId: string): boolean {
  const run = runs.get(chatId);
  const live = !!run?.busy;
  if (run?.busy) {
    // Abort first so a token already in flight cannot move the phase back to writing.
    run.abort.abort();
    run.busy = false;
    run.turns = run.turns.map((t) =>
      isStreaming(t.phase)
        ? {
            ...t,
            phase: "stopped",
            elapsedMs: t.startedAt != null ? performance.now() - t.startedAt : t.elapsedMs,
          }
        : t,
    );
    emit(chatId);
    run.release();
  }
  void stopChat(chatId).catch(() => {});
  return live;
}

/** The checkpoint of the ask in flight. The question is saved before the first search. */
function isLiveRow(server: Turn, live: Turn) {
  if (sameQuestionKey(server.question) !== sameQuestionKey(live.question)) return false;
  if (isStreaming(server.phase)) return true;
  return !server.answer && server.blocks.length === 0 && server.phase === "done";
}

/** History can arrive after a follow-up has already started. Keep the live turn. */
export function adoptHistory(chatId: string, serverTurns: Turn[]) {
  const run = runs.get(chatId);
  if (!run?.busy) return;
  const live = run.turns[run.turns.length - 1];
  if (!live) return;
  const last = serverTurns[serverTurns.length - 1];
  const base = last && isLiveRow(last, live) ? serverTurns.slice(0, -1) : serverTurns;
  const localBefore = run.turns.slice(0, -1);
  // A slow response can be older than the turns already on screen. Don't drop those.
  const prefix = base.length >= localBefore.length ? base : localBefore;
  const next = [...prefix, live];
  if (next.length === run.turns.length && next.every((t, i) => t === run.turns[i])) return;
  run.turns = next;
  emit(chatId);
}

export function startRun(opts: {
  chatId: string;
  question: string;
  fileIds: string[];
  model: string;
  depth: string;
  /** "answer" | "discussion" — a long-form report written section by section */
  mode: string;
  prior: Turn[];
  reload: () => void;
  settle: () => void;
  setLive: (id: string, live: Live | null) => void;
}): string | null {
  const question = opts.question.trim();
  const current = runs.get(opts.chatId);
  if (!question || current?.busy) return null;

  const history = opts.prior
    .filter((t) => t.answer.trim())
    .slice(-3)
    .flatMap((t) => [
      { role: "user", content: t.question },
      { role: "assistant", content: t.answer },
    ]);
  const key = uuid();
  const started = performance.now();
  const first = opts.prior[0]?.question || question;
  const ctrl = new AbortController();
  const release = () => opts.setLive(opts.chatId, { question: first, answering: false });
  const turn: Turn = {
    key,
    question,
    askedAt: new Date().toISOString(),
    answer: "",
    blocks: [],
    files: opts.fileIds,
    phase: "search",
    startedAt: started,
    model: opts.model,
  };
  runs.set(opts.chatId, { turns: [...opts.prior, turn], busy: true, abort: ctrl, release });
  emit(opts.chatId);
  opts.setLive(opts.chatId, { question: first, answering: true });

  const patch = (fn: (t: Turn) => Turn) => {
    const run = runs.get(opts.chatId);
    if (!run || run.abort !== ctrl || ctrl.signal.aborted) return;
    run.turns = run.turns.map((t) => (t.key === key ? fn(t) : t));
    emit(opts.chatId);
  };

  let pending = "";
  let pendingThought = "";
  let frame = 0;
  const flush = () => {
    frame = 0;
    if (ctrl.signal.aborted) {
      pending = "";
      pendingThought = "";
      return;
    }
    const chunk = pending;
    const thought = pendingThought;
    pending = "";
    pendingThought = "";
    if (!chunk && !thought) return;
    patch((t) => ({
      ...t,
      ...(chunk
        ? { answer: t.answer + chunk, phase: t.phase === "error" ? t.phase : ("write" as const) }
        : {}),
      ...(thought ? { thought: (t.thought || "") + thought } : {}),
    }));
  };

  void (async () => {
    try {
      await ask(
        {
          id: opts.chatId,
          query: question,
          ...(opts.model ? { model: opts.model, models: { answer: opts.model } } : {}),
          depth: opts.depth || "balance",
          mode: opts.mode || "answer",
          message_history: history,
          files: opts.fileIds,
        },
        (e) => {
          if (ctrl.signal.aborted) return;
          if (e.type === "a") {
            pending += e.c;
            if (!frame) frame = requestAnimationFrame(flush);
          } else if (e.type === "t") {
            pendingThought += e.c;
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
            opts.reload();
          } else if (e.type === "loc") {
            const precise = e.precise !== false;
            if (precise) patch((t) => ({ ...t, locating: true }));
            void shareLocation(opts.chatId, ctrl.signal, precise).finally(() => {
              if (precise) patch((t) => ({ ...t, locating: false }));
            });
          }
        },
        ctrl.signal,
      );
    } catch (err: any) {
      if (!ctrl.signal.aborted && err?.name !== "AbortError") {
        patch((t) => ({ ...t, error: String(err?.message || err), phase: "error" }));
      }
    } finally {
      cancelAnimationFrame(frame);
      if (ctrl.signal.aborted) {
        pending = "";
        pendingThought = "";
      } else flush();
      const run = runs.get(opts.chatId);
      // A follow-up may already have replaced this run.
      if (run?.abort !== ctrl) return;
      if (!ctrl.signal.aborted) {
        patch((t) => ({
          ...t,
          phase: t.phase === "error" ? "error" : "done",
          elapsedMs: performance.now() - started,
        }));
        run.busy = false;
        emit(opts.chatId);
      }
      release();
      opts.settle();
    }
  })();

  return key;
}

/** Coordinates when the question needs a place. A clock question only needs the timezone. */
async function shareLocation(chatId: string, signal: AbortSignal, precise: boolean) {
  const timezone = typeof Intl !== "undefined" ? Intl.DateTimeFormat().resolvedOptions().timeZone || "" : "";
  const coords = precise ? await readCoords(signal) : null;
  if (signal.aborted) return;
  try {
    await sendLocation(
      chatId,
      coords ? { latitude: coords.latitude, longitude: coords.longitude, timezone } : { denied: precise, timezone },
    );
  } catch {
    /* the answer continues without a place */
  }
}

function readCoords(signal: AbortSignal): Promise<{ latitude: number; longitude: number } | null> {
  if (signal.aborted || typeof navigator === "undefined" || !navigator.geolocation || !window.isSecureContext) {
    return Promise.resolve(null);
  }
  return new Promise((resolve) => {
    const stop = () => resolve(null);
    signal.addEventListener("abort", stop, { once: true });
    navigator.geolocation.getCurrentPosition(
      (pos) => {
        signal.removeEventListener("abort", stop);
        resolve(
          signal.aborted
            ? null
            : { latitude: pos.coords.latitude, longitude: pos.coords.longitude },
        );
      },
      () => {
        signal.removeEventListener("abort", stop);
        resolve(null);
      },
      { enableHighAccuracy: false, timeout: 20_000, maximumAge: 10 * 60 * 1000 },
    );
  });
}
