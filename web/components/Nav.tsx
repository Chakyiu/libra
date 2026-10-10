"use client";

import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { Chat, api } from "@/lib/api";

const LINKS = [
  { href: "/", label: "Answer" },
  { href: "/discover", label: "Discover" },
  { href: "/images", label: "Images" },
  { href: "/videos", label: "Videos" },
  { href: "/settings", label: "Settings" },
];

export default function Nav() {
  const [chats, setChats] = useState<Chat[]>([]);
  const pathname = usePathname();
  const router = useRouter();

  async function load() {
    try {
      setChats(await api<Chat[]>("/api/chats"));
    } catch {
      setChats([]);
    }
  }

  useEffect(() => {
    load();
    window.addEventListener("vane:chats", load);
    return () => window.removeEventListener("vane:chats", load);
  }, [pathname]);

  return (
    <aside className="w-60 shrink-0 border-r border-edge bg-panel/60 flex flex-col">
      <div className="p-4 border-b border-edge">
        <Link href="/" className="text-lg font-semibold tracking-tight">
          Vane<span className="text-accent">.</span>
        </Link>
        <p className="text-xs text-mute mt-0.5">rust + redis + sqlite</p>
      </div>

      <nav className="p-2 space-y-0.5">
        {LINKS.map((l) => (
          <Link
            key={l.href}
            href={l.href}
            className={`block px-3 py-1.5 rounded text-sm ${
              pathname === l.href ? "bg-edge text-ink" : "text-mute hover:text-ink"
            }`}
          >
            {l.label}
          </Link>
        ))}
      </nav>

      <div className="px-3 pt-2 pb-1 flex items-center justify-between">
        <span className="text-xs uppercase tracking-wide text-mute">Chats</span>
        <button
          onClick={() => router.push("/")}
          className="text-xs text-accent hover:underline"
        >
          + new
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-2 pb-3 space-y-0.5">
        {chats.map((c) => (
          <div key={c.id} className="group flex items-center">
            <Link
              href={`/?chat=${c.id}`}
              className={`flex-1 truncate px-3 py-1.5 rounded text-sm ${
                pathname === "/" ? "text-ink hover:bg-edge" : "text-mute hover:text-ink"
              }`}
            >
              {c.title || "Untitled"}
            </Link>
            <button
              title="Delete chat"
              onClick={async () => {
                await fetch(`/api/chats/${c.id}`, { method: "DELETE" });
                load();
                if (pathname === "/") router.push("/");
              }}
              className="opacity-0 group-hover:opacity-100 text-mute hover:text-red-400 px-2 text-sm"
            >
              ×
            </button>
          </div>
        ))}
        {!chats.length && <p className="px-3 text-xs text-mute">No chats yet</p>}
      </div>
    </aside>
  );
}
