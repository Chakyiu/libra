#!/usr/bin/env python3
"""Mock OpenAI-compatible LLM for end-to-end tests (no API keys needed)."""
import hashlib
import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ANSWER = (
    "Rust is a systems programming language focused on safety [1]. "
    "Redis is an in-memory data store often used for caching [2]. "
    "Together they give a low-latency backend.\n\n## Summary\n\nFast, memory-safe stack."
)


def classify(system: str, user: str) -> str | None:
    if system.startswith("You are an expert at categorizing"):
        return json.dumps({
            "type": "normal",
            "need_web_search": True,
            "needs_image_results": False,
            "needs_video_results": False,
            "content_generation": None,
        })
    if system.startswith("You are a widget selection"):
        if "usd" in user.lower() or "eur" in user.lower():
            return json.dumps({"widgets": [{"name": "calculator", "input": "100 USD to EUR"}]})
        return json.dumps({"widgets": [{"name": "calculator", "input": "12 * 7"}]})
    if system.startswith("You are a search result relevance"):
        return json.dumps({"selected": [1, 2, 3]})
    if system.startswith("You select the images"):
        return json.dumps({"selected": [0, 1]})
    if system.startswith("You generate follow-up questions"):
        return json.dumps({"questions": ["How does Redis compare to Memcached?", "Why is Rust fast?"]})
    if system.startswith("You generate short, descriptive titles"):
        return json.dumps({"title": "Rust and Redis", "emoji": "🦀"})
    if system.startswith("You decide whether a follow-up"):
        return json.dumps({"needs_follow_up": True, "question": "rust redis performance"})
    if system.startswith("You plan the sections of a long-form"):
        return json.dumps({"sections": ["Opening", "How it works", "Tradeoffs", "What to watch"]})
    if system.startswith("You are the Automator's helper"):
        # A schedule the server cannot read, to exercise the fallbacks and the notes.
        if "unparseable" in user:
            return json.dumps({
                "name": "Odd one",
                "kind": "command",
                "cron": "sometime soon",
                "timezone": "Mars/Olympus_Mons",
                "command": "curl -s https://example.com | head",
                "why": "Runs a shell line soon-ish",
            })
        return json.dumps({
            "name": "Rust release notes",
            "kind": "prompt",
            "cron": "0 8 * * 1-5",
            "timezone": "Europe/London",
            "prompt": "What changed in the Rust release notes since the last check?",
            "image": "",
            "memory": "",
            "max_duration_secs": 300,
            "notify_url": "",
            "why": "Weekdays at 08:00 London time, asking Libra what changed since the last run",
        })
    return None


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")

        if self.path.endswith("/embeddings"):
            dim = 64
            out = []
            for i, inp in enumerate(body.get("input", [])):
                h = hashlib.sha256(str(inp).encode()).digest()
                vec = [(b / 127.5) - 1 for b in h[:dim]]
                out.append({"object": "embedding", "index": i, "embedding": vec})
            return self.json({"object": "list", "data": out, "model": body.get("model")})

        messages = body.get("messages", [])
        system = next((m["content"] for m in messages if m["role"] == "system"), "")
        user = next((m["content"] for m in reversed(messages) if m["role"] == "user"), "")
        fixed = classify(system, user)
        content = fixed if fixed is not None else ANSWER

        if not body.get("stream"):
            return self.json({
                "object": "chat.completion",
                "model": body.get("model"),
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            })

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        # MOCK_LLM_DELAY=4 makes the answer arrive slowly, to test what a timed-out
        # run does with an answer that is still on its way.
        delay = float(os.environ.get("MOCK_LLM_DELAY", 0))
        step = 24
        for i in range(0, len(content), step):
            chunk = {"choices": [{"index": 0, "delta": {"content": content[i:i + step]}}]}
            self.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n")
            self.wfile.flush()
            if delay:
                time.sleep(delay)
        self.wfile.write(b"data: [DONE]\n\n")

    def json(self, obj):
        raw = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 8899), Handler).serve_forever()
