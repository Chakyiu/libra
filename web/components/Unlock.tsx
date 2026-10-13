"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { getToken, isLocked, setToken } from "@/lib/api";
import { useInstance } from "@/components/instance";

/**
 * Shown when the server runs with LIBRA_AUTH_TOKEN and answers 401. The token lives in
 * this browser's localStorage and rides as a bearer header on every /api call.
 */
export default function Unlock() {
  const { refresh } = useInstance();
  const [open, setOpen] = useState(isLocked());
  const [value, setValue] = useState(getToken());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const on = () => setOpen(true);
    const off = () => setOpen(false);
    window.addEventListener("libra:locked", on);
    window.addEventListener("libra:unlocked", off);
    return () => {
      window.removeEventListener("libra:locked", on);
      window.removeEventListener("libra:unlocked", off);
    };
  }, []);

  const submit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (!value.trim()) return;
      setBusy(true);
      setError("");
      setToken(value);
      await refresh();
      setBusy(false);
      if (isLocked()) {
        setError("That token was refused.");
        return;
      }
      input.current?.blur();
    },
    [refresh, value],
  );

  if (!open) return null;
  return (
    <div className="unlock" role="dialog" aria-modal="true" aria-labelledby="unlock-title">
      <form className="unlock__box" onSubmit={submit}>
        <p className="kicker">Locked instance</p>
        <h2 id="unlock-title">Enter the access token</h2>
        <p>
          This Libra runs with <code>LIBRA_AUTH_TOKEN</code> set, so every API call needs the token. It is
          stored in this browser only.
        </p>
        <label className="field-label">
          <span>Access token</span>
          <input
            ref={input}
            type="password"
            autoFocus
            autoComplete="off"
            spellCheck={false}
            aria-invalid={error ? "true" : undefined}
            value={value}
            onChange={(e) => {
              setValue(e.target.value);
              setError("");
            }}
          />
        </label>
        {error && <p className="field-error">{error}</p>}
        <div className="unlock__foot">
          <button type="submit" className="btn btn--primary" disabled={busy || !value.trim()}>
            {busy ? "Checking…" : "Unlock"}
          </button>
        </div>
      </form>
    </div>
  );
}
