"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import Topbar from "@/components/Topbar";
import { api, uuid } from "@/lib/api";
import { host } from "@/lib/format";

type Blog = {
  title: string;
  content: string;
  url: string;
  thumbnail: string;
  chatId: string;
};

const TOPICS = [
  { key: "tech", label: "Tech & Science" },
  { key: "finance", label: "Finance" },
  { key: "art", label: "Art & Culture" },
  { key: "sports", label: "Sports" },
  { key: "entertainment", label: "Entertainment" },
];

export default function Discover() {
  const [topic, setTopic] = useState(TOPICS[0].key);
  const [blogs, setBlogs] = useState<Blog[]>([]);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    document.title = "Discover — Libra";
  }, []);

  useEffect(() => {
    let cancelled = false;
    setBusy(true);
    setError("");
    api<{ blogs: Omit<Blog, "chatId">[] }>(`/api/discover?topic=${topic}`)
      .then((r) => {
        if (cancelled) return;
        setBlogs((r.blogs ?? []).filter((b) => b.url).map((b) => ({ ...b, chatId: uuid() })));
      })
      .catch((e) => {
        if (cancelled) return;
        setBlogs([]);
        setError(e.message);
      })
      .finally(() => !cancelled && setBusy(false));
    return () => {
      cancelled = true;
    };
  }, [topic]);

  const [lead, ...rest] = blogs;

  return (
    <>
      <Topbar title="Discover" />
      <div className="page">
        <div className="page__inner page__inner--wide">
          <header className="page__head">
            <p className="kicker">From the news</p>
            <h2>Discover</h2>
            <p>Recent stories on a topic. Open one and Libra summarizes it in a new thread.</p>
            <div className="topics" role="tablist" aria-label="Topics">
              {TOPICS.map((t) => (
                <button
                  key={t.key}
                  type="button"
                  role="tab"
                  className="chip"
                  aria-selected={topic === t.key}
                  aria-pressed={topic === t.key}
                  onClick={() => setTopic(t.key)}
                >
                  {t.label}
                </button>
              ))}
            </div>
          </header>

          {error && (
            <p className="callout callout--error" role="alert">
              <b>Couldn’t load stories.</b> {error}
            </p>
          )}
          {busy && <p className="meta">Looking up the news…</p>}
          {!busy && !error && !blogs.length && (
            <p className="callout">Nothing in this topic right now. Try another, or check that a news search provider is configured.</p>
          )}

          {!busy && lead && (
            <div className="news">
              <Story item={lead} lead />
              {rest.length > 0 && (
                <div className="news__grid">
                  {rest.map((item) => (
                    <Story key={item.url} item={item} />
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </>
  );
}

function Story({ item, lead }: { item: Blog; lead?: boolean }) {
  const href = `/?chat=${item.chatId}&q=${encodeURIComponent(`Summary: ${item.url}`)}`;
  return (
    <Link className={lead ? "news__lead" : "news__card"} href={href}>
      <Shot src={item.thumbnail} alt="" />
      <span className="news__copy">
        <span className="news__domain">{host(item.url)}</span>
        <span className="news__title">{item.title}</span>
        {item.content && <span className="news__excerpt">{item.content}</span>}
      </span>
    </Link>
  );
}

function Shot({ src, alt }: { src: string; alt: string }) {
  const [ok, setOk] = useState(Boolean(src));
  return (
    <span className="news__figure">
      {ok && src && (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={src} alt={alt} referrerPolicy="no-referrer" onError={() => setOk(false)} />
      )}
    </span>
  );
}
