import type { Block, ChatMessage, SearchResult } from "./api";

/** search → read → write while streaming; the rest are terminal. */
export type Phase = "search" | "read" | "write" | "done" | "stopped" | "error";

/** One question and its answer. A thread is a list of these. */
export type Turn = {
  key: string;
  question: string;
  askedAt: string | null;
  answer: string;
  /** reasoning trace streamed ahead of the answer */
  thought?: string;
  blocks: Block[];
  /** uploaded file ids the question was scoped to (document mode) */
  files: string[];
  phase: Phase;
  /** The browser is resolving the user's place for this question. */
  locating?: boolean;
  error?: string;
  startedAt?: number;
  elapsedMs?: number;
  model?: string;
};

export const isStreaming = (p: Phase) => p === "search" || p === "read" || p === "write";

export function phaseOf(status?: string | null): Phase {
  switch (status) {
    case "search":
    case "read":
    case "write":
    case "stopped":
    case "error":
      return status;
    default:
      return "done";
  }
}

function fill(turn: Turn, m: ChatMessage) {
  turn.answer = m.content || (m.blocks ?? []).find((b) => b.type === "message")?.message || "";
  turn.blocks = m.blocks ?? [];
  turn.thought = (m.blocks ?? []).find((b) => b.type === "thought")?.message || "";
  turn.phase = phaseOf(m.status);
  if (m.error) turn.error = m.error;
  if (m.model) turn.model = m.model;
  if (m.elapsed_ms != null) turn.elapsedMs = m.elapsed_ms;
}

const WIDGETS = new Set(["widget", "weather", "stocks", "calculation", "time"]);

export function fromMessages(messages: ChatMessage[]): Turn[] {
  const turns: Turn[] = [];
  for (const m of messages) {
    if (m.role === "user") {
      turns.push({
        key: m.id,
        question: m.content,
        askedAt: m.created_at,
        answer: "",
        blocks: [],
        files: m.files ?? [],
        phase: "done",
      });
      continue;
    }
    const last = turns[turns.length - 1];
    if (last && !last.answer && !last.blocks.length) {
      fill(last, m);
    } else {
      const turn: Turn = { key: m.id, question: "", askedAt: m.created_at, answer: "", blocks: [], files: [], phase: "done" };
      fill(turn, m);
      turns.push(turn);
    }
  }
  return turns;
}

export function sourcesOf(blocks: Block[]): SearchResult[] {
  const refs = blocks.find((b) => b.type === "references");
  return Array.isArray(refs?.data?.web) ? refs!.data.web : [];
}

export const hasReferences = (blocks: Block[]) => blocks.some((b) => b.type === "references");

export function relatedOf(blocks: Block[]): string[] {
  const rel = blocks.find((b) => b.type === "related");
  return Array.isArray(rel?.data?.questions) ? rel!.data.questions : [];
}

export function mediaOf(blocks: Block[], type: "images" | "videos"): SearchResult[] {
  const b = blocks.find((x) => x.type === type);
  return Array.isArray(b?.data?.results) ? b!.data.results : [];
}

export const widgetsOf = (blocks: Block[]) => blocks.filter((b) => WIDGETS.has(b.type));

export type Research = {
  depth: string;
  round: number;
  rounds: number;
  queries: string[];
  /** "search" while queries run, then "filter", then "rerank" */
  step: string;
};

/** Queries the engine decided to run, in the order the rounds produced them. */
export function researchOf(blocks: Block[]): Research {
  const queries: string[] = [];
  let depth = "";
  let round = 0;
  let rounds = 0;
  let step = "";
  for (const b of blocks) {
    if (b.type !== "research") continue;
    if (typeof b.data?.depth === "string") depth = b.data.depth;
    if (typeof b.data?.step === "string") step = b.data.step;
    round = Number(b.data?.round) || round;
    rounds = Number(b.data?.rounds) || rounds;
    const list = Array.isArray(b.data?.queries) ? b.data.queries : [];
    for (const q of list) {
      if (typeof q === "string" && q && !queries.includes(q)) queries.push(q);
    }
  }
  return { depth, round, rounds, queries, step };
}

export type Stats = {
  elapsedMs?: number;
  model?: string;
  searchQuery?: string;
};

/** Timing and model come from the closing `answered` block, which is only stored, not streamed. */
export function statsOf(turn: Turn): Stats {
  const closing = [...turn.blocks].reverse().find((b) => b.type === "answered" && b.data?.elapsed_ms != null);
  return {
    elapsedMs: closing?.data?.elapsed_ms ?? turn.elapsedMs,
    model: closing?.data?.model ?? turn.model,
    searchQuery: closing?.data?.search_query,
  };
}
