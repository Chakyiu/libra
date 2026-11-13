"use client";

import { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import Sidebar from "@/components/Sidebar";
import { InstanceProvider } from "@/components/instance";
import Unlock from "@/components/Unlock";
import { ShellContext, focusAsk, isTyping } from "@/components/shell-context";
import { ThreadsProvider } from "@/components/threads";
import { getPref, setPref } from "@/lib/prefs";

const NARROW = "(max-width: 760px)";

function pathOf(pathname: string) {
  if (pathname.length > 1 && pathname.endsWith("/")) return pathname.slice(0, -1);
  return pathname;
}

/** A same-origin link whose path is a different page. Query-only links stay in the app. */
function documentUrl(a: HTMLAnchorElement) {
  if ((a.target && a.target !== "_self") || a.hasAttribute("download")) return "";
  const raw = a.getAttribute("href");
  if (!raw || raw.startsWith("#") || raw.startsWith("mailto:") || raw.startsWith("javascript:")) return "";
  let url: URL;
  try {
    url = new URL(raw, window.location.href);
  } catch {
    return "";
  }
  if (url.origin !== window.location.origin) return "";
  if (pathOf(url.pathname) === pathOf(window.location.pathname)) return "";
  return url.pathname + url.search + url.hash;
}

function Frame({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const [drawer, setDrawer] = useState(false);

  // On phones the thread list is a drawer; elsewhere `[` collapses it and the choice sticks.
  const toggleSidebar = useCallback(() => {
    if (matchMedia(NARROW).matches) setDrawer((d) => !d);
    else setPref("sidebar", getPref("sidebar") === "collapsed" ? "expanded" : "collapsed");
  }, []);
  const closeDrawer = useCallback(() => setDrawer(false), []);

  // A focused text field blurs on the press. That reflow can move the control
  // before mouseup, so the click never lands and the next press is the one that works.
  // A link to another page must load that document. The client router otherwise
  // writes the new path and leaves the page that was already on screen.
  useEffect(() => {
    const onPointerDown = (e: PointerEvent) => {
      if (e.button !== 0) return;
      const target = e.target;
      if (!(target instanceof Element)) return;
      if (target.closest("input, textarea, select, option")) return;
      const control = target.closest("a, button, summary, [role='tab']");
      if (!control) return;
      const active = document.activeElement;
      if (!(active instanceof HTMLElement) || active === control || control.contains(active)) return;
      if (!/^(INPUT|TEXTAREA)$/.test(active.tagName) && !active.isContentEditable) return;
      e.preventDefault();
    };
    const onClick = (e: MouseEvent) => {
      if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
      const target = e.target;
      if (!(target instanceof Element)) return;
      const a = target.closest("a");
      if (!(a instanceof HTMLAnchorElement)) return;
      const next = documentUrl(a);
      if (!next) return;
      e.preventDefault();
      e.stopPropagation();
      window.location.assign(next);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("click", onClick, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("click", onClick, true);
    };
  }, []);

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
        <Unlock />
      </ThreadsProvider>
    </InstanceProvider>
  );
}
