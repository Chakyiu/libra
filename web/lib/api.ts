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

export type Chat = {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  sources: any[];
  files: { id: string; name: string }[];
};

export type ChatMessage = {
  id: string;
  role: string;
  content: string;
  blocks: Block[];
  files: { id: string; name: string }[];
  created_at: string;
};

export type AskEvent =
  | { type: "id"; id: string }
  | { type: "a"; c: string }
  | { type: "b"; block: Block }
  | { type: "e" }
  | { type: "c"; message: string };

export async function api<T = any>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, init);
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(body || `${res.status} ${res.statusText}`);
  }
  return res.status === 204 ? (undefined as T) : res.json();
}

export const postJson = (path: string, body: unknown) =>
  api(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });

export async function uploadFiles(chatId: string, files: FileList | File[]) {
  const form = new FormData();
  form.append("chatId", chatId);
  for (const f of Array.from(files)) form.append("files", f);
  return api<{ files: { id: string; name: string; chunks: number }[] }>("/api/upload", {
    method: "POST",
    body: form,
  });
}

/** POST /api/ask and hand back each SSE event as it arrives. */
export async function ask(body: unknown, onEvent: (e: AskEvent) => void, signal?: AbortSignal) {
  const res = await fetch("/api/ask", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal,
  });
  if (!res.body) throw new Error("no response body");
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
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
}

export const host = (url: string) => {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
};

export const favicon = (url: string) => `https://www.google.com/s2/favicons?domain=${host(url)}&sz=32`;
