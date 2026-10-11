"use client";

import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import Scope, { useAttachments, useFileDrop } from "@/components/Scope";
import Topbar, { VERSION, useHost } from "@/components/Topbar";
import { ArrowIcon, Mark } from "@/components/icons";
import { useInstance } from "@/components/instance";
import { useThreads } from "@/components/threads";
import { uuid } from "@/lib/api";
import { cleanTitle, groupOf, modelLabel, threadTime, toDate } from "@/lib/format";
import { handOff } from "@/lib/handoff";

const EXAMPLES = [
  "What changed in Firecracker microVM cold-start performance this year?",
  "How does SQLite's WAL mode let readers and a writer work at the same time?",
  "When is Redis the wrong choice for a cache?",
];

function StatusStrip() {
  const { online, health, settings, models, needsProvider } = useInstance();
  const enabled = settings?.enabledProviders ?? [];
  const order = (settings?.searchOrder?.web ?? []).filter((p) => !enabled.length || enabled.includes(p));
  const model = settings?.defaultModel || models.chat[0];

  return (
    <div className="status">
      <span className="status__item meta">
        <span className="dot" data-state={online === null ? "pending" : online ? "ok" : "down"} />
        {online === null ? "Connecting…" : online ? "Engine online" : "Engine unreachable"}
      </span>
      {order.length > 0 && <span className="status__item meta">Search: {order.join(" → ")}</span>}
      {health && <span className="status__item meta">{health.redis ? "Cache: Redis" : "Cache off"}</span>}
      {model ? (
        <span className="status__item meta">Model: {modelLabel(model)}</span>
      ) : (
        needsProvider && <span className="status__item meta">Model: none yet</span>
      )}
      <span className="topbar__spacer" />
      <span className="status__item meta">{VERSION ? `v${VERSION} · ` : ""}self-hosted</span>
    </div>
  );
}

export default function Home() {
  const router = useRouter();
  const host = useHost();
  const { chats } = useThreads();
  const { needsProvider } = useInstance();
  const [draft] = useState(uuid);
  const [text, setText] = useState("");
  const att = useAttachments(draft);
  const drop = useFileDrop(att.attach, att.uploading);
  const box = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    document.title = "Libra";
    if (matchMedia("(pointer: fine)").matches) box.current?.focus();
  }, []);

  function ask(q: string) {
    const question = q.trim();
    if (!question || att.uploading || needsProvider) return;
    handOff(draft, { files: att.files, selected: att.selected });
    router.push(`/?chat=${draft}&q=${encodeURIComponent(question)}`);
  }

  const recent = chats.filter((c) => cleanTitle(c.title)).slice(0, 3);

  return (
    <>
      <Topbar meta={host} />
      <div className="home">
        <div className="home__center">
          <div className="hero">
            <span className="hero__mark">
              <Mark size={38} stroke={1.5} dot />
            </span>
            <h1>Libra</h1>
            <p className="hero__sub">Every sentence carries its source. Ask the open web or your own files — on your own hardware.</p>

            <form
              className={`askbox${drop.over ? " is-drop" : ""}`}
              onSubmit={(e) => {
                e.preventDefault();
                ask(text);
              }}
              onDragEnter={drop.onDragEnter}
              onDragOver={drop.onDragOver}
              onDragLeave={drop.onDragLeave}
              onDrop={drop.onDrop}
            >
              {drop.over && <p className="drop-hint">Drop to attach</p>}
              <div className="askbox__field">
                <textarea
                  ref={box}
                  data-ask-input
                  rows={2}
                  value={text}
                  onChange={(e) => setText(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                      e.preventDefault();
                      ask(text);
                    }
                  }}
                  placeholder="Ask anything the web or your files can answer…"
                  aria-label="Your question"
                />
                <button className="btn btn--primary" type="submit" disabled={att.uploading || needsProvider}>
                  Ask
                  <ArrowIcon />
                </button>
              </div>

              <div className="askbox__bar">
                <Scope
                  files={att.files}
                  selected={att.selected}
                  onSelect={att.setSelected}
                  onAttach={att.attach}
                  uploading={att.uploading}
                />
                <span className="askbox__hint meta">
                  <span className="kbd">↵</span> to ask · <span className="kbd">⇧↵</span> newline
                </span>
              </div>
              {att.error && <p className="askbox__error">{att.error}</p>}
            </form>

            {needsProvider && (
              <div className="callout home__setup">
                <b>No model provider yet.</b> Libra needs one to write answers — a local server such as Ollama or LM Studio, or an
                API such as OpenAI or OpenRouter.
                <div>
                  <Link className="btn btn--primary" href="/settings/?tab=providers">
                    Add a model provider
                    <ArrowIcon />
                  </Link>
                </div>
              </div>
            )}

            <section className="starters">
              {recent.length > 0 ? (
                <>
                  <p className="kicker">Pick up where you left off</p>
                  <ul>
                    {recent.map((c) => {
                      const d = toDate(c.updated_at);
                      const group = groupOf(d);
                      return (
                        <li key={c.id}>
                          <Link href={`/?chat=${c.id}`}>
                            <span>{cleanTitle(c.title)}</span>
                            <em className="meta">{group === "Yesterday" ? "yesterday" : threadTime(d, group)}</em>
                          </Link>
                        </li>
                      );
                    })}
                  </ul>
                </>
              ) : (
                !needsProvider && (
                <>
                  <p className="kicker">Try asking</p>
                  <ul>
                    {EXAMPLES.map((q) => (
                      <li key={q}>
                        <button type="button" onClick={() => ask(q)}>
                          <span>{q}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                </>
                )
              )}
            </section>
          </div>
        </div>

        <StatusStrip />
      </div>
    </>
  );
}
