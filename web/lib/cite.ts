import type { SearchResult } from "./api";

/** One or more adjacent bracket citations: [1], [2][3], [1, 4], [2-4]. Not markdown links. */
const RUN = /(?:\[\s*\d{1,3}(?:\s*(?:[,;]|[-–])\s*\d{1,3})*\s*\])+(?!\()/g;
const FENCE = /^\s{0,3}(`{3,}|~{3,})/;

function numbersIn(run: string): number[] {
  const out: number[] = [];
  for (const group of run.match(/\[[^\]]*\]/g) ?? []) {
    for (const part of group.slice(1, -1).split(/[,;]/)) {
      const range = part.split(/[-–]/).map((x) => Number(x.trim()));
      if (range.length === 2 && range[1] > range[0] && range[1] - range[0] <= 20) {
        for (let n = range[0]; n <= range[1]; n++) out.push(n);
      } else if (Number.isFinite(range[0])) {
        out.push(range[0]);
      }
    }
  }
  return out.filter((n, i) => n > 0 && out.indexOf(n) === i);
}

/** Apply `fn` to markdown text outside fenced code blocks and inline code spans. */
function outsideCode(md: string, fn: (text: string) => string) {
  let fence: string | null = null;
  return md
    .split("\n")
    .map((line) => {
      const m = line.match(FENCE);
      if (m) {
        if (!fence) fence = m[1][0];
        else if (m[1][0] === fence) fence = null;
        return line;
      }
      if (fence) return line;
      return line
        .split(/(`+[^`]*`+)/)
        .map((part, i) => (i % 2 ? part : fn(part)))
        .join("");
    })
    .join("\n");
}

/**
 * Rewrite citation runs into `#cite-1-3` links that the answer renderer turns into chips,
 * pulling each chip up against the word it cites ("second [2]" → "second²") unless it opens a list item.
 */
export function linkCitations(md: string): string {
  return outsideCode(md, (text) =>
    text
      .replace(RUN, (run) => {
        const ns = numbersIn(run);
        return ns.length ? `[${ns.join(",")}](#cite-${ns.join("-")})` : run;
      })
      .replace(/([ \t]+)(?=\[[\d,]+\]\(#cite-)/g, (ws: string, _: string, at: number, line: string) =>
        /^\s*(?:[-*+>]|\d+[.)])?\s*$/.test(line.slice(0, at)) ? ws : "",
      ),
  );
}

export function parseCiteHref(href?: string): number[] | null {
  const m = href?.match(/^#cite-([\d-]+)$/);
  return m ? m[1].split("-").map(Number) : null;
}

/** While streaming, hide a citation that has only half arrived ("… second [1"). */
export function trimPartial(md: string) {
  return md.replace(/\[[\d\s,;–-]*$/, "");
}

/** Every citation in reading order, repeats included. */
export function citationsIn(md: string): number[] {
  const out: number[] = [];
  outsideCode(md, (text) => {
    for (const m of text.matchAll(RUN)) out.push(...numbersIn(m[0]));
    return text;
  });
  return out;
}

/** The answer as portable markdown, with the sources it cites listed underneath. */
export function answerMarkdown(answer: string, sources: SearchResult[], question?: string) {
  const lines: string[] = [];
  if (question) lines.push(`# ${question}`, "");
  lines.push(answer.trim());
  if (sources.length) {
    lines.push("", "## Sources", "");
    sources.forEach((s, i) => lines.push(`[${i + 1}] ${s.title || s.url} — ${s.url}`));
  }
  return lines.join("\n") + "\n";
}
