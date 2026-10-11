import { host } from "./format";

/** What kind of page a source is — drives the rail filters and the tag on each card. */
export type SourceKind = "web" | "code" | "forum" | "paper" | "video";

export const KIND_LABEL: Record<SourceKind, string> = {
  web: "Web",
  code: "Code",
  forum: "Forums",
  paper: "Papers",
  video: "Video",
};

export const KIND_TAG: Record<SourceKind, string> = {
  web: "Web",
  code: "Code",
  forum: "Forum",
  paper: "Paper",
  video: "Video",
};

export const KIND_CLASS: Record<SourceKind, string> = {
  web: "tag",
  code: "tag tag--code",
  forum: "tag",
  paper: "tag tag--warn",
  video: "tag",
};

const KIND_ORDER: SourceKind[] = ["web", "code", "forum", "paper", "video"];

const RULES: [SourceKind, RegExp][] = [
  ["code", /(^|\.)(github\.com|gitlab\.com|bitbucket\.org|codeberg\.org|sr\.ht|sourceforge\.net|docs\.rs|crates\.io|npmjs\.com|pypi\.org|pkg\.go\.dev|rubygems\.org|packagist\.org|nuget\.org|hex\.pm|gist\.github\.com|githubusercontent\.com)$/],
  ["forum", /(^|\.)(reddit\.com|news\.ycombinator\.com|stackoverflow\.com|stackexchange\.com|superuser\.com|serverfault\.com|askubuntu\.com|lobste\.rs|quora\.com|discord\.com|discourse\.org)$|^(forum|forums|community|discuss|discourse)\./],
  ["paper", /(^|\.)(arxiv\.org|doi\.org|acm\.org|ieee\.org|springer\.com|nature\.com|science\.org|sciencedirect\.com|semanticscholar\.org|researchgate\.net|nih\.gov|biorxiv\.org|medrxiv\.org|ssrn\.com|openreview\.net|jstor\.org|usenix\.org|aclanthology\.org|wiley\.com|plos\.org|frontiersin\.org|mdpi\.com)$/],
  ["video", /(^|\.)(youtube\.com|youtu\.be|vimeo\.com|dailymotion\.com|twitch\.tv|ted\.com)$/],
];

export function kindOf(url: string): SourceKind {
  const h = host(url).toLowerCase();
  for (const [kind, re] of RULES) if (re.test(h)) return kind;
  return "web";
}

/** Kinds present in a source list, in a stable order. */
export function kindsIn(urls: string[]): SourceKind[] {
  const seen = new Set(urls.map(kindOf));
  return KIND_ORDER.filter((k) => seen.has(k));
}
