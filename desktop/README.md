# libra desktop

A Tauri 2 window whose only job is to open a libra server. The start screen is a
**Server URL** field; the URL you type takes over the window, and the page it
loads is the server's own UI. Nothing is started, no port is picked, no data
directory is created.

```
desktop/
  ui/                 start screen (bundled, loaded first)
  src-tauri/          the Rust shell
```

## Why a dumb window

`web/lib/api.ts` calls relative `/api/...` paths, so the page and the API must be
the same origin. A Tauri asset origin (`tauri://localhost` /
`http://tauri.localhost`) calling a `127.0.0.1:3001` API is cross-origin, and
`src/api.rs` `cors_layer` installs no allowed origins when `LIBRA_ALLOWED_ORIGINS`
is empty. Loading the server's own page keeps one origin, needs no CORS change
and no frontend change: the normal unlock flow (token in that origin's
`localStorage`, sent as `Authorization: Bearer`) works as it does in a browser.

Consequences, by design:

- Remote origins get no Tauri capabilities (`src-tauri/capabilities/default.json`
  names only the local window, never a `remote` URL), so the page that loads
  after navigation cannot call the shell's commands.
- Only `http` and `https` are accepted; a bare `host:port` gets `http://`.
- The last URL is kept in the app config dir (`%APPDATA%\dev.libra.desktop\last-url.txt`
  on Windows, `~/Library/Application Support/dev.libra.desktop/` on macOS) and is
  only **offered** on the next launch — the window always starts on the field.
- A second launch focuses the first window instead of opening another.

## Run and build

Needs Rust 1.85+ (the repo uses edition 2024 elsewhere; this crate uses 2021),
Node 20+, and on Windows the WebView2 runtime (present on Windows 11).

```bash
cd desktop
npm install
npm run dev            # debug build and launch
npm run build          # release build + installer for this OS
```

Artifacts: `src-tauri/target/release/bundle/` (`nsis/…-setup.exe` on Windows,
`.app` and `.dmg` on macOS). Build each installer on the machine that will run
it; a macOS bundle needs Xcode on that Mac.

## What has been run

Windows 11 (WebView2), against a real libra server and a probe page:

- start screen renders; typing `http://127.0.0.1:3100` loads that server's own UI;
- `fetch` streaming (`res.body.getReader()`, the path `web/lib/api.ts` `ask()`
  uses) receives chunks in WebView2;
- webview storage survives quit and relaunch (a counter in `localStorage` went
  1 → 2 across two launches, same profile);
- a second launch exits and focuses the first window; quitting leaves no process;
- Tauri IPC from the loaded server origin is refused (`not allowed`), and the app
  never starts a server or creates `libra.db`.

macOS (WKWebView) is not verified yet: same code, but the streaming and
plain-`http` paths need a run on the Mac mini before they can be claimed.

## Out of scope

Bundling or spawning the `libra` binary, free-port polling, the spawn env table,
and a bundled copy of `web/out`. Local-server mode, if wanted, is a separate
issue.
