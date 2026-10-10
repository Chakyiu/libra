import Markdown from "markdown-to-jsx";

function CiteLink({ href, children }: { href?: string; children?: React.ReactNode }) {
  if (href?.startsWith("#src-")) {
    return (
      <sup className="text-accent">
        <a href={href}>{children}</a>
      </sup>
    );
  }
  return (
    <a href={href} target="_blank" rel="noreferrer">
      {children}
    </a>
  );
}

/** Renders an answer and turns [n] citations into links to the source list. */
export default function Answer({ text }: { text: string }) {
  const src = text.replace(/\[(\d{1,2})\](?!\()/g, "[$1](#src-$1)");
  return (
    <div className="prose-answer">
      <Markdown
        options={{
          forceBlock: true,
          overrides: {
            a: { component: CiteLink },
            img: { props: { loading: "lazy", className: "max-h-80 rounded" } },
          },
        }}
      >
        {src}
      </Markdown>
    </div>
  );
}
