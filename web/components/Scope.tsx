"use client";

import { useCallback, useRef, useState } from "react";
import { ChevronDownIcon, ClipIcon } from "@/components/icons";
import { useAnswerModel, useInstance } from "@/components/instance";
import { useThreads } from "@/components/threads";
import { FileRef, ensureChat, uploadFiles } from "@/lib/api";
import { modelLabel } from "@/lib/format";
import { setPref, usePref } from "@/lib/prefs";

/**
 * Files attached to a thread and which of them the next question is scoped to.
 * The server answers from the selected files instead of searching, so "Web" means "no files selected".
 */
export function useAttachments(chatId: string) {
  const { reload } = useThreads();
  const { settings } = useInstance();
  const [files, setFiles] = useState<FileRef[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState("");

  const attach = useCallback(
    async (picked: File[]) => {
      if (!picked.length) return;
      if (settings && !settings.defaultEmbeddingModel) {
        setError("Files are read with an embedding model — pick one in Settings → Answering first.");
        return;
      }
      setUploading(true);
      setError("");
      try {
        await ensureChat(chatId);
        const res = await uploadFiles(chatId, picked);
        if (!res.files.length) throw new Error("the server stored nothing");
        const added = res.files.map((f) => ({ id: f.id, name: f.name }));
        setFiles((fs) => [...fs, ...added]);
        setSelected((sel) => [...sel, ...added.map((f) => f.id)]);
        reload();
      } catch (e: any) {
        setError(`Couldn’t read that file — ${e.message}`);
      } finally {
        setUploading(false);
      }
    },
    [chatId, reload, settings],
  );

  return { files, setFiles, selected, setSelected, uploading, error, setError, attach };
}

type ScopeProps = {
  files: FileRef[];
  selected: string[];
  onSelect: (ids: string[]) => void;
  onAttach: (files: File[]) => void;
  uploading: boolean;
  disabled?: boolean;
};

export default function Scope({ files, selected, onSelect, onAttach, uploading, disabled }: ScopeProps) {
  const input = useRef<HTMLInputElement>(null);
  const { models } = useInstance();
  const [model, setModel] = useAnswerModel();
  const depth = usePref("depth");
  const toggle = (id: string) => onSelect(selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id]);

  return (
    <div className="chips" role="group" aria-label="Answer from">
      <button
        type="button"
        className="chip"
        aria-pressed={selected.length === 0}
        onClick={() => onSelect([])}
        disabled={disabled}
        title="Search the web for the answer"
      >
        <i className="chip__led" />
        Web
      </button>
      {files.map((f) => (
        <button
          key={f.id}
          type="button"
          className="chip chip--file"
          aria-pressed={selected.includes(f.id)}
          onClick={() => toggle(f.id)}
          disabled={disabled}
          title={`Answer from ${f.name}`}
        >
          <i className="chip__led" />
          <span>{f.name}</span>
        </button>
      ))}
      <button
        type="button"
        className="chip chip--action"
        onClick={() => input.current?.click()}
        disabled={disabled || uploading}
        title="Attach text, Markdown, CSV, PDF or Word files"
      >
        <ClipIcon />
        {uploading ? "Reading…" : "Attach"}
      </button>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          // copy first: clearing the input empties its live FileList
          const picked = Array.from(e.target.files ?? []);
          e.target.value = "";
          onAttach(picked);
        }}
      />
      <label
        className="chip chip--select"
        title="Fast decides one search and runs it. Balance does that, then searches again if something is missing. Deep runs three rounds."
      >
        <span className="sr-only">Search depth</span>
        <select
          value={depth}
          disabled={disabled}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "fast" || v === "balance" || v === "deep") setPref("depth", v);
          }}
        >
          <option value="fast">Fast search</option>
          <option value="balance">Balance search</option>
          <option value="deep">Deep search</option>
        </select>
        <ChevronDownIcon />
      </label>
      {models.chat.length > 0 && (
        <label className="chip chip--select" title="Answer model">
          <span className="sr-only">Answer model</span>
          <select value={model} onChange={(e) => setModel(e.target.value)} disabled={disabled}>
            {models.chat.map((m) => (
              <option key={m} value={m}>
                {modelLabel(m)}
              </option>
            ))}
          </select>
          <ChevronDownIcon />
        </label>
      )}
    </div>
  );
}
