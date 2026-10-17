"use client";

import { useSyncExternalStore } from "react";

/** Per-browser reading preferences. Keys match the inline boot script in app/layout.tsx. */
export type ThemePref = "light" | "dark" | "system";

export type FacePref = "default" | "serif" | "sans" | "mono";

export type Prefs = {
  theme: ThemePref;
  sidebar: "expanded" | "collapsed";
  /** answer column width in rem; 0 fills the page */
  col: number;
  face: FacePref;
  /** base text size for the whole app, in px */
  size: number;
  /** line spacing for the whole app */
  leading: number;
  /** a font installed on this computer; empty uses the typeface above */
  font: string;
  /** open the sources rail by default on wide windows */
  rail: boolean;
  /** answer model; empty follows the instance default */
  model: string;
  /** how many search rounds a question gets */
  depth: "fast" | "balance" | "deep";
  /** a short cited answer, or a long-form report written section by section */
  mode: "answer" | "discussion";
};

export const SIZE_MIN = 13;
export const SIZE_MAX = 22;
export const LEADING_MIN = 1.3;
export const LEADING_MAX = 1.85;

export const PREF_DEFAULTS: Prefs = {
  theme: "light",
  sidebar: "expanded",
  col: 0,
  face: "default",
  size: 15,
  leading: 1.5,
  font: "",
  rail: true,
  model: "",
  depth: "balance",
  mode: "answer",
};

const KEY: Record<keyof Prefs, string> = {
  theme: "libra:theme",
  sidebar: "libra:sidebar",
  col: "libra:col",
  face: "libra:face",
  size: "libra:size",
  leading: "libra:leading",
  font: "libra:font",
  rail: "libra:rail",
  model: "libra:model",
  depth: "libra:depth",
  mode: "libra:mode",
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
      return (raw === "serif" || raw === "sans" || raw === "mono" ? raw : "default") as Prefs[K];
    case "size":
      return inRange(Number(raw), SIZE_MIN, SIZE_MAX, fallback as number) as Prefs[K];
    case "leading":
      return inRange(Number(raw), LEADING_MIN, LEADING_MAX, fallback as number) as Prefs[K];
    case "font":
      return safeFontFamily(raw) as Prefs[K];
    case "col": {
      const n = Number(raw);
      return (Number.isFinite(n) && (n === 0 || (n >= 28 && n <= 80)) ? n : fallback) as Prefs[K];
    }
    case "rail":
      return (raw !== "0") as Prefs[K];
    case "depth":
      return (raw === "fast" || raw === "deep" ? raw : "balance") as Prefs[K];
    case "mode":
      return (raw === "discussion" ? raw : "answer") as Prefs[K];
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

/** A font-family list, with anything that is not a family name removed. */
export function safeFontFamily(raw: string): string {
  return raw.replace(/[^A-Za-z0-9 ,.'"-]/g, "").slice(0, 80);
}

function inRange(n: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(n) || n < min || n > max) return fallback;
  return Math.round(n * 100) / 100;
}

function setOrClear(root: HTMLElement, name: string, value: string | null) {
  if (value == null) root.style.removeProperty(name);
  else root.style.setProperty(name, value);
}

/** Mirror the stored prefs onto <html>, where the CSS reads them. */
export function applyPrefs() {
  const root = document.documentElement;
  root.dataset.themePref = getPref("theme");
  root.dataset.theme = resolvedTheme();
  root.dataset.sidebar = getPref("sidebar");
  const face = getPref("face");
  if (face === "default") delete root.dataset.face;
  else root.dataset.face = face;
  const col = getPref("col");
  root.style.setProperty("--col", col > 0 ? `${col}rem` : "none");
  const size = getPref("size");
  setOrClear(root, "--ui-size", size === PREF_DEFAULTS.size ? null : `${size}px`);
  const leading = getPref("leading");
  setOrClear(root, "--ui-leading", leading === PREF_DEFAULTS.leading ? null : String(leading));
  const font = safeFontFamily(getPref("font")).trim();
  if (font) {
    root.dataset.font = "custom";
    root.style.setProperty("--font-custom", font);
  } else {
    delete root.dataset.font;
    root.style.removeProperty("--font-custom");
  }
  const chrome = resolvedTheme() === "dark" ? "#2a2724" : "#faf8f5";
  document.querySelectorAll('meta[name="theme-color"]').forEach((node, index) => {
    if (index > 0) {
      node.remove();
      return;
    }
    node.setAttribute("content", chrome);
    node.removeAttribute("media");
  });
}

function notify() {
  applyPrefs();
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
