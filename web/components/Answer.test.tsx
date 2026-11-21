import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import Answer from "@/components/Answer";
import { safeHref } from "@/lib/safeHref";

const noop = () => {};

/** Render one answer body the way the thread does, then return the HTML. */
function render(text: string) {
  return renderToStaticMarkup(<Answer text={text} turnKey="t" count={3} onCite={noop} onHover={noop} />);
}

describe("safeHref", () => {
  it("keeps http, https, relative and scheme-less hrefs", () => {
    for (const href of ["https://example.com/a?b=1", "http://example.com", "HTTPS://EXAMPLE.COM", "/settings/", "#cite-2", "page"]) {
      expect(safeHref(href), href).toBe(href);
    }
  });

  it("drops dangerous schemes, including the whitespace and case variants a prefix check misses", () => {
    for (const href of [
      "javascript:alert(1)",
      "JAVASCRIPT:alert(1)",
      " JavaScript:alert(1)",
      "java\tscript:alert(1)",
      "java\nscript:alert(1)",
      "\u0000javascript:alert(1)",
      "vbscript:msgbox(1)",
      "data:text/html,<script>alert(1)</script>",
      "file:///etc/passwd",
    ]) {
      expect(safeHref(href), JSON.stringify(href)).toBeNull();
    }
  });

  it("drops empty and unparseable hrefs", () => {
    expect(safeHref(undefined)).toBeNull();
    expect(safeHref("   ")).toBeNull();
    expect(safeHref("https://exa mple.com/%")).toBeNull();
  });
});

describe("Answer links", () => {
  it("renders [x](javascript:alert(1)) without an anchor", () => {
    // markdown-to-jsx reads a leading `[x]` as a task-list checkbox, so this exact form
    // never became a link; keep it inert either way.
    const html = render("[x](javascript:alert(1))");
    expect(html).not.toContain("<a ");
    expect(html).not.toContain("href=");
  });

  it("renders a javascript: link as inert text", () => {
    const html = render("[click](javascript:alert(1))");
    expect(html).toContain(">click<");
    expect(html).not.toContain("<a ");
    expect(html).not.toContain("href=");
  });

  it("renders a data: link as inert text", () => {
    const html = render("[click](data:text/html,<b>hi</b>)");
    expect(html).toContain(">click<");
    expect(html).not.toContain("<a ");
    expect(html).not.toContain("href=");
  });

  it("keeps an ordinary http(s) source link clickable and hardened", () => {
    const html = render("see [example](https://example.com/a)");
    expect(html).toContain('href="https://example.com/a"');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noreferrer noopener"');
  });

  it("keeps a relative link clickable", () => {
    expect(render("see [settings](/settings/)")).toContain('href="/settings/"');
  });

  it("still turns [2] into a citation chip", () => {
    const html = render("second [2]");
    expect(html).toContain('aria-label="Source 2"');
    expect(html).not.toContain("href=");
  });

  it("leaves a citation past the given sources inert", () => {
    expect(render("ninth [9]")).toContain('data-missing=""');
  });
});
