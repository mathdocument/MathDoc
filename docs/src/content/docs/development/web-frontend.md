---
title: Web Frontend
description: Axum state, API contracts, Svelte architecture, and browser write serialization.
---

`mdc serve` combines an Axum HTTP server, a JSON API over `IndCache` and `DepGraph`, and
a Svelte 5 single-page application embedded in the Rust binary.

## Server architecture

- `src/web/mod.rs` defines `AppState`, holding `Arc<Mutex<IndCache>>` and a separate
  process-wide mutation mutex.
- `src/web/api.rs` implements handlers. No handler holds the synchronous cache lock
  across `.await`.
- `src/web/server.rs` builds the router, binds a loopback port, opens the browser, and
  handles graceful SIGINT/SIGTERM shutdown.
- `src/web/assets.rs` embeds `web/dist` with `rust-embed`, or uses `ServeDir` under the
  `dev-web` feature.
- `web/src/lib/state.svelte.ts` stores navigation state using Svelte runes.
- `web/src/components/BlockEditor.svelte` wraps one CodeMirror 6 editor per source
  block.

`node_view` performs one discovery and reference resolution while holding one cache
lock, then returns focused node detail, direct referrers, and direct dependencies. The
frontend uses this combined request for three-column navigation.

Title and block writes share one local resolve, snapshot, fnode recheck, mutation,
replacement, reindex, and detail-response transaction helper. Dependency writes remain
owned by `DepGraph`.

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
PUT    /api/node/:fnode/title                 { title }
PUT    /api/node/:fnode/block/:srctype        { content }
DELETE /api/node/:fnode/block/:srctype
POST   /api/node/:fnode/dep/add               { dep_fnode }
POST   /api/node/:fnode/dep/rm                { dep_fnodes: [] }
POST   /api/node/new                          { title, file?, parent_fnode? }
```

All `:fnode` parameters accept an exact fnode, unique prefix, or path-like reference via
`IndCache::resolve_ref`. Write handlers return canonical `NodeDetail` for the affected
node so the frontend does not need a second detail round trip.

## API contracts

- Search and candidate `n` values are capped at `200`; `/api/resolve` has no title
  fallback.
- `/api/graph/full` includes only non-broken nodes and edges whose endpoints remain in
  that set.
- The force graph is always mounted but defers full graph retrieval and D3 simulation
  creation until first activation.
- Base graph radius is
  `6 * (max(0, ln1p(in_degree) - ln1p(out_degree)) + 1)` before selection and hover
  emphasis.
- `dep/add.dep_fnode` and `node/new.parent_fnode` resolve as normal references despite
  their names. Neither accepts title fallback; self or existing additions return `422`.
- `dep/rm.dep_fnodes` contains literal case-sensitive fnodes. Exact direct matches are
  removed; nonmatches are ignored if at least one value matches, and no matches returns
  `422`.
- Creating a standalone node returns the new node. Creating and linking a child returns
  the parent.
- Block PUT preserves existing metadata but cannot set it. A new block starts with no
  metadata.

After Host validation, API errors use `{ "error": string }`. Common statuses are `400`
for malformed input, `404` not found, `405` unsupported method, `409` generation
conflict, `422` rejected mutation, and `500` infrastructure failure. The outer Host
check returns plain-text `421` before API normalization.

## Concurrency and security

`IndCache` owns one SQLite connection, so cache operations serialize on its mutex. The
separate mutation mutex covers each complete write. The workspace mutation lock
coordinates with other `mdc` processes.

Direct title and block writes hold both mutation boundaries across resolve, source
snapshot and parse, fnode recheck, mutation, conflict-aware replacement, index update,
and committed response construction.

The router accepts only loopback binding, validates Host as loopback or `localhost`, and
adds no CORS permission. These controls reduce cross-origin and DNS-rebinding exposure;
they are not authentication against local HTTP clients.

## Frontend build

Production Rust builds embed the committed `web/dist` at compile time. Frontend source
changes must include `npm run build` so hashed JavaScript, CSS, and `index.html` stay in
sync. The release workflow builds the frontend and fails if the committed distribution
changes.
