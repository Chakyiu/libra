"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { Health, ModelOptions, Settings, api, isUnauthorized } from "@/lib/api";
import { setPref, usePref } from "@/lib/prefs";

type Instance = {
  /** null until the first health check answers */
  online: boolean | null;
  /** the first load finished, so an empty model list really means "no provider added yet" */
  ready: boolean;
  /** online and ready, but nothing to answer with until the user adds a model provider */
  needsProvider: boolean;
  health: Health | null;
  settings: Settings | null;
  models: ModelOptions;
  refresh: () => Promise<void>;
  setSettings: (s: Settings) => void;
};

const InstanceContext = createContext<Instance | null>(null);
const NO_MODELS: ModelOptions = { chat: [], embedding: [] };

export function InstanceProvider({ children }: { children: React.ReactNode }) {
  const [online, setOnline] = useState<boolean | null>(null);
  const [ready, setReady] = useState(false);
  const [health, setHealth] = useState<Health | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [models, setModels] = useState<ModelOptions>(NO_MODELS);

  const refresh = useCallback(async () => {
    const [h, s, m] = await Promise.allSettled([
      api<Health>("/api/health"),
      api<Settings>("/api/settings"),
      api<ModelOptions>("/api/models"),
    ]);
    // A 401 means the server is up and locked, which is online as far as the banner goes.
    setOnline(h.status === "fulfilled" || (h.reason != null && isUnauthorized(h.reason)));
    if (h.status === "fulfilled") setHealth(h.value);
    if (s.status === "fulfilled") setSettings(s.value);
    if (m.status === "fulfilled") setModels(m.value);
    setReady(true);
  }, []);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(() => {
      api<Health>("/api/health")
        .then((h) => {
          setHealth(h);
          setOnline(true);
        })
        .catch((e) => setOnline(isUnauthorized(e)));
    }, 60_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const needsProvider = ready && online === true && models.chat.length === 0;
  const value = useMemo(
    () => ({ online, ready, needsProvider, health, settings, models, refresh, setSettings }),
    [online, ready, needsProvider, health, settings, models, refresh],
  );
  return <InstanceContext.Provider value={value}>{children}</InstanceContext.Provider>;
}

export function useInstance() {
  const ctx = useContext(InstanceContext);
  if (!ctx) throw new Error("useInstance outside InstanceProvider");
  return ctx;
}

/** The model for the next question: the composer's pick while the server still offers it, else the instance default. */
export function useAnswerModel(): [string, (model: string) => void] {
  const picked = usePref("model");
  const { models, settings } = useInstance();
  const fallback = settings?.defaultModel || models.chat[0] || "";
  const model = picked && models.chat.includes(picked) ? picked : fallback;
  const defaultModel = settings?.defaultModel;
  const choose = useCallback((m: string) => setPref("model", m === defaultModel ? "" : m), [defaultModel]);
  return [model, choose];
}
