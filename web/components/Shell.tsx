"use client";

import { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import Sidebar from "@/components/Sidebar";
import { InstanceProvider } from "@/components/instance";
import { ShellContext, focusAsk, isTyping } from "@/components/shell-context";
import { ThreadsProvider } from "@/components/threads";
import { getPref, setPref } from "@/lib/prefs";

const NARROW = "(max-width: 760px)";

function Frame({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const [drawer, setDrawer] = useState(false);

  // On phones the thread list is a drawer; elsewhere `[` collapses it and the choice sticks.
  const toggleSidebar = useCallback(() => {
    if (matchMedia(NARROW).matches) setDrawer((d) => !d);
    else setPref("sidebar", getPref("sidebar") === "collapsed" ? "expanded" : "collapsed");
  }, []);
  const closeDrawer = useCallback(() => setDrawer(false), []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || e.defaultPrevented) return;
      if (e.key === "Escape") {
        setDrawer(false);
        return;
      }
      if (isTyping()) return;
      if (e.key === "/") {
        if (focusAsk()) e.preventDefault();
      } else if (e.key === "[") {
        e.preventDefault();
        toggleSidebar();
      } else if (e.key === "n" || e.key === "N") {
        e.preventDefault();
        router.push("/");
        window.setTimeout(focusAsk, 80);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [router, toggleSidebar]);

  const value = useMemo(() => ({ drawer, toggleSidebar, closeDrawer }), [drawer, toggleSidebar, closeDrawer]);

  return (
    <ShellContext.Provider value={value}>
      <div className="app" data-drawer={drawer ? "open" : undefined}>
        <Suspense fallback={<aside className="sidebar" />}>
          <Sidebar />
        </Suspense>
        <main className="main">{children}</main>
        <div className="drawer-scrim" onClick={closeDrawer} />
      </div>
    </ShellContext.Provider>
  );
}

export default function Shell({ children }: { children: React.ReactNode }) {
  return (
    <InstanceProvider>
      <ThreadsProvider>
        <Frame>{children}</Frame>
      </ThreadsProvider>
    </InstanceProvider>
  );
}
