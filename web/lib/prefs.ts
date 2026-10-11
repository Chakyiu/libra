"use client";

import { useSyncExternalStore } from "react";

/** Per-browser reading preferences. Keys match the inline boot script in app/layout.tsx. */
export type ThemePref = "light" | "dark" | "system";

export type Prefs = {
  theme: ThemePref;
  sidebar: "expanded" | "collapsed";
  /** answer column width in rem; 0 fills the page */
  col: number;
  face: "serif" | "sans";
  /** open the sources rail by default on wide windows */
  rail: boolean;
  /** answer model; empty follows the instance default */
  model: string;
};

export const PREF_DEFAULTS: Prefs = {
  theme: "light",
  sidebar: "expanded",
  col: 0,
  face: "serif",
  rail: true,
  model: "",
};

const KEY: Record<keyof Prefs, string> = {
  theme: "libra:theme",
  sidebar: "libra:sidebar",
  col: "libra:col",
  face: "libra:face",
  rail: "libra:rail",
  model: "libra:model",
};

const listeners = new Set<() => void>();
const DARK = "(prefers-color-scheme: dark)";

function parse<K extends keyof Prefs>(key: K, raw: string | null): Prefs[K] {
  const fallback = PREF_DEFAULTS[key];
  if (raw == null) return fallback;
  switch (key) {
    case "theme":
      return (["light", "dark", "system"].includes(raw) ? raw : fallback) as Prefs[K];
    case "sidebar":
      return (raw === "collapsed" ? "collapsed" : "expanded") as Prefs[K];
    case "face":
      return (raw === "sans" ? "sans" : "serif") as Prefs[K];
    case "col": {
      const n = Number(raw);
      return (Number.isFinite(n) && (n === 0 || (n >= 28 && n <= 80)) ? n : fallback) as Prefs[K];
    }
    case "rail":
      return (raw !== "0") as Prefs[K];
    default:
      return raw as Prefs[K];
  }
}

export function getPref<K extends keyof Prefs>(key: K): Prefs[K] {
  try {
    return parse(key, localStorage.getItem(KEY[key]));
  } catch {
    return PREF_DEFAULTS[key];
  }
}

export function resolvedTheme(): "light" | "dark" {
  const pref = getPref("theme");
  if (pref === "system") return matchMedia(DARK).matches ? "dark" : "light";
  return pref;
}

/** Mirror the stored prefs onto <html>, where the CSS reads them. */
function apply() {
  const root = document.documentElement;
  root.dataset.themePref = getPref("theme");
  root.dataset.theme = resolvedTheme();
  root.dataset.sidebar = getPref("sidebar");
  root.dataset.face = getPref("face");
  const col = getPref("col");
  root.style.setProperty("--col", col > 0 ? `${col}rem` : "none");
}

function notify() {
  apply();
  listeners.forEach((fn) => fn());
}

export function setPref<K extends keyof Prefs>(key: K, value: Prefs[K]) {
  try {
    const raw = typeof value === "boolean" ? (value ? "1" : "0") : String(value);
    if (value === PREF_DEFAULTS[key] && key === "model") localStorage.removeItem(KEY[key]);
    else localStorage.setItem(KEY[key], raw);
  } catch {
    /* private mode: the pref lasts for this page only */
  }
  notify();
}

let wired = false;
function subscribe(fn: () => void) {
  if (!wired && typeof window !== "undefined") {
    wired = true;
    window.addEventListener("storage", (e) => {
      if (e.key?.startsWith("libra:")) notify();
    });
    matchMedia(DARK).addEventListener("change", () => {
      if (getPref("theme") === "system") notify();
    });
  }
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function usePref<K extends keyof Prefs>(key: K): Prefs[K] {
  return useSyncExternalStore(
    subscribe,
    () => getPref(key),
    () => PREF_DEFAULTS[key],
  );
}

export function useResolvedTheme(): "light" | "dark" {
  return useSyncExternalStore(subscribe, resolvedTheme, () => "light");
}

export function useMedia(query: string, serverValue = false): boolean {
  return useSyncExternalStore(
    (fn) => {
      const mq = matchMedia(query);
      mq.addEventListener("change", fn);
      return () => mq.removeEventListener("change", fn);
    },
    () => matchMedia(query).matches,
    () => serverValue,
  );
}
