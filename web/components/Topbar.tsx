"use client";

import { useSyncExternalStore } from "react";
import Link from "next/link";
import { GearIcon, MenuIcon, MoonIcon, SunIcon } from "@/components/icons";
import { useShell } from "@/components/shell-context";
import { setPref, useMedia, usePref, useResolvedTheme } from "@/lib/prefs";

export const VERSION = process.env.NEXT_PUBLIC_APP_VERSION ?? "";

const noop = () => () => {};

/** The host this instance is served from — client-only, so prerendered pages hydrate cleanly. */
export function useHost() {
  return useSyncExternalStore(noop, () => window.location.host, () => "");
}

export function ThemeToggle() {
  const theme = useResolvedTheme();
  const next = theme === "dark" ? "light" : "dark";
  return (
    <button className="btn btn--quiet" onClick={() => setPref("theme", next)} aria-label={`Switch to ${next} theme`} title={`Switch to ${next} theme`}>
      {theme === "dark" ? <MoonIcon /> : <SunIcon />}
      <span className="btn__label">{theme === "dark" ? "Dark" : "Light"}</span>
    </button>
  );
}

type TopbarProps = {
  title?: string;
  /** muted text in place of a title (home shows the host) */
  meta?: string;
  actions?: React.ReactNode;
  settingsLink?: boolean;
};

export default function Topbar({ title, meta, actions, settingsLink = true }: TopbarProps) {
  const { drawer, toggleSidebar } = useShell();
  const sidebar = usePref("sidebar");
  const narrow = useMedia("(max-width: 760px)");
  const expanded = narrow ? drawer : sidebar !== "collapsed";

  return (
    <header className="topbar">
      <button
        className="btn btn--quiet btn--icon"
        onClick={toggleSidebar}
        aria-expanded={expanded}
        aria-label={expanded ? "Hide threads" : "Show threads"}
        title={`${expanded ? "Hide" : "Show"} threads  [`}
      >
        <MenuIcon />
      </button>
      {title ? <span className="topbar__title">{title}</span> : meta ? <span className="meta topbar__host">{meta}</span> : null}
      <span className="topbar__spacer" />
      {actions}
      <ThemeToggle />
      {settingsLink && (
        <Link className="btn btn--quiet" href="/settings/" aria-label="Settings">
          <GearIcon size={14} className="topbar__icon-narrow" />
          <span className="btn__label">Settings</span>
        </Link>
      )}
    </header>
  );
}
