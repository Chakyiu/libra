"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Chat, api } from "@/lib/api";

export default function Discover() {
  const [chats, setChats] = useState<Chat[]>([]);
  const [chatId, setChatId] = useState("");
  const [questions, setQuestions] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api<Chat[]>("/api/chats")
      .then((c) => {
        setChats(c);
        if (c[0]) setChatId(c[0].id);
      })
      .catch(() => setError("cannot reach the backend"));
  }, []);

  useEffect(() => {
    if (!chatId) return;
    setBusy(true);
    setError("");
    api<{ questions: string[] }>(`/api/discover?chat_id=${chatId}`)
      .then((r) => setQuestions(r.questions))
      .catch((e) => setError(e.message))
      .finally(() => setBusy(false));
  }, [chatId]);

  return (
    <div className="max-w-3xl mx-auto px-6 py-8">
      <h1 className="text-2xl font-semibold">Discover</h1>
      <p className="text-sm text-mute mt-1">Follow-up questions suggested from a conversation.</p>

      <div className="flex items-center gap-3 mt-6">
        <select value={chatId} onChange={(e) => setChatId(e.target.value)} className="min-w-[16rem]">
          <option value="">Select a chat…</option>
          {chats.map((c) => (
            <option key={c.id} value={c.id}>
              {c.title || "Untitled"}
            </option>
          ))}
        </select>
        {busy && <span className="text-sm text-mute">thinking…</span>}
      </div>

      {error && <p className="text-sm text-red-400 mt-4">{error}</p>}
      {!busy && !questions.length && !error && (
        <p className="text-sm text-mute mt-6">
          Ask something in a chat first — suggestions need conversation history.
        </p>
      )}

      <div className="grid sm:grid-cols-2 gap-3 mt-6">
        {questions.map((q) => (
          <Link
            key={q}
            href={`/?chat=${chatId}&q=${encodeURIComponent(q)}`}
            className="border border-edge bg-panel rounded-lg p-4 hover:border-accent text-sm"
          >
            {q}
          </Link>
        ))}
      </div>
    </div>
  );
}
