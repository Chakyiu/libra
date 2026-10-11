import type { FileRef } from "./api";

/** Home → thread: the files a first question was scoped to travel with it, not through the URL. */
export type Handoff = { files: FileRef[]; selected: string[] };

const pending = new Map<string, Handoff>();

export function handOff(chatId: string, value: Handoff) {
  pending.set(chatId, value);
}

/** Pure read, so it is safe in a state initializer (StrictMode calls those twice). */
export function peekHandoff(chatId: string): Handoff | undefined {
  return pending.get(chatId);
}

export function clearHandoff(chatId: string) {
  pending.delete(chatId);
}
