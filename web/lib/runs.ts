// An ask keeps running after its thread unmounts. The thread reads this
// store when it mounts again, so leaving and coming back still shows the
// question and whatever has streamed so far.

import { ask, uuid } from "@/lib/api";
import type { Turn } from "@/lib/turn";

export type RunSnapshot = {
  turns: Turn[];
  busy: boolean;
};

type Run = RunSnapshot & { abort: AbortController };

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

export function stopRun(chatId: string) {
  runs.get(chatId)?.abort.abort();
}

/** History can arrive after a follow-up has already started. Keep the live turn. */
export function adoptHistory(chatId: string, serverTurns: Turn[]) {
  const run = runs.get(chatId);
  if (!run?.busy) return;
  if (run.turns.length - 1 >= serverTurns.length) return;
  const live = run.turns[run.turns.length - 1];
  run.turns = [...serverTurns, live];
  emit(chatId);
}

export function startRun(opts: {
  chatId: string;
  question: string;
  fileIds: string[];
  model: string;
  depth: string;
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
  runs.set(opts.chatId, { turns: [...opts.prior, turn], busy: true, abort: ctrl });
  emit(opts.chatId);
  opts.setLive(opts.chatId, { question: first, answering: true });

  const patch = (fn: (t: Turn) => Turn) => {
    const run = runs.get(opts.chatId);
    if (!run || run.abort !== ctrl) return;
    run.turns = run.turns.map((t) => (t.key === key ? fn(t) : t));
    emit(opts.chatId);
  };

  let pending = "";
  let frame = 0;
  const flush = () => {
    frame = 0;
    if (!pending) return;
    const chunk = pending;
    pending = "";
    patch((t) => ({ ...t, answer: t.answer + chunk, phase: t.phase === "error" ? t.phase : "write" }));
  };

  void (async () => {
    try {
      await ask(
        {
          id: opts.chatId,
          query: question,
          ...(opts.model ? { model: opts.model, models: { answer: opts.model } } : {}),
          depth: opts.depth || "balance",
          message_history: history,
          files: opts.fileIds,
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
            opts.reload();
          }
        },
        ctrl.signal,
      );
    } catch (err: any) {
      if (err?.name !== "AbortError") {
        patch((t) => ({ ...t, error: String(err?.message || err), phase: "error" }));
      }
    } finally {
      cancelAnimationFrame(frame);
      flush();
      const run = runs.get(opts.chatId);
      if (run?.abort === ctrl) {
        const stopped = ctrl.signal.aborted;
        patch((t) => ({
          ...t,
          phase: t.phase === "error" ? "error" : stopped ? "stopped" : "done",
          elapsedMs: performance.now() - started,
        }));
        run.busy = false;
        emit(opts.chatId);
        opts.setLive(opts.chatId, { question: first, answering: false });
        opts.settle();
      }
    }
  })();

  return key;
}
