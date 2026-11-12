/** A file Libra can hand back for the user to copy or download. */
export type AnswerFile = {
  name: string;
};

/** Bare fences in these languages are files even without a filename. Other languages need one. */
const LANG_DEFAULT: Record<string, string> = {
  md: "notes.md",
  markdown: "notes.md",
  bash: "script.sh",
  sh: "script.sh",
  shell: "script.sh",
  zsh: "script.sh",
  script: "script.sh",
  txt: "notes.txt",
  text: "notes.txt",
};

const EXT_ALIAS: Record<string, string> = {
  markdown: "md",
  text: "txt",
};

const MIME: Record<string, string> = {
  md: "text/markdown;charset=utf-8",
  html: "text/html;charset=utf-8",
  htm: "text/html;charset=utf-8",
  css: "text/css;charset=utf-8",
  csv: "text/csv;charset=utf-8",
  json: "application/json;charset=utf-8",
  xml: "application/xml;charset=utf-8",
  svg: "image/svg+xml",
  js: "text/javascript;charset=utf-8",
  mjs: "text/javascript;charset=utf-8",
  cjs: "text/javascript;charset=utf-8",
  ts: "text/typescript;charset=utf-8",
  tsx: "text/typescript;charset=utf-8",
  jsx: "text/javascript;charset=utf-8",
  py: "text/x-python;charset=utf-8",
  sh: "text/x-shellscript;charset=utf-8",
  bash: "text/x-shellscript;charset=utf-8",
  zsh: "text/x-shellscript;charset=utf-8",
  yaml: "text/yaml;charset=utf-8",
  yml: "text/yaml;charset=utf-8",
  txt: "text/plain;charset=utf-8",
};

const FENCE = /^(\s{0,3})(`{3,}|~{3,})(.*)$/;

/**
 * One path segment safe to use as a download name.
 * `file:` may name anything (`app.py`, `Dockerfile`, `.gitignore`).
 * A fence that merely names a language is not a filename.
 */
export function sanitizeFileName(raw: string): string | null {
  if (raw.split(/[/\\]/).some((part) => part === "..")) return null;
  const base = raw.split(/[/\\]/).pop()?.trim() ?? "";
  if (!base || base === "." || base === "..") return null;

  const dotfile = base.startsWith(".") && !base.startsWith("..");
  let cleaned = base
    .replace(/[^\p{L}\p{N}._-]+/gu, "-")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 80);
  cleaned = dotfile ? `.${cleaned.replace(/^\.+/, "")}` : cleaned.replace(/^\.+/, "");
  if (!cleaned || cleaned === "." || cleaned.includes("..")) return null;
  if (dotfile && !cleaned.slice(1).includes(".")) return cleaned;

  const match = cleaned.match(/^(.*)\.([A-Za-z0-9]{1,12})$/);
  if (!match) return /^[\p{L}\p{N}_-][\p{L}\p{N}._-]{0,60}$/u.test(cleaned) ? cleaned : null;
  const stem = match[1].replace(/\.$/, "");
  if (!stem) return null;
  const ext = EXT_ALIAS[match[2].toLowerCase()] ?? match[2].toLowerCase();
  return `${stem}.${ext}`;
}

/** A name the user would save, rather than a language tag such as `python`. */
function isFileName(name: string): boolean {
  if (name.startsWith(".") && name.length > 1 && !name.slice(1).includes(".")) return true;
  return name.lastIndexOf(".") > 0;
}

function splitName(name: string): { stem: string; ext: string } {
  if (name.startsWith(".") && !name.slice(1).includes(".")) return { stem: name, ext: "" };
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return { stem: name, ext: "" };
  return { stem: name.slice(0, dot), ext: name.slice(dot) };
}

function uniqueName(name: string, used: Map<string, number>): string {
  const { stem, ext } = splitName(name);
  let candidate = name;
  let n = 2;
  while (used.has(candidate.toLocaleLowerCase())) {
    candidate = `${stem}-${n}${ext}`;
    n++;
  }
  used.set(candidate.toLocaleLowerCase(), 1);
  return candidate;
}

function pickFileName(raw: string): string | null {
  if (!raw) return null;
  const tokens = raw.split(/\s+/);
  const last = sanitizeFileName(tokens[tokens.length - 1] ?? "");
  if (raw.includes(" ") && last && isFileName(last)) {
    const whole = sanitizeFileName(raw);
    const ext = splitName(last).ext;
    if (whole && ext && whole.endsWith(ext)) return whole;
  }
  for (const token of tokens) {
    const one = sanitizeFileName(token);
    if (one && isFileName(one)) return one;
  }
  return null;
}

function nameFromInfo(info: string, used: Map<string, number>): string | null {
  if (!info) return null;

  if (/^file:/i.test(info)) {
    const raw = info.slice(5).trim();
    const first = raw.split(/\s+/)[0] ?? "";
    const preferred = sanitizeFileName(first) ?? sanitizeFileName(raw);
    return preferred ? uniqueName(preferred, used) : null;
  }

  const parts = info.split(/\s+/);
  let lang = parts[0];
  let rest = parts.slice(1).join(" ");
  const colon = lang.match(/^([A-Za-z0-9_+#]+):(.+)$/);
  if (colon) {
    lang = colon[1];
    rest = [colon[2], rest].filter(Boolean).join(" ");
  }

  const named = pickFileName(rest) ?? pickFileName(info);
  if (named) return uniqueName(named, used);

  const fallback = LANG_DEFAULT[lang.toLowerCase()];
  return fallback ? uniqueName(fallback, used) : null;
}

/**
 * Turn a fence the user can keep into `file:name.ext`.
 * `file:app.py`, `python app.py`, and a bare shell or markdown fence qualify.
 * A language tag alone (`python`) stays code.
 * An opener still being typed (the last line, with no newline yet) is left alone.
 */
export function promoteFiles(md: string): string {
  const text = md.replace(/\r\n?/g, "\n");
  const lines = text.split("\n");
  const used = new Map<string, number>();
  let open: { char: string; len: number } | null = null;

  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(FENCE);
    if (!m) continue;
    const marker = m[2];
    const info = m[3].trim();
    const lineComplete = i < lines.length - 1 || text.endsWith("\n");

    if (open) {
      if (marker[0] === open.char && marker.length >= open.len && info === "") open = null;
      continue;
    }
    if (!lineComplete) continue;

    const name = nameFromInfo(info, used);
    if (name) lines[i] = `${m[1]}${marker}file:${name}`;
    open = { char: marker[0], len: marker.length };
  }
  return lines.join("\n");
}

/** The file a promoted fence names, checked again so a raw `file:` fence cannot escape the directory. */
export function fileFromLang(lang?: string): AnswerFile | null {
  if (!lang || !/^file:/i.test(lang)) return null;
  const name = sanitizeFileName(lang.slice(5).trim());
  return name ? { name } : null;
}

export function fileLabel(name: string): string {
  const { ext } = splitName(name);
  return ext ? ext.slice(1) : "file";
}

/** Text files end in a newline, so a downloaded script runs. */
export function fileText(text: string): string {
  return text.endsWith("\n") ? text : `${text}\n`;
}

export function fileMime(name: string): string {
  const { ext } = splitName(name);
  return MIME[ext.slice(1).toLowerCase()] ?? "text/plain;charset=utf-8";
}
