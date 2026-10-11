import type { Block, ChatMessage, SearchResult } from "./api";

/** search → read → write while streaming; the rest are terminal. */
export type Phase = "search" | "read" | "write" | "done" | "stopped" | "error";

/** One question and its answer. A thread is a list of these. */
export type Turn = {
  key: string;
  question: string;
  askedAt: string | null;
  answer: string;
  blocks: Block[];
  /** uploaded file ids the question was scoped to (document mode) */
  files: string[];
  phase: Phase;
  error?: string;
  startedAt?: number;
  elapsedMs?: number;
  model?: string;
};

export const isStreaming = (p: Phase) => p === "search" || p === "read" || p === "write";

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
    const answer = m.content || (m.blocks ?? []).find((b) => b.type === "message")?.message || "";
    if (last && !last.answer && !last.blocks.length) {
      last.answer = answer;
      last.blocks = m.blocks ?? [];
    } else {
      turns.push({ key: m.id, question: "", askedAt: m.created_at, answer, blocks: m.blocks ?? [], files: [], phase: "done" });
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
