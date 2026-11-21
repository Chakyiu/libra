/** The only schemes an answer link may use; anything else renders as text. */
const ALLOWED = new Set(["http:", "https:"]);

/**
 * Return `href` when it is safe to put on an `<a>`, else null.
 *
 * The href comes from model-written markdown, so `javascript:` must not reach the DOM.
 * A prefix check is not enough: browsers strip ASCII whitespace and control characters
 * before reading the scheme, so `java\tscript:` or ` JavaScript:` still runs. `URL` applies
 * the same normalisation, so parse the href and inspect the protocol it actually resolves to.
 * Scheme-less hrefs (`/settings/`, `#frag`, `page`) resolve against a placeholder base and
 * keep their http(s) protocol, so they stay allowed and are returned unchanged.
 */
export function safeHref(href?: string): string | null {
  const raw = href?.trim();
  if (!raw) return null;
  let url: URL;
  try {
    url = new URL(raw, "https://libra.invalid/");
  } catch {
    return null;
  }
  return ALLOWED.has(url.protocol) ? raw : null;
}
