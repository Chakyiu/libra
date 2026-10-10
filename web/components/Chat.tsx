"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { ChatMessage, api, ask, postJson, uploadFiles } from "@/lib/api";
import BlockView from "@/components/Blocks";
import Answer from "@/components/Answer";

type View = { id: string; role: string; content: string; blocks: any[]; streaming?: boolean };

const toView = (m: ChatMessage): View => ({ id: m.id, role: m.role, content: m.content, blocks: m.blocks ?? [] });

export default function Chat() {
  const params = useSearchParams();
  const router = useRouter();
  const [chatId, setChatId] = useState(() => params.get("chat") || crypto.randomUUID());
  const [views, setViews] = useState<View[]>([]);
  const [input, setInput] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [model, setModel] = useState("");
  const [files, setFiles] = useState<{ id: string; name: string }[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState("");
  const activeId = useRef<string>("");
  const abort = useRef<AbortController | null>(null);
  const bottom = useRef<HTMLDivElement | null>(null);
  const fileInput = useRef<HTMLInputElement | null>(null);

  const urlChat = params.get("chat");
  useEffect(() => {
    if (urlChat && urlChat !== chatId) {
      setChatId(urlChat);
      setFiles([]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [urlChat]);

  // /discover links arrive as /?q=… and are asked once history is in.
  const seed = params.get("q");
  const seeded = useRef("");
  useEffect(() => {
    if (seed && loaded === chatId && seeded.current !== seed) {
      seeded.current = seed;
      router.replace(`/?chat=${chatId}`);
      send(seed);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seed, loaded, chatId]);

  useEffect(() => {
    api<{ chat: string[] }>("/api/models").then((m) => {
      setModels(m.chat);
      api<any>("/api/settings").then((s) => setModel(s.defaultModel)).catch(() => {});
    });
    api<{ messages: ChatMessage[] }>(`/api/chats/${chatId}`).then((r) => r.messages)
      .then((m) => setViews(m.map(toView)))
      .catch(() => setViews([]))
      .finally(() => setLoaded(chatId));
  }, [chatId]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [views]);

  const patchActive = useCallback((fn: (v: View) => View) => {
    setViews((all) => all.map((v) => (v.id === activeId.current ? fn(v) : v)));
  }, []);

  async function send(query: string) {
    const q = query.trim();
    if (!q || busy) return;
    setError("");
    setInput("");
    setBusy(true);

    const history = views
      .filter((v) => v.content.trim())
      .slice(-6)
      .map((v) => ({ role: v.role, content: v.content }));

    const assistantId = crypto.randomUUID();
    activeId.current = assistantId;
    setViews((all) => [
      ...all,
      { id: crypto.randomUUID(), role: "user", content: q, blocks: [] },
      { id: assistantId, role: "assistant", content: "", blocks: [], streaming: true },
    ]);

    abort.current = new AbortController();
    try {
      await ask(
        { id: chatId, query: q, model, models: { answer: model }, message_history: history, files: files.map((f) => f.id) },
        (e) => {
          if (e.type === "id") {
            setChatId(e.id);
            window.dispatchEvent(new Event("vane:chats"));
          } else if (e.type === "a") {
            patchActive((v) => ({ ...v, content: v.content + e.c }));
          } else if (e.type === "b") {
            patchActive((v) => ({ ...v, blocks: [...v.blocks, e.block] }));
          } else if (e.type === "c") {
            setError(e.message);
          }
        },
        abort.current.signal,
      );
    } catch (err: any) {
      if (err?.name !== "AbortError") setError(String(err?.message || err));
    } finally {
      patchActive((v) => ({ ...v, streaming: false }));
      setBusy(false);
      abort.current = null;
      window.dispatchEvent(new Event("vane:chats"));
    }
  }

  async function onFiles(list: FileList | null) {
    if (!list?.length) return;
    setError("");
    try {
      const res = await uploadFiles(chatId, list);
      setFiles((f) => [...f, ...res.files.map((x) => ({ id: x.id, name: x.name }))]);
      window.dispatchEvent(new Event("vane:chats"));
    } catch (e: any) {
      setError(`upload failed: ${e.message}`);
    } finally {
      if (fileInput.current) fileInput.current.value = "";
    }
  }

  return (
    <div className="flex flex-col h-screen">
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-3xl mx-auto px-6 py-8 space-y-6">
          {!views.length && (
            <div className="pt-24 text-center">
              <h1 className="text-3xl font-semibold tracking-tight">Ask anything</h1>
              <p className="text-mute mt-2 text-sm">
                Self-hosted answer engine · search, widgets and document RAG in one stream
              </p>
            </div>
          )}

          {views.map((v) =>
            v.role === "user" ? (
              <div key={v.id} className="text-right">
                <span className="inline-block bg-panel border border-edge rounded-lg px-4 py-2 text-sm max-w-[80%]">
                  {v.content}
                </span>
              </div>
            ) : (
              <div key={v.id} className="space-y-1">
                {v.blocks
                  .filter((b) => b.type !== "message" && b.type !== "references")
                  .map((b, i) => (
                    <BlockView key={i} block={b} onFollowUp={send} />
                  ))}
                {v.content ? (
                  <Answer text={v.content} />
                ) : (
                  v.blocks
                    .filter((b) => b.type === "message")
                    .map((b, i) => <BlockView key={`m${i}`} block={b} onFollowUp={send} />)
                )}
                {v.blocks
                  .filter((b) => b.type === "references")
                  .map((b, i) => (
                    <BlockView key={`r${i}`} block={b} onFollowUp={send} />
                  ))}
                {v.streaming && !v.content && (
                  <p className="text-sm text-mute animate-pulse">researching…</p>
                )}
              </div>
            ),
          )}

          {error && (
            <div className="border border-red-900 bg-red-950/40 text-red-300 rounded p-3 text-sm">
              {error}
            </div>
          )}
          <div ref={bottom} />
        </div>
      </div>

      <div className="border-t border-edge bg-panel/40">
        <div className="max-w-3xl mx-auto px-6 py-3">
          {files.length > 0 && (
            <div className="flex flex-wrap gap-2 mb-2">
              {files.map((f) => (
                <span key={f.id} className="text-xs border border-edge rounded-full px-2 py-0.5 text-mute">
                  📎 {f.name}
                </span>
              ))}
            </div>
          )}
          <form
            onSubmit={(e) => {
              e.preventDefault();
              send(input);
            }}
            className="flex items-end gap-2"
          >
            <input
              ref={fileInput}
              type="file"
              multiple
              hidden
              onChange={(e) => onFiles(e.target.files)}
            />
            <button
              type="button"
              title="Attach documents"
              onClick={() => fileInput.current?.click()}
              className="px-2 py-2 text-mute hover:text-ink"
            >
              📎
            </button>
            <textarea
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send(input);
                }
              }}
              rows={1}
              placeholder="Ask a question…"
              className="flex-1 resize-none max-h-40"
            />
            <select
              value={model}
              onChange={(e) => setModel(e.target.value)}
              className="max-w-[14rem] text-xs"
              title="Answer model"
            >
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
            {busy ? (
              <button
                type="button"
                onClick={() => abort.current?.abort()}
                className="px-4 py-2 rounded bg-edge text-sm"
              >
                Stop
              </button>
            ) : (
              <button type="submit" className="px-4 py-2 rounded bg-accent text-bg text-sm font-medium">
                Ask
              </button>
            )}
          </form>
        </div>
      </div>
    </div>
  );
}
