"use client";

import { Fragment, createContext, memo, useContext, useMemo } from "react";
import Markdown, { MarkdownToJSX } from "markdown-to-jsx";
import { linkCitations, parseCiteHref, trimPartial } from "@/lib/cite";
import { useLit } from "@/lib/lit";

type CiteHandlers = {
  turnKey: string;
  /** sources available to this answer; citations past it have nothing to point at */
  count: number;
  onCite: (n: number) => void;
  onHover: (n: number | null) => void;
};

const CiteContext = createContext<CiteHandlers | null>(null);

function Cite({ n }: { n: number }) {
  const ctx = useContext(CiteContext);
  const lit = useLit(`${ctx?.turnKey}:${n}`);
  const missing = !ctx || n > ctx.count;
  if (missing) {
    return (
      <span className="cite" data-missing="" title="No matching source">
        {n}
      </span>
    );
  }
  return (
    <button
      type="button"
      className={`cite${lit ? " is-lit" : ""}`}
      aria-label={`Source ${n}`}
      onMouseEnter={() => ctx.onHover(n)}
      onMouseLeave={() => ctx.onHover(null)}
      onFocus={() => ctx.onHover(n)}
      onBlur={() => ctx.onHover(null)}
      onClick={() => ctx.onCite(n)}
    >
      {n}
    </button>
  );
}

function Anchor({ href, children, ...rest }: { href?: string; children?: React.ReactNode }) {
  const refs = parseCiteHref(href);
  if (refs) {
    return refs.length > 1 ? (
      <span className="cite-group">
        {refs.map((n) => (
          <Cite key={n} n={n} />
        ))}
      </span>
    ) : (
      <Cite n={refs[0]} />
    );
  }
  return (
    <a {...rest} href={href} target="_blank" rel="noreferrer noopener">
      {children}
    </a>
  );
}

const OPTIONS: MarkdownToJSX.Options = {
  forceBlock: true,
  forceWrapper: true,
  wrapper: Fragment,
  disableParsingRawHTML: true,
  overrides: {
    a: { component: Anchor },
    img: { props: { loading: "lazy", referrerPolicy: "no-referrer" } },
  },
};

const Prose = memo(function Prose({ source }: { source: string }) {
  return <Markdown options={OPTIONS}>{source}</Markdown>;
});

type AnswerProps = CiteHandlers & {
  text: string;
  writing?: boolean;
};

/** The answer column: serif prose whose [n] citations become chips tied to the sources rail. */
export default function Answer({ text, writing, turnKey, count, onCite, onHover }: AnswerProps) {
  const source = useMemo(() => linkCitations(writing ? trimPartial(text) : text), [text, writing]);
  const handlers = useMemo(() => ({ turnKey, count, onCite, onHover }), [turnKey, count, onCite, onHover]);
  return (
    <CiteContext.Provider value={handlers}>
      <article className={`answer${writing ? " is-writing" : ""}`}>
        <Prose source={source} />
      </article>
    </CiteContext.Provider>
  );
}
