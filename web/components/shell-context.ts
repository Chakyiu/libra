"use client";

import { createContext, useContext } from "react";

export type ShellState = {
  /** the thread drawer on phones */
  drawer: boolean;
  toggleSidebar: () => void;
  closeDrawer: () => void;
};

export const ShellContext = createContext<ShellState | null>(null);

export function useShell() {
  const ctx = useContext(ShellContext);
  if (!ctx) throw new Error("useShell outside Shell");
  return ctx;
}

export function isTyping() {
  const el = document.activeElement as HTMLElement | null;
  return !!el && (/^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName) || el.isContentEditable);
}

/** Focus whichever ask field the page has: the home ask box or the docked composer. */
export function focusAsk() {
  const box = document.querySelector<HTMLTextAreaElement>("[data-composer], [data-ask-input]");
  if (box && !box.disabled) {
    box.focus();
    return true;
  }
  return false;
}
