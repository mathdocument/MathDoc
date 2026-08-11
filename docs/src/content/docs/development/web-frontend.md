---
title: Web Frontend
description: Axum state, API contracts, Svelte architecture, and browser write serialization.
---

`mdc serve` combines an Axum HTTP server, a JSON API over `IndCache` and `DepGraph`, and
a Svelte 5 single-page application embedded in the Rust binary.

## Server architecture

- `src/web/mod.rs` defines `AppState`, holding `Arc<Mutex<IndCache>>` and a separate
  `AppState`-scoped mutation mutex shared by handlers on the router.
- `src/web/api.rs` implements handlers. Synchronous cache and filesystem work runs on
  Tokio's blocking pool, and no handler holds the cache lock across `.await`.
- `src/web/server.rs` builds the router, binds a loopback port, opens the browser, and
  handles graceful SIGINT/SIGTERM shutdown.
- `src/web/assets.rs` embeds and serves `web/dist` with `rust-embed` in normal builds.
  Under `dev-web`, `src/web/server.rs` uses `ServeDir` over `$MDC_WEB_DIR/dist`, defaulting
  to `web/dist`, with an extensionless SPA fallback and `404` for missing file-like paths.
- `web/src/lib/state.svelte.ts` stores navigation state using Svelte runes.
- `web/src/components/BlockEditor.svelte` wraps one CodeMirror 6 editor per source
  block.

`node_view` performs one discovery and reference resolution while holding one cache
lock, then returns focused node detail, direct referrers, and direct dependencies. The
frontend uses this combined request for three-column navigation.

Title and block writes share one local resolve, snapshot, fnode recheck, mutation,
replacement, reindex, and detail-response transaction helper. Dependency writes remain
owned by `DepGraph`. JSON extraction and simple title/source-type validation happen
before the mutation mutex is acquired.

## API surface

```text
GET    /api/graph/roots
GET    /api/graph/check
GET    /api/graph/full
GET    /api/search?q=&n=
GET    /api/resolve?ref=
GET    /api/node/:fnode
GET    /api/node/:fnode/view
GET    /api/node/:fnode/children
GET    /api/node/:fnode/dep/candidates?q=&n=
PUT    /api/node/:fnode/title                 { title, expected_revision? }
PUT    /api/node/:fnode/block/:srctype        { content, expected_revision? }
DELETE /api/node/:fnode/block/:srctype        { expected_revision? }
POST   /api/node/:fnode/dep/add               { dep_fnode }
POST   /api/node/:fnode/dep/rm                { dep_fnodes: [] }
POST   /api/node/new                          { title, file?, parent_fnode? }
```

Read endpoints share a one-second workspace discovery gate so navigation and typing bursts
do not repeatedly walk every `.mdoc`. The first read and the first read after the gate
expires still discover external filesystem changes. Explicit refreshes use `fresh=true`,
successful web mutations update the shared index immediately, and discovery failures do
not advance the gate.

All `:fnode` parameters accept an exact fnode, unique prefix, or path-like reference via
`IndCache::resolve_ref`. Write handlers return canonical `NodeDetail` for the affected
node. The frontend consumes write responses directly and merges them into the focused
snapshot without replacing unrelated drafts. It refreshes only the dependency summary
list after relationship changes.

All successful endpoints currently return `200 OK`, including node creation and block
deletion. Creation does not return `201`, and deletion does not return `204`. Read
responses use these shapes:

| Endpoint | Response |
| --- | --- |
| `graph/roots` | `GraphRootItem[]` |
| `graph/check` | `{ nodes, edges, missing, invalid, cycles }` |
| `graph/full` | `{ nodes: NodeSummary[], edges: { source, target }[] }` |
| `search`, `node/:fnode/children` | `NodeSummary[]` |
| `resolve` | `{ fnode, title, rel_path }` |
| `node/:fnode` | `NodeDetail` |
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
- `/api/graph/full` includes only non-broken nodes and edges whose endpoints remain in
  that set.
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
- Title and block clients may send the `revision` returned by a read or prior write as
  `expected_revision`. A stale value returns `409` without changing the file. The
  bundled frontend always sends it and serializes writes per node so concurrent local
  edits carry each committed revision into the next request. Omitting it preserves the
  API's last-write-wins compatibility behavior. A bodyless block DELETE remains valid.

After Host validation, API errors use `{ "error": string }`. Common statuses are `400`
for malformed input, `404` not found, `405` unsupported method, `409` generation
conflict, `422` rejected mutation, and `500` infrastructure failure. The outer Host
check returns plain-text `421` before API normalization.

## Concurrency and security

`IndCache` owns one SQLite connection, so cache operations serialize on its mutex. The
separate `AppState` mutex covers each write's mutation critical section for handlers on
that router. The flock-based workspace mutation lock coordinates across application
states and other `mdc` processes.

One five-second deadline, created before dispatch to the blocking pool, bounds the
complete wait to enter a Web mutation: blocking-pool queue time, the local mutation
mutex, cache mutex, and workspace flock consume the same budget. Cache and flock
contention therefore do not block Tokio worker threads indefinitely. A poisoned local
mutex or a panicking blocking task becomes a structured `500` response.

Direct title and block writes hold both mutation boundaries across resolve, source
snapshot and parse, fnode recheck, mutation, conflict-aware replacement, index update,
and committed response construction.

Existing-node reads and replacements require regular `.mdoc` files inside the workspace
and reject symlink traversal. The router accepts only loopback binding, validates Host
as a numeric loopback, `localhost`, or a hostname ending in `.localhost`
case-insensitively, and adds no CORS permission. These controls reduce cross-origin and
DNS-rebinding exposure; they are not authentication against local HTTP clients.

## Frontend build

Production Rust builds embed the committed `web/dist` at compile time. Frontend source
changes must include `npm run build` so hashed JavaScript, CSS, and `index.html` stay in
sync. The release workflow builds the frontend and fails if the committed distribution
changes.
