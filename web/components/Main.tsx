"use client";

import { useSearchParams } from "next/navigation";
import Home from "@/components/Home";
import Thread from "@/components/Thread";

/** `/` is home; `/?chat=<id>` is a thread (static export, so no dynamic segments). */
export default function Main() {
  const params = useSearchParams();
  const chat = params.get("chat");
  return chat ? <Thread key={chat} chatId={chat} seed={params.get("q")} /> : <Home />;
}
