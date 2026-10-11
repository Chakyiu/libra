"use client";

import { useSyncExternalStore } from "react";

/**
 * Which citation ↔ source pairs are lit. Keys are `${turnKey}:${n}`.
 * A tiny external store so hovering a chip re-renders only the chips and cards it touches.
 */
let hovered: string | null = null;
const flashes = new Map<string, number>();
const listeners = new Set<() => void>();

const emit = () => listeners.forEach((fn) => fn());

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function hover(key: string | null) {
  if (hovered === key) return;
  hovered = key;
  emit();
}

/** Light a pair for a moment — used when a citation is written or clicked. */
export function flash(key: string, ms = 900) {
  window.clearTimeout(flashes.get(key));
  flashes.set(
    key,
    window.setTimeout(() => {
      flashes.delete(key);
      emit();
    }, ms),
  );
  emit();
}

export function useLit(key: string) {
  return useSyncExternalStore(
    subscribe,
    () => hovered === key || flashes.has(key),
    () => false,
  );
}
