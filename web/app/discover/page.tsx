"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import Topbar from "@/components/Topbar";
import { useThreads } from "@/components/threads";
import { api } from "@/lib/api";
import { cleanTitle } from "@/lib/format";

export default function Discover() {
  const { chats, status } = useThreads();
  const [chatId, setChatId] = useState("");
  const [questions, setQuestions] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    document.title = "Discover — Libra";
  }, []);

  useEffect(() => {
    if (!chatId && chats[0]) setChatId(chats[0].id);
  }, [chats, chatId]);

  useEffect(() => {
    if (!chatId) return;
    let cancelled = false;
    setBusy(true);
    setError("");
    setQuestions([]);
    api<{ questions: string[] }>(`/api/discover?chat_id=${chatId}`)
      .then((r) => !cancelled && setQuestions(r.questions ?? []))
      .catch((e) => !cancelled && setError(e.message))
      .finally(() => !cancelled && setBusy(false));
    return () => {
      cancelled = true;
    };
  }, [chatId]);

  return (
    <>
      <Topbar title="Discover" />
      <div className="page">
        <div className="page__inner">
          <header className="page__head">
            <p className="kicker">From your threads</p>
            <h2>Discover</h2>
            <p>Questions worth asking next, drawn from a thread’s history. Pick one to continue that thread with it.</p>
          </header>

          <div className="discover-pick">
            <select value={chatId} onChange={(e) => setChatId(e.target.value)} aria-label="Thread" disabled={!chats.length}>
              {!chats.length && <option value="">No threads yet</option>}
              {chats.map((c) => (
                <option key={c.id} value={c.id}>
                  {cleanTitle(c.title) || "Untitled thread"}
                </option>
              ))}
            </select>
            {busy && <span className="meta">Thinking…</span>}
          </div>

          {error && (
            <p className="callout callout--error" style={{ marginTop: 20 }} role="alert">
              <b>Couldn’t suggest questions.</b> {error}
            </p>
          )}
          {status === "ready" && !chats.length && (
            <p className="callout" style={{ marginTop: 20 }}>
              Ask something first — suggestions are drawn from a thread’s history.
            </p>
          )}
          {!busy && !error && chatId && !questions.length && (
            <p className="callout" style={{ marginTop: 20 }}>
              Nothing to suggest for this thread yet.
            </p>
          )}

          {questions.length > 0 && (
            <nav className="followups" aria-label="Suggested questions">
              <p className="kicker">Keep pulling the thread</p>
              <ul>
                {questions.map((q) => (
                  <li key={q}>
                    <Link href={`/?chat=${chatId}&q=${encodeURIComponent(q)}`}>{q}</Link>
                  </li>
                ))}
              </ul>
            </nav>
          )}
        </div>
      </div>
    </>
  );
}
