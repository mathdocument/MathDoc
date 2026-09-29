---
title: Browser frontend
---

`web/` is a Svelte 5 application served by the API process from `MDC_WEB_DIR`. It
mounts the project directory at `/` and the editor at `/p/DATABASE/BRANCH/`.

## Structure

| Area | Files |
| --- | --- |
| Sign-in and token | `components/SignIn.svelte`, `lib/auth.ts` (token in `localStorage` under `mdc-access-token`, added to every request; a `401` signs out). |
| Directory | `Projects.svelte`, `components/ProjectCreate.svelte`; polls `/api/projects` while visible. |
| Editor | `App.svelte`, `components/EditorPane.svelte`, `NodeColumn.svelte`, `DepthGraph.svelte`, `BlockEditor.svelte`, overlays for search and dependencies. |
| Status | `components/FormalStatus.svelte`: local check and LeanGround certification. |
| Collaboration | `components/Collaboration.svelte` (four views), `LeanGroundSubmit.svelte`, `ProofEnvironmentForm.svelte`, `lib/coordination.ts` (API client; commands carry `If-Match` and a fresh `Idempotency-Key`). |
| LaTeX | `components/LatexPreview.svelte`, `LatexProjectForm.svelte`, `lib/latex-session.svelte.ts` (draft requests and cancellation), `lib/latex-completion.ts`, `lib/latex-math.ts` (MathJax 4). |
| Shared | `lib/project-path.ts` scopes branch API calls; `lib/unsaved.ts` tracks drafts; `lib/monaco.ts` and `lib/monaco-scroll.ts` set up editors. |

All block types, Lean included, use Monaco with native TextMate highlighting
(grammar data from `@shikijs/langs`, `lean4` for Lean), the system monospace
font and VS Code's light/dark themes, in one runtime per page. There is no Lean language client, Infoview or Lean
iframe. Editors and their undo history survive layout and preview changes.
`web/src/design.css` supplies colors, fonts and header geometry shared with this
documentation site.

Node mutations are serialized and carry the node revision; a `412` keeps the
draft for reconciliation. Relation columns render only a window of rows above 100
matches. The graph view loads on first use, keeps its layout in memory and uses
fixed 5:3 columns; its canvas is capped at eight million pixels on high-density
displays. Directory and view switches keep the outgoing view until the
destination is ready, using View Transitions where available.

Scrolling: block scrollers hand gestures to the node pane at their boundary, and
the node pane uses `overscroll-behavior-y: contain`. `web/build/monaco-wheel.ts`
makes Monaco's disabled wheel-zoom listener passive, in builds and dependency
prebundling, so Safari keeps native boundary feedback; the build fails if the
upstream listener changes shape.

## Development server

Run the backend (`serve`, and `worker` for proof work) as in
[Installation](../../getting-started/installation/), then:

```sh
npm run dev -w mdc-web
```

Open the Vite URL (normally `http://localhost:5173`) and sign in with a token.
Vite proxies `/api` and `/p/DATABASE/BRANCH/api` to `http://127.0.0.1:17843`,
preserving Host and Origin for the backend's same-origin check. `MDC_API_PROXY`
selects another backend URL; it is a Vite setting only.

## Build and validation

`npm run build -w mdc-web` writes `web/dist`, which the API serves and the Docker
image copies. The browser suite ([Development setup](../setup/#browser-tests))
covers the project directory, CLI/browser conflicts, navigation, graph
invariants, drafts, editor layout, LaTeX, Lean blocks as plain editors, and a Lean
node submitted, assembled and written back through the collaboration views (with
a LeanGround server). Frontend performance is in
[Performance measurements](../performance/#frontend-performance).
