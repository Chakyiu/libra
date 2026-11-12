"use client";

import { useState } from "react";
import { CopyIcon, DownloadIcon } from "@/components/icons";
import { copyText, download } from "@/lib/clipboard";
import { fileLabel, fileMime, fileText } from "@/lib/files";

export default function FileBlock({ name, text }: { name: string; text: string }) {
  const [copied, setCopied] = useState(false);
  const body = fileText(text);
  const shown = text.endsWith("\n") ? text.slice(0, -1) : text;

  return (
    <div className="file" role="group" aria-label={`${name} file`}>
      <div className="file__bar">
        <span className="file__name" title={name}>
          {name}
        </span>
        <span className="file__kind">{fileLabel(name)}</span>
        <span className="file__actions">
          <button
            type="button"
            className="btn btn--quiet"
            onClick={async () => {
              await copyText(body);
              setCopied(true);
              window.setTimeout(() => setCopied(false), 1600);
            }}
          >
            <CopyIcon />
            {copied ? "Copied" : "Copy"}
          </button>
          <button type="button" className="btn btn--quiet" onClick={() => download(name, body, fileMime(name))}>
            <DownloadIcon />
            Download
          </button>
        </span>
      </div>
      <pre className="file__body" spellCheck={false}>
        <code>{shown}</code>
      </pre>
    </div>
  );
}
