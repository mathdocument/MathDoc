---
title: Web Frontend
description: Axum state, API contracts, Svelte architecture, and browser write serialization.
---

`mdc serve` combines an Axum HTTP server, a JSON API over `WorkspaceStore` and `DepGraph`, and
a Svelte 5 single-page application embedded in the Rust binary.

## Server architecture

- `src/web/mod.rs` defines `AppState`, holding `Arc<DeadlineMutex<WorkspaceStore>>` and a separate
  `AppState`-scoped mutation mutex shared by handlers on the router.
- `src/web/api.rs` implements handlers. Synchronous cache and filesystem work runs on
  Tokio's blocking pool, and no handler holds the cache lock across `.await`.
- `src/web/server.rs` builds the router, binds a loopback port, prints the URL, and
  handles graceful SIGINT/SIGTERM shutdown. It does not launch a browser.
- `src/web/assets.rs` always embeds and serves `web/dist` with `rust-embed`, with an
  extensionless SPA fallback and `404` for missing file-like paths. Rust has no second
  development asset-serving path.
- `web/src/lib/state.svelte.ts` owns node state and per-instance navigation history.
  `browser-history.ts` adapts history writes to the browser and can be replaced in tests.
- `web/src/lib/workspace.svelte.ts` owns graph refresh ordering, local count updates,
  loading/error state, and cancellation of obsolete results.
- `web/src/components/BlockEditor.svelte` wraps one CodeMirror 6 editor per source
  block.

`node_view` performs discovery and reference resolution while holding one cache lock,
strongly upserts the focused path, then returns focused node detail, direct referrers,
and direct dependencies as `NodeView.children`. The frontend uses this combined request
for three-column navigation. Indexed fields in that response share one database read
snapshot, as do nodes and edges in `graph/full`. Focused source-file generation checks
remain separate from the SQLite snapshot.

Full graph reads copy nodes and edges from that snapshot, then release the cache
lock before filtering nodes and encoding edge indexes. Response assembly can run
alongside the next request without mixing database generations.

Title and block changes use application content operations and share one local resolve, snapshot, fnode recheck, mutation,
replacement, reindex, and detail-response transaction helper. Dependency writes remain
owned by `DepGraph`. JSON extraction and simple title/source-type validation happen
before the mutation mutex is acquired.

## API surface

```text
GET    /api/graph/roots
GET    /api/graph/check
GET    /api/graph/full
POST   /api/workspace/refresh
GET    /api/search?q=&n=
GET    /api/resolve?ref=
GET    /api/node/:fnode/view
GET    /api/node/:fnode/dep/candidates?q=&n=
PUT    /api/node/:fnode/title                 { title }          + If-Match
PUT    /api/node/:fnode/block/:srctype        { content }        + If-Match
DELETE /api/node/:fnode/block/:srctype                           + If-Match
POST   /api/node/:fnode/dep/add               { dep_fnode }      + If-Match
POST   /api/node/:fnode/dep/rm                { dep_fnodes: [] } + If-Match
POST   /api/node/new                          { title, file?, parent_fnode? }
```

Linked node creation also requires `If-Match`, matched against the parent; standalone
creation does not.

Graph roots, full graph, search, resolve, node view, and dependency-candidate reads run
workspace discovery, sharing a scan only with requests already queued when it started.
Later requests scan again; there is no TTL or query parameter for requesting a fresher read. Discovery remains metadata-based rather than a strong full
refresh; node view additionally performs a strong focused-path upsert.
`GET /api/graph/check` reports current indexed state without discovery or `refresh_all()`.
`POST /api/workspace/refresh` runs `WorkspaceStore::refresh_all()` and then returns the
graph-check report. Successful web mutations update the shared index immediately.

All `:fnode` parameters accept an exact fnode, unique prefix, or path-like reference via
`WorkspaceStore::resolve_ref`. Write handlers return canonical `NodeDetail` for the
affected node. The frontend accepts complete write responses without replacing editor-local
drafts. `NodeSession` owns the canonical `NodeView`; after relationship changes it
resynchronizes that view rather than maintaining a separate children result.

All successful endpoints currently return `200 OK`, including node creation and block
deletion. Creation does not return `201`, and deletion does not return `204`. Read
responses use these shapes:

| Endpoint | Response |
| --- | --- |
| `graph/roots` | `GraphRootItem[]` |
| `graph/check` | `{ nodes, edges, missing, invalid, cycles }` |
| `graph/full` | `{ nodes: NodeSummary[], edges: [sourceIndex, targetIndex][] }` |
| `search` | `NodeSummary[]` |
| `resolve` | `{ fnode, title, rel_path }` |
| `node/:fnode/view` | `{ node, referrers, children }` |
| `node/:fnode/dep/candidates` | `{ nodes, empty }` |
| Every write | `NodeDetail` |

`NodeSummary` contains `fnode`, `title`, `rel_path`, `broken`, and `depth`.
`NodeDetail` adds a SHA-256 `revision` for the exact `.mdoc` generation, ordered `depens`,
and source blocks containing `srctype`, `content`, and `metadata`. Root items use
`component_size` and `topo_depth`. An empty candidate result explains itself with
`no_match`, `excluded`, or `result_limit`.

## API contracts

- Search and candidate `n` values default to and are capped at `200`; `/api/resolve` has
  no title fallback. Matching is a case-insensitive literal substring over title and
  fnode, ranked by fnode prefix, title match position, title length, and path. The UI
  debounces for 120 ms and requests 50 results; broken global results are visible but
  cannot be selected.
- Candidate search excludes the source node, existing direct dependencies, and invalid
  or duplicate nodes. Its empty reason distinguishes no match, excluded matches, and an
  available match hidden by the result limit.
- `/api/graph/full` includes only non-broken nodes. Edge pairs index directly into that
  node array, and both endpoints are guaranteed to exist.
- The graph component is always mounted but defers full graph retrieval, static layout
  calculation, and canvas rendering until first activation. Layout is deterministic,
  grouped by descending topological depth, and ordered by title then fnode within a
  layer.
- Base graph radius is
  `min(24, 6 * (max(0, ln1p(in_degree) - ln1p(out_degree)) + 1))`. Selection adds three
  pixels; otherwise hover adds two.
- `dep/add.dep_fnode` and `node/new.parent_fnode` resolve as normal references despite
  their names. Neither accepts title fallback; self additions return `422`, while an
  existing dependency is an idempotent `200` response.
- `dep/rm.dep_fnodes` contains literal case-sensitive fnodes. Exact direct matches are
  removed; nonmatches are ignored if at least one value matches, and no matches returns
  `422`.
- Creating a standalone node returns the new node. Creating and linking a child returns
  the parent.
- `node/new.file` is trimmed and must be workspace-relative. Empty or `.` selects
  `<generated-fnode>.mdoc`, and `.mdoc` may be omitted. Absolute or escaping paths,
  `.mdc` and nested-workspace targets, symlinked components or targets, and existing
  targets are rejected with `422`.
- Block source types are case-insensitive but limited to `text`, `latex`, `python`,
  `lean`, and `rocq`, and are stored canonically in lowercase. Block PUT normalizes
  nonempty content to a trailing newline, preserves existing metadata, and gives a new
  block no metadata. Unknown types and deletion of a missing block return `422`.
- Titles are trimmed and must remain nonempty. Title or block mutations that would
  produce invalid MathDoc structure return `422`.
- `GET /api/node/:fnode/view` and every successful node mutation
  return a quoted SHA-256 `ETag` matching `NodeDetail.revision`. Existing-node mutations
  require that value in `If-Match`; linked creation checks the parent revision. A missing
  precondition returns `428`, a stale one returns `412` without changing the file, and a
  malformed header returns `400`. The bundled frontend serializes mutations per node and
  carries each returned revision into the next request. A bodyless block DELETE remains
  valid.

After Host validation, API errors use `{ "error": string }`. Common statuses are `400`
for malformed input, `404` not found, `405` unsupported method, `409` non-precondition
generation conflict, `412` stale precondition, `422` rejected mutation, `428` missing
precondition, and `500` infrastructure failure. The outer Host check returns plain-text
`421` before API normalization.

## Generated API types

Rust DTOs are the API type source of truth. `web/src/lib/api-types.generated.ts` is
generated and must not be edited directly. Check it with:

```bash
cargo test web::api::tests::api_types_are_current -- --exact
```

Regenerate it with:

```bash
UPDATE_API_TYPES=1 cargo test web::api::tests::api_types_are_current -- --exact
```

## Frontend state boundaries

`NodeSession` in `web/src/lib/state.svelte.ts` owns the canonical `NodeView`, shared
columns/graph selection, navigation history, and navigation/sync request generations.
The per-node mutation queue remains in `api.ts`; CodeMirror draft state remains
component-local, while dirty-draft and pending-mutation registries remain in `unsaved.ts`.
Graph data/layout/pan/zoom remain in `DepthGraph.svelte`, and overlay state remains in
`App.svelte`.

## Concurrency and security

`WorkspaceStore` owns one SQLite connection, so store operations serialize on its mutex. The
separate `AppState` mutex covers each write's mutation critical section for handlers on
that router. The flock-based workspace mutation lock coordinates across application
states and other `mdc` processes.

One five-second deadline, created before dispatch to the blocking pool, bounds the
complete wait to enter a Web mutation: blocking-pool queue time, the local mutation
mutex, cache mutex, and workspace flock consume the same budget. Cache and flock
contention therefore do not block Tokio worker threads indefinitely. A poisoned local
mutex or a panicking blocking task becomes a structured `500` response.

The local cache and mutation mutexes wait on a condition variable and notify on
unlock, preserving deadlines without adding a fixed polling delay to each queued
request. Notifications also propagate poison and expired waiters hand off their
wakeup, so other requests cannot be left sleeping while the lock is available.

Direct title and block writes hold both mutation boundaries across resolve, source
snapshot and parse, fnode recheck, mutation, conflict-aware replacement, index update,
and committed response construction.

Existing-node reads and replacements require regular `.mdoc` files inside the workspace
and reject symlink traversal. The router accepts only loopback binding, validates Host
as a numeric loopback, `localhost`, or a hostname ending in `.localhost`
case-insensitively, and adds no CORS permission. These controls reduce cross-origin and
DNS-rebinding exposure; they are not authentication against local HTTP clients.

## Client state and integration tests

`NodeSession` commits history on its own instance through an injected browser adapter;
creating another session does not update the exported default session. `WorkspaceSession`
rejects obsolete refresh results and marks graph diagnostics stale after local mutations.
The API client throws `ApiError` with the HTTP status and parsed response body;
`isConflict` identifies `409` and `412` while existing message rendering remains unchanged.

`npm run test:e2e` launches Chromium against a real `mdc serve` process and fresh temporary
workspaces. It covers stale saves after external edits, navigation while a real save is
pending, browser back/forward, and simultaneous CLI/browser dependency mutations. Backend
responses are not mocked; one test delays delivery to make the pending-save race deterministic.
Each fixture runs `graph check` and cleans up its browser context, server, and workspace.

## Frontend build

Production Rust builds embed the committed `web/dist` at compile time. Frontend source
changes must include `npm run build` so hashed JavaScript, CSS, and `index.html` stay in
sync. The release workflow builds the frontend and fails if the committed distribution
changes.

For live development, Vite owns frontend serving and HMR. It proxies `/api` to an
ordinary `mdc serve` process; the Rust server still uses the same embedded-asset path as
every other build.
