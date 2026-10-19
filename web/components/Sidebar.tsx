"use client";

import { Fragment, useMemo } from "react";
import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { CloseIcon, CompassIcon, GearIcon, Mark, MenuIcon, PlusIcon, TimerIcon } from "@/components/icons";
import { useInstance } from "@/components/instance";
import { useShell } from "@/components/shell-context";
import { useThreads } from "@/components/threads";
import type { Chat } from "@/lib/api";
import { Group, cleanTitle, groupOf, plural, threadTime, toDate } from "@/lib/format";

const TOOLS = [
  { href: "/discover/", label: "Discover", Icon: CompassIcon },
  { href: "/automator/", label: "Automator", Icon: TimerIcon },
];

export default function Sidebar() {
  const pathname = usePathname();
  const params = useSearchParams();
  const router = useRouter();
  const { chats, status, live, remove } = useThreads();
  const { online, needsProvider } = useInstance();
  const { toggleSidebar, closeDrawer } = useShell();

  const activeChat = pathname === "/" ? params.get("chat") : null;

  const groups = useMemo(() => {
    const now = new Date();
    const out: { label: Group; items: { chat: Chat; time: string }[] }[] = [];
    for (const chat of chats) {
      const d = toDate(chat.updated_at) ?? toDate(chat.created_at);
      const label = groupOf(d, now);
      let group = out.find((g) => g.label === label);
      if (!group) out.push((group = { label, items: [] }));
      group.items.push({ chat, time: threadTime(d, label) });
    }
    return out;
  }, [chats]);

  async function onDelete(chat: Chat, title: string) {
    if (!window.confirm(`Delete “${title}”? Its answers and uploaded files go with it.`)) return;
    try {
      await remove(chat.id);
      if (activeChat === chat.id) router.push("/");
    } catch (e: any) {
      window.alert(`Couldn’t delete the thread: ${e.message}`);
    }
  }

  const health = online === null ? "pending" : online ? "ok" : "down";

  return (
    <aside
      className="sidebar"
      aria-label="Threads"
      onClick={(e) => {
        if ((e.target as HTMLElement).closest("a")) closeDrawer();
      }}
    >
      <div className="sidebar__head">
        <Link className="wordmark" href="/">
          <Mark />
          <b>Libra</b>
        </Link>
        <button className="btn btn--quiet btn--icon" onClick={toggleSidebar} aria-label="Hide threads" title="Hide threads  [">
          <MenuIcon />
        </button>
      </div>

      <Link className="btn sidebar__new" href="/">
        <PlusIcon />
        New thread
        <span className="kbd">N</span>
      </Link>

      <nav className="sidebar__tools" aria-label="Tools">
        {TOOLS.map(({ href, label, Icon }) => (
          <Link key={href} href={href} aria-current={pathname.startsWith(href.slice(0, -1)) ? "page" : undefined}>
            <Icon />
            {label}
          </Link>
        ))}
      </nav>

      <div className="sidebar__scroll">
        {groups.map((group) => (
          <Fragment key={group.label}>
            <p className="kicker sidebar__group">{group.label}</p>
            {group.items.map(({ chat, time }) => {
              const known = live[chat.id];
              const title = cleanTitle(chat.title) || known?.question || "Untitled thread";
              const files = chat.files?.length ? plural(chat.files.length, "file") : "";
              const answering = known
                ? known.answering
                : chat.status === "search" || chat.status === "read" || chat.status === "write";
              const meta = answering ? "answering…" : [files, time].filter(Boolean).join(" · ");
              return (
                <div className="thread-row" key={chat.id}>
                  <Link className="thread" href={`/?chat=${chat.id}`} aria-current={chat.id === activeChat ? "page" : undefined}>
                    <span>{title}</span>
                    <em>{meta}</em>
                  </Link>
                  <button
                    className="btn btn--quiet btn--icon thread__del"
                    onClick={() => onDelete(chat, title)}
                    aria-label={`Delete “${title}”`}
                    title="Delete thread"
                  >
                    <CloseIcon size={12} />
                  </button>
                </div>
              );
            })}
          </Fragment>
        ))}
        {status === "ready" && !chats.length && (
          <p className="sidebar__empty">No threads yet. Every question you ask is kept here with its sources.</p>
        )}
        {status === "error" && !chats.length && <p className="sidebar__empty">Threads are unavailable until the Libra server answers.</p>}
      </div>

      <div className="sidebar__foot">
        {needsProvider ? (
          <Link className="meta sidebar__setup" href="/settings/?tab=providers">
            <span className="dot" data-state="warn" /> Add a model provider
          </Link>
        ) : (
          <span className="meta">
            <span className="dot" data-state={health} />{" "}
            {online === null ? "Connecting…" : online ? "Engine online" : "Engine unreachable"}
          </span>
        )}
        <Link className="btn btn--quiet btn--icon" href="/settings/" aria-label="Settings" title="Settings">
          <GearIcon />
        </Link>
      </div>
    </aside>
  );
}
