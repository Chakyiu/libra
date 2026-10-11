"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { Chat, api, deleteChat } from "@/lib/api";

/**
 * What the sidebar knows about a thread asked in this session: its first question stands in for the
 * title until the server names it in the background.
 */
export type Live = { question: string; answering: boolean };

type Threads = {
  chats: Chat[];
  status: "loading" | "ready" | "error";
  reload: () => Promise<void>;
  /** Reload now and twice more: the server names a thread in the background after it answers. */
  settle: () => void;
  live: Record<string, Live>;
  setLive: (id: string, live: Live | null) => void;
  remove: (id: string) => Promise<void>;
};

const ThreadsContext = createContext<Threads | null>(null);

export function ThreadsProvider({ children }: { children: React.ReactNode }) {
  const [chats, setChats] = useState<Chat[]>([]);
  const [status, setStatus] = useState<Threads["status"]>("loading");
  const [live, setLiveMap] = useState<Record<string, Live>>({});
  const timers = useRef<number[]>([]);

  const reload = useCallback(async () => {
    try {
      setChats(await api<Chat[]>("/api/chats"));
      setStatus("ready");
    } catch {
      setStatus((s) => (s === "ready" ? s : "error"));
    }
  }, []);

  const settle = useCallback(() => {
    timers.current.forEach(window.clearTimeout);
    reload();
    timers.current = [4_000, 10_000].map((ms) => window.setTimeout(reload, ms));
  }, [reload]);

  const setLive = useCallback((id: string, value: Live | null) => {
    setLiveMap((m) => {
      const next = { ...m };
      if (value) next[id] = value;
      else delete next[id];
      return next;
    });
  }, []);

  const remove = useCallback(
    async (id: string) => {
      await deleteChat(id);
      await reload();
    },
    [reload],
  );

  useEffect(() => {
    reload();
    const t = timers.current;
    return () => t.forEach(window.clearTimeout);
  }, [reload]);

  const value = useMemo(
    () => ({ chats, status, reload, settle, live, setLive, remove }),
    [chats, status, reload, settle, live, setLive, remove],
  );
  return <ThreadsContext.Provider value={value}>{children}</ThreadsContext.Provider>;
}

export function useThreads() {
  const ctx = useContext(ThreadsContext);
  if (!ctx) throw new Error("useThreads outside ThreadsProvider");
  return ctx;
}
