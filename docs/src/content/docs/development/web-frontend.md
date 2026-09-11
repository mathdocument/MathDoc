---
title: Browser frontend
---

The Svelte 5 interface provides search, graph/column navigation, dependency
operations and block editing. It tracks unsaved drafts and serializes node
mutations with revision guards. All four block types use consistent controls.

Lean uses a lazy-loaded page embedding `lean4monaco` and upstream Infoview. Each
page has an isolated native WebSocket session, with source on the left and
Infoview on the right. LeanMonaco installs browser providers; its desktop
extension entry is disabled in the browser manifest. Save reuses the editor's
native result for the exact saved source and imports. CLI `lean check --build`
requests target artifacts when required.

## Development server

Create a disposable development database once, then start its branch. Keep the
cache outside the source checkout. A fixed port makes the default proxy work:

```sh
mdc init dev
mdc start dev/main --port 7599
npm --prefix web ci
npm --prefix web run dev
```

Open the Vite URL (normally `http://localhost:5173`). Vite serves frontend assets
and proxies HTTP and WebSocket `/api` requests to `http://127.0.0.1:7599`.
`MDC_API_PROXY` selects another backend URL if you use an automatically allocated
port. It is a Vite development setting, not an mdc client project selector.
The proxy preserves the incoming Host/Origin pair for backend same-origin checks.
Stop the backend with `mdc stop dev/main`.

## Release and validation

`npm --prefix web run build` writes `web/dist`; Cargo embeds those assets with
`rust-embed`. Commit rebuilt assets together with frontend changes. The release
binary needs no Node.js runtime.

The real-browser integration suite checks CLI/browser conflicts, navigation,
graph invariants, Lean goals and diagnostics, draft recovery, saved-editor
certification reuse and explicit `.olean` builds. Run it using
[Development setup](../setup/). The frontend performance fixture measures graph
and non-Lean interaction; native Lean behavior is covered by the real-server
suite.
