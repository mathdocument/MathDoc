---
title: Browser frontend
---

The Svelte 5 interface mounts the project directory at `/` and the editor at
`/p/DATABASE/BRANCH/`. The directory polls `/api/status` while visible. A shared
path helper scopes branch API calls, Lean iframe URLs and WebSocket URLs;
static assets stay at the root. The editor provides search, graph/column navigation, dependency
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
cache outside the source checkout. The default entry port matches Vite's proxy:

```sh
mdc init dev
mdc start dev/main
npm --prefix web ci
npm --prefix web run dev
```

Open the Vite URL (normally `http://localhost:5173`). Vite serves frontend assets
for the project list, or `/p/dev/main/` for the editor. Vite proxies `/api` and
`/p/DATABASE/BRANCH/api` HTTP/WebSocket requests to `http://127.0.0.1:17843`.
Its development middleware serves nested `lean.html` requests from the shared
Lean entry page. `MDC_API_PROXY` selects another entry URL if its port was
changed. It is a Vite development setting, not an mdc client project selector.
The proxy preserves the incoming Host/Origin pair for backend same-origin checks.
Stop the branch with `mdc stop dev/main`; `mdc stop` shuts down the server and all loaded branches.

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
