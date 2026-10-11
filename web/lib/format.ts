const DAY = 86_400_000;

function startOfDay(d: Date) {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

export function toDate(s?: string | null): Date | null {
  if (!s) return null;
  const d = new Date(s);
  return Number.isNaN(d.getTime()) ? null : d;
}

export function clock(d: Date) {
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
}

export type Group = "Today" | "Yesterday" | "Last week" | "Earlier";

export function groupOf(d: Date | null, now = new Date()): Group {
  if (!d) return "Earlier";
  const days = Math.round((startOfDay(now) - startOfDay(d)) / DAY);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return "Last week";
  return "Earlier";
}

/** The short time shown under a thread title; the group heading carries the day. */
export function threadTime(d: Date | null, group: Group) {
  if (!d) return "";
  if (group === "Today" || group === "Yesterday") return clock(d);
  if (group === "Last week") return d.toLocaleDateString([], { weekday: "short" });
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

/** "Asked 11:04", "Asked yesterday 17:22", "Asked Sep 14, 11:04" */
export function askedAt(d: Date | null) {
  if (!d) return "Asked";
  const group = groupOf(d);
  if (group === "Today") return `Asked ${clock(d)}`;
  if (group === "Yesterday") return `Asked yesterday ${clock(d)}`;
  return `Asked ${d.toLocaleDateString([], { month: "short", day: "numeric" })}, ${clock(d)}`;
}

export function shortDate(s?: string | null) {
  const d = toDate(s);
  if (!d) return "";
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString([], { month: "short", day: "numeric", ...(sameYear ? {} : { year: "numeric" }) });
}

export function seconds(ms: number) {
  return ms < 10_000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms / 1000)}s`;
}

export function plural(n: number, one: string, many = `${one}s`) {
  return `${n} ${n === 1 ? one : many}`;
}

/** Thread titles arrive as "🦀 Rust and Redis"; the editorial chrome drops the emoji. */
export function cleanTitle(title?: string | null) {
  return (title ?? "").replace(/^(?:\p{Extended_Pictographic}|\p{Regional_Indicator}|\uFE0F|\u200D)+\s*/u, "").trim();
}

/** Loose key for comparing questions: case, punctuation and spacing don't count. */
export function sameQuestionKey(s: string) {
  return s.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim();
}

export function host(url: string) {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** "openrouter/anthropic/claude-3.5-sonnet" → "openrouter · anthropic/claude-3.5-sonnet" */
export function modelLabel(ref?: string | null) {
  if (!ref) return "";
  const i = ref.indexOf("/");
  return i < 0 ? ref : `${ref.slice(0, i)} · ${ref.slice(i + 1)}`;
}

export function slug(s: string) {
  return (
    s
      .toLowerCase()
      .normalize("NFKD")
      .replace(/[^\p{L}\p{N}]+/gu, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 60) || "answer"
  );
}
