---
title: Browser frontend
---

The Svelte 5 interface mounts the project directory at `/` and the editor at
`/p/DATABASE/BRANCH/`. The directory polls `/api/status` while visible. A shared
path helper scopes branch API calls, Lean iframe URLs and WebSocket URLs;
static assets stay at the root. The editor provides search, graph/column navigation, dependency
operations and block editing. It tracks unsaved drafts and serializes node
mutations with revision guards. All four block types use consistent controls.
`web/src/design.css` supplies the shared colors, fonts and header geometry for
the project directory, editor and documentation site. The documentation theme
maps Starlight surfaces to these tokens; its header retains native search and
theme persistence.

Project-directory navigation retains the outgoing native view-transition snapshot
until the destination has loaded its data and initialized its editors (or has an
error to display). CodeMirror wrapping and gutters are measured before revealing
the page in one step. Knowledge/Graph switches use the same readiness rule,
including the first graph fetch. There is no fade for these switches, including
with reduced motion enabled. Ordinary links, browser history and unsaved-draft
guards are preserved; browsers without View Transitions use normal navigation.

Relation columns render a viewport window for lists over 100 nodes, with two
title lines reserved per row. Scrolling and Arrow/Home/End navigation can reach
the complete list. Switching views keeps the columns' scroll positions and the
same editor session. Node snapshots are replaced atomically instead of deeply
proxied. The graph component loads on first use and keeps its layout in memory;
the desktop layout uses fixed 5:3 grid columns for the graph and editor.
The canvas bitmap fits its own viewport, with an eight-million-pixel budget on
Retina displays. Window-size and pixel-density changes resize and paint in one
frame.
The dependency removal dialog filters by title or UUID and displays 50 results
per page; selections remain active across pages and filters until submitted.

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
