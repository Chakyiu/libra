"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import Topbar from "@/components/Topbar";
import { ChevronDownIcon, ChevronUpIcon, CloseIcon, PlusIcon, RerunIcon, SparkleIcon } from "@/components/icons";
import { api, putJson } from "@/lib/api";
import { askedAt, shortDate } from "@/lib/format";

type Job = {
  id: string;
  name: string;
  kind: "command" | "prompt";
  /** Standard 5-field cron; the server stores it with a leading seconds field. */
  cron: string;
  timezone: string;
  enabled: boolean;
  image: string;
  command: string;
  prompt: string;
  memory: string;
  max_duration_secs: number;
  notify_url: string;
  next_run_at: string;
  last_run_at: string;
  last_status: string;
};

type JobRun = {
  id: number;
  started_at: string;
  finished_at: string;
  status: string;
  exit_code: number | null;
  output: string;
  detail: string;
  /** Set for prompt jobs: the answer is a normal thread. */
  chat_id: string;
};

/** What the AI helper answers: the job it stored, plus how it read the request. */
type Created = { job: Job; why: string; notes: string[] };

const BLANK: Job = {
  id: "",
  name: "",
  kind: "command",
  cron: "0 7 * * *",
  timezone: "",
  enabled: true,
  image: "alpine",
  command: "",
  prompt: "",
  memory: "256M",
  max_duration_secs: 300,
  notify_url: "",
  next_run_at: "",
  last_run_at: "",
  last_status: "",
};

export default function Automator() {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [draft, setDraft] = useState<Job | null>(null);
  const [log, setLog] = useState<{ job: Job; runs: JobRun[] } | null>(null);
  const [error, setError] = useState("");
  const [idea, setIdea] = useState("");
  const [making, setMaking] = useState(false);
  const [made, setMade] = useState<Created | null>(null);

  const load = useCallback(async () => {
    try {
      setJobs(await api<Job[]>("/api/jobs"));
    } catch (e) {
      setError((e as Error).message);
    }
  }, []);

  useEffect(() => {
    document.title = "Automator — Libra";
    load();
  }, [load]);

  // A run in flight is the only reason to keep polling.
  useEffect(() => {
    if (!jobs.some((j) => j.last_status === "running")) return;
    const timer = setInterval(load, 2000);
    return () => clearInterval(timer);
  }, [jobs, load]);

  // One sentence to the helper: it drafts a job and stores it switched on.
  async function createFromText() {
    const text = idea.trim();
    if (!text || making) return;
    setError("");
    setMade(null);
    setMaking(true);
    try {
      setMade(
        await api<Created>("/api/jobs/from-text", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ text }),
        }),
      );
      setIdea("");
      await load();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setMaking(false);
    }
  }

  async function save() {
    if (!draft) return;
    setError("");
    try {
      if (draft.id) await putJson(`/api/jobs/${draft.id}`, draft);
      else await api("/api/jobs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(draft) });
      setDraft(null);
      await load();
    } catch (e) {
      setError((e as Error).message);
    }
  }

  async function run(job: Job) {
    setError("");
    try {
      await api(`/api/jobs/${job.id}/run`, { method: "POST" });
      await load();
    } catch (e) {
      setError((e as Error).message);
    }
  }

  async function toggle(job: Job) {
    try {
      await putJson(`/api/jobs/${job.id}`, { ...job, enabled: !job.enabled });
      await load();
    } catch (e) {
      setError((e as Error).message);
    }
  }

  async function remove(job: Job) {
    if (!window.confirm(`Delete “${job.name}”? Its run history goes with it.`)) return;
    try {
      await api(`/api/jobs/${job.id}`, { method: "DELETE" });
      if (log?.job.id === job.id) setLog(null);
      await load();
    } catch (e) {
      setError((e as Error).message);
    }
  }

  async function showLog(job: Job) {
    if (log?.job.id === job.id) return setLog(null);
    try {
      setLog({ job, runs: await api<JobRun[]>(`/api/jobs/${job.id}/runs?limit=10`) });
    } catch (e) {
      setError((e as Error).message);
    }
  }

  return (
    <>
      <Topbar title="Automator" />
      <div className="page">
        <div className="page__inner">
          <header className="page__head">
            <p className="kicker">On a schedule</p>
            <h2>Automator</h2>
            <p>
              Timely jobs Libra runs for you. A command job runs one shell line in a throwaway microVM on the machine
              serving Libra; a prompt job asks a saved question and files the answer as a thread.
            </p>
          </header>

          {!draft && (
            <form
              className="idea"
              onSubmit={(e) => {
                e.preventDefault();
                createFromText();
              }}
            >
              <input
                type="text"
                className="idea__input"
                value={idea}
                aria-label="Say what you want Libra to do on a schedule"
                placeholder="Every weekday morning at 8 London time, tell me what changed in the Rust release notes"
                onChange={(e) => setIdea(e.target.value)}
              />
              <button className="btn btn--primary" type="submit" disabled={making || !idea.trim()}>
                <SparkleIcon /> {making ? "Thinking…" : "Make it a job"}
              </button>
              <button className="btn btn--quiet" type="button" onClick={() => setDraft({ ...BLANK })}>
                <PlusIcon /> By hand
              </button>
            </form>
          )}

          {made && (
            <div className="callout callout--made">
              <p className="callout__head">
                <b>Scheduled “{made.job.name}”.</b>
                <span className="meta">
                  <code>{made.job.cron}</code> · {made.job.timezone || "server time"} · next {when(made.job.next_run_at)}
                </span>
              </p>
              {made.why && <p className="callout__why">{made.why}</p>}
              {!!made.notes.length && (
                <ul className="callout__notes">
                  {made.notes.map((note, i) => (
                    <li key={i}>{note}</li>
                  ))}
                </ul>
              )}
              <div className="job__actions">
                <button
                  className="btn btn--quiet"
                  type="button"
                  onClick={() => {
                    setDraft({ ...made.job });
                    setMade(null);
                  }}
                >
                  Adjust
                </button>
                <button className="btn btn--quiet" type="button" onClick={() => run(made.job)}>
                  <RerunIcon /> Run now
                </button>
                <button className="btn btn--quiet btn--icon" type="button" title="Dismiss" onClick={() => setMade(null)}>
                  <CloseIcon />
                </button>
              </div>
            </div>
          )}

          {error && (
            <p className="callout callout--error" role="alert">
              <b>Something went wrong.</b> {error}
            </p>
          )}

          {draft && (
            <JobForm
              draft={draft}
              onChange={setDraft}
              onSave={save}
              onCancel={() => {
                setDraft(null);
                setError("");
              }}
            />
          )}

          {!jobs.length && !draft ? (
            <p className="callout">
              Nothing scheduled yet. Describe a job above and the helper will set it up, or write it by hand — start
              small: <code>0 7 * * *</code> running <code>curl -s https://example.com | head</code>, or a prompt job that
              asks the same question every Monday.
            </p>
          ) : (
            <div className="jobs">
              {jobs.map((job) => (
                <article className="job" key={job.id}>
                  <div className="job__head">
                    <b className="job__name">{job.name}</b>
                    <span className="tag">{job.kind === "prompt" ? "prompt" : job.image || "alpine"}</span>
                    {!job.enabled && <span className="tag tag--warn">paused</span>}
                  </div>
                  <p className="job__when">
                    <code>{job.cron}</code>
                    <span>{job.timezone || "server time"}</span>
                    <span>next {when(job.next_run_at)}</span>
                    {job.last_status && <span>last {job.last_status} · {when(job.last_run_at)}</span>}
                  </p>
                  <div className="job__actions">
                    <button className="btn btn--quiet btn--icon" type="button" title="Run now" onClick={() => run(job)}>
                      <RerunIcon />
                    </button>
                    <button className="btn btn--quiet" type="button" aria-pressed={job.enabled} onClick={() => toggle(job)}>
                      {job.enabled ? "Pause" : "Resume"}
                    </button>
                    <button className="btn btn--quiet" type="button" onClick={() => setDraft({ ...job })}>
                      Edit
                    </button>
                    <button className="btn btn--quiet" type="button" onClick={() => showLog(job)}>
                      {log?.job.id === job.id ? <ChevronUpIcon /> : <ChevronDownIcon />} Log
                    </button>
                    <button className="btn btn--quiet btn--icon" type="button" title="Delete" onClick={() => remove(job)}>
                      <CloseIcon />
                    </button>
                  </div>
                </article>
              ))}
            </div>
          )}

          {log && (
            <section className="runs">
              <h3>
                {log.job.name} — recent runs
                <button className="btn btn--quiet btn--icon" type="button" onClick={() => setLog(null)} aria-label="Close log">
                  <CloseIcon />
                </button>
              </h3>
              {!log.runs.length && <p className="meta">No runs yet.</p>}
              {log.runs.map((run) => (
                <div className="run" key={run.id}>
                  <p className="run__head">
                    <b>{run.status}</b>
                    <span className="meta">
                      {askedAt(new Date(run.started_at))}
                      {run.exit_code !== null && ` · exit ${run.exit_code}`}
                    </span>
                    {run.chat_id && (
                      <Link className="meta" href={`/?chat=${run.chat_id}`}>
                        open the answer
                      </Link>
                    )}
                  </p>
                  {run.detail && <pre className="run__out is-error">{run.detail}</pre>}
                  {run.output && <pre className="run__out">{run.output}</pre>}
                </div>
              ))}
            </section>
          )}
        </div>
      </div>
    </>
  );
}

function JobForm({
  draft,
  onChange,
  onSave,
  onCancel,
}: {
  draft: Job;
  onChange: (job: Job) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  const set = (patch: Partial<Job>) => onChange({ ...draft, ...patch });
  const isPrompt = draft.kind === "prompt";
  return (
    <section className="jobform">
      <div className="fields">
        <label className="field-label">
          <span>Name</span>
          <input type="text" value={draft.name} onChange={(e) => set({ name: e.target.value })} placeholder="Nightly backup check" />
        </label>
        <label className="field-label">
          <span>Kind</span>
          <select
            value={draft.kind}
            onChange={(e) => {
              const kind = e.target.value as Job["kind"];
              // a question through Libra needs minutes, not a minute: lifting a short wait here keeps the form honest about what gets saved
              set(kind === "prompt" && draft.max_duration_secs < 300 ? { kind, max_duration_secs: 300 } : { kind });
            }}
          >
            <option value="command">Command in a microVM</option>
            <option value="prompt">Question through Libra</option>
          </select>
        </label>
        <label className="field-label">
          <span>Schedule (cron)</span>
          <input type="text" className="is-mono" value={draft.cron} onChange={(e) => set({ cron: e.target.value })} placeholder="0 7 * * *" />
        </label>
        <label className="field-label">
          <span>Timezone</span>
          <input type="text" value={draft.timezone} onChange={(e) => set({ timezone: e.target.value })} placeholder="Europe/London" />
        </label>
      </div>

      <label className="field-label jobform__wide">
        <span>{isPrompt ? "Question" : "Command"}</span>
        <textarea
          className="field"
          rows={isPrompt ? 3 : 4}
          value={isPrompt ? draft.prompt : draft.command}
          onChange={(e) => set(isPrompt ? { prompt: e.target.value } : { command: e.target.value })}
          placeholder={isPrompt ? "What should Libra answer every time it runs?" : "apk add curl && curl -s https://example.com | head -20"}
        />
      </label>

      {!isPrompt && (
        <div className="fields">
          <label className="field-label">
            <span>Image</span>
            <input type="text" className="is-mono" value={draft.image} onChange={(e) => set({ image: e.target.value })} placeholder="alpine" />
          </label>
          <label className="field-label">
            <span>Memory</span>
            <input type="text" className="is-mono" value={draft.memory} onChange={(e) => set({ memory: e.target.value })} placeholder="256M" />
          </label>
        </div>
      )}

      <div className="fields">
        <label className="field-label">
          <span>Max run seconds</span>
          <input
            type="number"
            min={isPrompt ? 300 : 10}
            max={3600}
            value={draft.max_duration_secs}
            onChange={(e) => set({ max_duration_secs: Number(e.target.value) || 0 })}
          />
          <span className="field-help">
            {isPrompt ? ("A question gets at least 300 s — a search-backed answer usually takes two to five minutes. If the wait runs out the job is marked failed, but the thread keeps the answer, which finishes in the background.") : ("microsandbox stops the shell line after this long.")}
          </span>
        </label>
        <label className="field-label">
          <span>Notify webhook (optional)</span>
          <input type="url" value={draft.notify_url} onChange={(e) => set({ notify_url: e.target.value })} placeholder="https://ntfy.sh/your-topic" />
        </label>
      </div>
      <p className="meta jobform__note">
        A finished run also posts to the Telegram chat set in <a href="/settings/?tab=notifications">Settings → Notifications</a>.
      </p>

      <div className="savebar">
        <button className="btn btn--primary" type="button" onClick={onSave}>
          {draft.id ? "Save job" : "Create job"}
        </button>
        <button className="btn btn--quiet" type="button" onClick={onCancel}>
          Cancel
        </button>
        <span className="savebar__msg">
          {isPrompt
            ? "The answer is stored as a thread you can open from the log."
            : "Needs microsandbox (`msb`) on the Libra server — set MSB_BIN if it is not on PATH."}
        </span>
      </div>
    </section>
  );
}

/** "in 6h" / "3 days ago" for the next and last run stamps. */
function when(stamp: string) {
  if (!stamp) return "—";
  const d = new Date(stamp);
  if (Number.isNaN(d.getTime())) return "—";
  const minutes = Math.round((d.getTime() - Date.now()) / 60_000);
  const future = minutes >= 0;
  const ago = Math.abs(minutes);
  if (ago < 45) return future ? "in a moment" : "just now";
  const hours = Math.round(ago / 60);
  if (hours < 36) return `${future ? "in " : ""}${hours}h${future ? "" : " ago"}`;
  const days = Math.round(hours / 24);
  if (days < 8) return `${future ? "in " : ""}${days}d${future ? "" : " ago"}`;
  return shortDate(stamp);
}
