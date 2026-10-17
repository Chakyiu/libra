"use client";

import { useLayoutEffect } from "react";
import { applyPrefs } from "@/lib/prefs";

/** Registers the app shell and keeps the status bar on the chosen theme. */
export default function Pwa() {
  useLayoutEffect(() => {
    applyPrefs();
    if (process.env.NODE_ENV !== "production") return;
    if (!("serviceWorker" in navigator)) return;
    navigator.serviceWorker.register("/sw.js").catch(() => {
      /* the site still works in a browser that refuses the worker */
    });
  }, []);
  return null;
}
