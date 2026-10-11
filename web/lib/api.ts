export type SearchResult = {
  title: string;
  url: string;
  snippet?: string | null;
  content?: string | null;
  image?: string | null;
  thumbnail?: string | null;
  source: string;
  published_at?: string | null;
  author?: string | null;
  score?: number | null;
};

export type Block = {
  type: string;
  message?: string;
  data?: any;
  extra?: any;
};

export type FileRef = { id: string; name: string };

export type Chat = {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  sources: any[];
  files: FileRef[];
  /** Phase of the latest answer: search, read, write, done, stopped, or error. */
  status?: string;
};

export type ChatMessage = {
  id: string;
  role: string;
  content: string;
  blocks: Block[];
  /** ids of the uploaded files the question was asked against */
  files: string[];
  created_at: string;
  /** search | read | write while answering, then done, stopped, or error */
  status?: string;
  error?: string;
  model?: string;
  elapsed_ms?: number | null;
};

export type ModelOptions = { chat: string[]; embedding: string[] };

export type Health = {
  status: string;
  redis: boolean;
  searchProviders: string[];
  models: ModelOptions;
};

export type Provider = {
  id: string;
  name: string;
  type: string;
  base_url: string;
  api_key: string;
  models: string[];
  embedding_models: string[];
};

/** GET /api/settings — the mutable part of the server config. */
export type Settings = {
  providers: Provider[];
  defaultModel: string;
  defaultLightModel: string;
  defaultRerankModel: string;
  defaultEmbeddingModel: string;
  defaultTemperature: number;
  searchOrder: Record<"web" | "news" | "images" | "videos", string[]>;
  enabledProviders: string[];
  disabledWidgets: string[];
  [key: string]: any;
};

export type AskEvent =
  | { type: "id"; id: string }
  | { type: "a"; c: string }
  | { type: "t"; c: string }
  | { type: "b"; block: Block }
  | { type: "e" }
  | { type: "c"; message: string }
  | { type: "loc"; precise?: boolean };

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

export async function api<T = any>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, init);
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    let message = body;
    try {
      message = (JSON.parse(body)?.error?.message ?? body).replace(/^(bad request|not found): /, "");
    } catch {
      /* not JSON */
    }
    throw new ApiError(message || `${res.status} ${res.statusText}`, res.status);
  }
  return res.status === 204 ? (undefined as T) : res.json();
}

export const putJson = <T = any>(path: string, body: unknown) =>
  api<T>(path, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });

/** Idempotent: makes sure the chat row exists so uploads are listed on it. */
export const ensureChat = (id: string) => api<Chat>(`/api/chats?chat_id=${encodeURIComponent(id)}`, { method: "POST" });

export const deleteChat = (id: string) => api<void>(`/api/chats/${id}`, { method: "DELETE" });

/** Stop the answer in progress on this thread. The partial answer stays stored. */
export const stopChat = (id: string) => api<void>(`/api/chats/${id}/stop`, { method: "POST" });

/** Browser fix for a location request on the ask stream. */
export const sendLocation = (
  id: string,
  body: { latitude?: number; longitude?: number; timezone?: string; denied?: boolean },
) =>
  api<void>(`/api/chats/${encodeURIComponent(id)}/location`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });

export async function uploadFiles(chatId: string, files: FileList | File[]) {
  const form = new FormData();
  form.append("chatId", chatId);
  for (const f of Array.from(files)) form.append("files", f);
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 90_000);
  try {
    return await api<{ files: { id: string; name: string; chunks: number }[] }>("/api/upload", {
      method: "POST",
      body: form,
      signal: ctrl.signal,
    });
  } finally {
    clearTimeout(timer);
  }
}

/** POST /api/ask and hand back each SSE event as it arrives. */
export async function ask(body: unknown, onEvent: (e: AskEvent) => void, signal?: AbortSignal) {
  const res = await fetch("/api/ask", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal,
  });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  if (!res.body) throw new Error("no response body");
  const reader = res.body.getReader();
  const stopReading = () => {
    reader.cancel().catch(() => {});
  };
  signal?.addEventListener("abort", stopReading);
  if (signal?.aborted) stopReading();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done || signal?.aborted) break;
      buffer += decoder.decode(value, { stream: true });
      let split;
      while ((split = buffer.indexOf("\n\n")) >= 0) {
        const frame = buffer.slice(0, split);
        buffer = buffer.slice(split + 2);
        const line = frame.split("\n").find((l) => l.startsWith("data:"));
        if (!line) continue;
        try {
          onEvent(JSON.parse(line.slice(5)) as AskEvent);
        } catch {
          /* partial frame */
        }
      }
    }
  } finally {
    signal?.removeEventListener("abort", stopReading);
  }
}

/** crypto.randomUUID only exists in secure contexts; a LAN http:// install is not one. */
export function uuid(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
