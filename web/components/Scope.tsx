"use client";

import { useCallback, useRef, useState } from "react";
import type { ClipboardEvent, DragEvent } from "react";
import { ChevronDownIcon, ClipIcon } from "@/components/icons";
import { useAnswerModel, useInstance } from "@/components/instance";
import { useThreads } from "@/components/threads";
import { FileRef, ensureChat, uploadFiles } from "@/lib/api";
import { modelLabel } from "@/lib/format";
import { setPref, usePref } from "@/lib/prefs";

const IMAGE = /\.(png|jpe?g|gif|webp|bmp)$/i;

export function isImageFile(file: File) {
  return file.type.startsWith("image/") || IMAGE.test(file.name);
}

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
    async (incoming: File[]) => {
      let picked = incoming.filter((f) => f.size > 0);
      if (!picked.length) return;
      const docs = picked.filter((f) => !isImageFile(f));
      let note = "";
      if (docs.length && settings && !settings.defaultEmbeddingModel) {
        note = "Text files are read with an embedding model — pick one in Settings → Answering first.";
        picked = picked.filter(isImageFile);
        if (!picked.length) {
          setError(note);
          return;
        }
      }
      setUploading(true);
      setError(note);
      try {
        await ensureChat(chatId);
        const res = await uploadFiles(chatId, picked);
        if (!res.files.length) throw new Error("the server stored nothing");
        const added = res.files.map((f) => ({ id: f.id, name: f.name }));
        setFiles((fs) => [...fs, ...added]);
        setSelected((sel) => [...sel, ...added.map((f) => f.id)]);
        reload();
      } catch (e: any) {
        const message = e?.name === "AbortError" ? "timed out" : e.message;
        setError(`Couldn’t read that file — ${message}`);
      } finally {
        setUploading(false);
      }
    },
    [chatId, reload, settings],
  );

  return { files, setFiles, selected, setSelected, uploading, error, setError, attach };
}

/** Drop files onto a composer. `over` is true while a file is hovering and the drop is allowed. */
export function useFileDrop(onDropFiles: (files: File[]) => void, disabled: boolean) {
  const [over, setOver] = useState(false);
  const depth = useRef(0);

  const onDragEnter = (e: DragEvent) => {
    if (!Array.from(e.dataTransfer.types).includes("Files")) return;
    e.preventDefault();
    depth.current += 1;
    if (!disabled) setOver(true);
  };
  const onDragOver = (e: DragEvent) => {
    if (!Array.from(e.dataTransfer.types).includes("Files")) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = disabled ? "none" : "copy";
  };
  const onDragLeave = (e: DragEvent) => {
    e.preventDefault();
    depth.current = Math.max(0, depth.current - 1);
    if (depth.current === 0) setOver(false);
  };
  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    depth.current = 0;
    setOver(false);
    if (disabled) return;
    const files = Array.from(e.dataTransfer.files ?? []);
    if (files.length) onDropFiles(files);
  };

  return { over, onDragEnter, onDragOver, onDragLeave, onDrop };
}

/** A screenshot paste has no real filename. `image.png` is the name the browser invents. */
const GENERIC_PASTE_NAME = /^(image\.(png|jpe?g|gif|webp|bmp)|blob)$/i;

const PASTE_EXT: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/jpg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/bmp": "bmp",
};

function clipboardImages(data: DataTransfer | null): File[] {
  if (!data) return [];
  const fromItems: File[] = [];
  for (const item of Array.from(data.items ?? [])) {
    if (item.kind !== "file") continue;
    if (item.type && !item.type.startsWith("image/")) continue;
    const file = item.getAsFile();
    if (file && file.size > 0 && isImageFile(file)) fromItems.push(file);
  }
  if (fromItems.length) return fromItems;
  return Array.from(data.files ?? []).filter((f) => isImageFile(f) && f.size > 0);
}

let pasted = 0;

function namedPaste(file: File, total: number): File {
  if (file.name && !GENERIC_PASTE_NAME.test(file.name)) return file;
  const ext = PASTE_EXT[file.type] || "png";
  const n = ++pasted;
  const label = n === 1 && total === 1 ? "" : ` ${n}`;
  return new File([file], `Pasted image${label}.${ext}`, {
    type: file.type || "image/png",
    lastModified: file.lastModified,
  });
}

/**
 * Attach images from a paste. Words still land in the box.
 * A screenshot has no text, so that paste is cancelled — otherwise some browsers
 * drop the invented filename into the question.
 */
export function onPasteImages(
  e: ClipboardEvent<HTMLTextAreaElement>,
  onAttach: (files: File[]) => void,
  disabled: boolean,
) {
  const data = e.clipboardData;
  if (!data) return;
  // Read the text first. Some browsers clear it once the image item is taken.
  const text = data.getData("text/plain").trim();
  const raw = clipboardImages(data);
  if (!raw.length) return;
  if (!text || raw.some((f) => f.name === text)) e.preventDefault();
  if (!disabled) onAttach(raw.map((file) => namedPaste(file, raw.length)));
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
  const mode = usePref("mode");
  const toggle = (id: string) => onSelect(selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id]);

  return (
    <div className="scope">
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
          title="Attach a text file, PDF, Word document, or image. You can also drop it on the box or paste an image into it."
        >
          <ClipIcon />
          {uploading ? "Reading…" : "Attach"}
        </button>
        <input
          ref={input}
          type="file"
          multiple
          hidden
          accept="image/*,.png,.jpg,.jpeg,.gif,.webp,.bmp,.txt,.md,.csv,.pdf,.docx,text/plain,application/pdf"
          onChange={(e) => {
            // copy first: clearing the input empties its live FileList
            const picked = Array.from(e.target.files ?? []);
            e.target.value = "";
            onAttach(picked);
          }}
        />
        <label
          className="chip chip--select"
          title="The fast model plans the first search, then decides the next round from what came back. It drops sources that do not help, and the rerank model orders what is left. Fast is one round, Balance is two, Deep is three."
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
        <label
          className="chip chip--select"
          title="A short answer leads with the result and stops. A discussion report plans 4–6 sections from the same research and writes them one after another — longer to read, slower to write, still cited."
        >
          <span className="sr-only">Answer length</span>
          <select
            value={mode}
            disabled={disabled}
            onChange={(e) => {
              const v = e.target.value;
              if (v === "answer" || v === "discussion") setPref("mode", v);
            }}
          >
            <option value="answer">Short answer</option>
            <option value="discussion">Discussion report</option>
          </select>
          <ChevronDownIcon />
        </label>
      </div>
      {models.chat.length > 0 && (
        <label className="chip chip--select scope__model" title="Answer model">
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
