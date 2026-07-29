# Development Guide

`README.md` is the user guide. This file is the developer guide: architecture,
maintenance commands, cache semantics, and editor-extension maintenance.

After completing a development task, suggest a conventional-commit message and
say whether the Cargo package version should be bumped.

## Commands

Development currently requires Unix. The release check uses stable Rust and
Node 24.

```bash
cargo build                     # debug build
cargo build --release           # release build; binary: target/release/mdc
cargo test                      # all tests, including integration tests
cargo test <name>               # run tests matching a name substring
cargo test --test test_indcache # run one integration test target
cargo fmt                       # format Rust code
cargo clippy                    # lint

# Web frontend (mdc serve)
cd web && npm ci                # reproducible dependency install
cd web && npm run build         # write web/dist/ (embedded by cargo build --release)
cd web && npm run check         # svelte-check type/syntax pass
cargo run --features dev-web -- serve --bind 127.0.0.1:7599 --no-open
cd web && npm run dev                  # separate terminal; Vite requests :5173
```

Integration tests live in `tests/`. Unit tests are inline in source files.
Release builds embed the committed `web/dist/` output. Keep it in sync with web
source changes by running `npm run build`; the build intentionally bundles only
the five supported source languages and one theme.

The release workflow runs `npm ci`, `npm run check`, `npm run build`, verifies
that `web/dist` is unchanged, and then runs `cargo +stable test --locked`.

## Product Model

`mdc` recognizes a workspace by a real, non-symlink `.mdc/` control directory.
`mdc init` creates that directory and its configuration template; discovery does
not verify how it was created and skips nested workspaces and symlinked entries.

A minimal `.mdoc` file looks like this:

```text
@fnode: <uuid>
@title: <title>

@dep:
<dependency-fnode>
@end

@src: latex
content here
@end
```

Important format details:

- `@fnode` is the stable node ID. `mdc new` generates UUID v4 values, while the
  parser accepts lowercase ASCII letters and digits with internal hyphens. The
  first eight Unicode scalar values are used only for short display.
- `@title` is searchable display text.
- `@dep:` contains direct dependency fnodes, one per line.
- `@src: <srctype>` contains one source block. Current built-in srctypes are
  `text`, `latex`, `python`, `lean`, and `rocq`.
- A file may contain at most one block per srctype.
- `@src` headers may contain optional `key=value` metadata tokens. They are
  parsed and preserved by `MdocNode`, but are not currently used by compilers.
- References accepted by commands are path-like refs, exact fnodes, or unique
  fnode prefixes.

## Module Map

| Module | Purpose |
| --- | --- |
| `src/mdocnode/` | `.mdoc` parser, serializer, and `MdocNode` model |
| `src/indcache/` | SQLite-backed workspace index (`IndCache`) |
| `src/depgraph/` | Dependency mutation session for one root node (`DepGraph`) |
| `src/metric/` | Read-only metric framework: registry/context in `mod.rs`, with all concrete formulas in `function.rs` |
| `src/compiler/` | Compiler registry/language implementations plus shared subprocess control in `process.rs` |
| `src/cli/` | `clap` command definitions, command handlers, and terminal output |
| `src/web/` | `mdc serve` HTTP server (axum): JSON API over `IndCache`/`DepGraph` + SPA asset serving |
| `src/core/` | Shared models and database-independent graph algorithms (topo depth, weak components, SCCs, and cycles) |
| `src/config.rs` | Canonical built-in source descriptors, `.mdc/config.toml`, its initialization template, and compiler defaults |
| `src/workspace/` | Workspace initialization/discovery, path validation, mutation locking, and generation-checked descriptor-relative file updates |
| `src/workdraft/` | Source mirror manifest, safe layout, shared reconciliation classification, snapshot-checked batch application, and reverse-order rollback attempts after detected failures; rollback is best-effort and batches are not crash-atomic |
| `editors/vscode/` | VS Code language extension for `.mdoc` files |
| `web/` | Svelte 5 + Vite + TypeScript frontend; built output is embedded into the `mdc` binary via `rust-embed` |

## `.mdoc` Parsing

`MdocNode::load` fully parses a file, including block contents. Index refreshes
open each regular file descriptor-relatively with no symlink following, read it
once, and pass those bytes to `MdocHead::load_bytes`, which walks and structurally
validates the complete document, including source headers and block termination,
but does not allocate or retain source-block bodies.
`MdocNode::upsert_source_block()` normalizes nonempty mutation content to the same
trailing-newline representation produced by the parser.

Invalid files are represented in `mdoc_issues` where possible. `MdocIdentity`
is intentionally lenient and can recover `(fnode, title)` from a broken file so
the cache can report useful invalid/duplicate diagnostics.

## IndCache

The SQLite database lives at `.mdc/index.db`. Current schema version is `16`.
The DB is opened with WAL mode and foreign keys enabled.

Internal module boundaries are strict:

- `schema.rs`: table DDL and schema versioning (`PRAGMA user_version`). Older
  managed cache schemas are discarded and rebuilt from `.mdoc`; newer schemas
  are rejected without mutation.
- `queries.rs`: pure database reads. It contains no index materialization or
  invalidation writes.
- `derived.rs`: materialization and invalidation for topo depth, in-degree, weak
  components, graph epochs, and the SCC/cycle cache.
- `refresh.rs`: file-backed row, edge, and issue upsert/delete operations.
  Functions take `&Connection` and do not own transactions.
- `discovery.rs`: workspace enumeration and `(mtime_ns, size)` change discovery.

`IndCache` owns the SQLite connection and all transaction boundaries. Multi-step
mutations should be wrapped as `conn.transaction()` followed by `tx.commit()` in
`src/indcache/mod.rs`.

Reference resolution and pure query paths do not prune cached rows when path
validation fails; they ignore those candidates. Discovery, full/targeted refresh,
explicit single-path upserts, and `reconcile_fnode_paths()` own stale-row deletion
and derived-state updates. Read-facing facades such as `global_root_items()` and
`graph_check_report()` may transactionally materialize derived caches.

Indexed node creation is also cache-owned: `IndCache::create_node()` validates the
node against the cache root and workspace mutation lock, renders it, creates any
parents, revalidates the final path, and atomically writes against
`FileSnapshot::Missing`. On index failure, the cache attempts file rollback and
path-index repair; either recovery step can fail and is preserved in the returned
error. `MdocNode::render()` only validates and serializes bytes; production
filesystem creation remains owned by `IndCache`.

Safe-file writes bind the validated parent-directory inode, create temporary
files descriptor-relatively, and use same-directory no-clobber quarantine
renames before replacing or rolling back a generation. Ancestor identities and
the quarantined generation are checked again around persistence. If generation
or directory identity becomes uncertain, MathDoc returns `FileConflict` and
restores the quarantined name when that is safe; otherwise it leaves the
quarantine file in place rather than overwriting or deleting another writer's
bytes.

Replacing an existing file quarantines the old generation before installing the
new one, so the target pathname may be briefly absent. If a later batch step
fails, completed operations are rolled back in reverse order where possible;
rollback failures are preserved in the returned error.

Unix cannot provide a portable atomic content compare-and-unlink operation. In
particular, on macOS a non-cooperating process can continue writing through a
descriptor opened before quarantine. MathDoc verifies the quarantined inode
immediately before unlinking and preserves/restores it when such a write is
observed. A write that lands in the final interval between that verification and
`unlinkat` cannot be detected atomically; the open descriptor still retains the
inode until it is closed. Workspace mutation locking prevents this race between
cooperating MathDoc processes but cannot constrain arbitrary external editors.

Replacement snapshots include content, inode identity, permissions, ownership,
and supported extended attributes. Writes reject symlink/nonregular targets and,
where supported, read-only, hard-linked, ACL-bearing, or unsupported-flag files.
The SQLite database is opened no-follow and multiply linked databases are
rejected. Multi-file rollback is best-effort rather than guaranteed or
crash-atomic.

Workspace-wide source reconciliation uses an operation-scoped
`FileSnapshotBatch`. It keeps a bounded cache of no-follow parent-directory
descriptors, records every traversed directory identity, and verifies those
generations before reconciliation can apply writes. Mdocs are processed in
2,048-source batches; mdoc and mirror reads are split across at most eight scoped
workers, with deterministic result ordering. Read-only mirror snapshots omit replacement
metadata, while a mirror selected for writing is recaptured as a full
`FileSnapshot` and its observed content is checked again. When reconciliation
produces no writes, removals, renames, or manifest change, one lightweight batch
read confirms that all mdoc inputs still match before returning.

`CHUNK_SIZE = 500` is used for SQL `IN (...)` chunks. Full refresh replacement
uses 200-row multi-value inserts to stay under SQLite variable limits even for
the four-column edge and issue tables.

### Database Tables

| Table | Key columns | Purpose |
| --- | --- | --- |
| `mdoc_files` | `path`, `mtime_ns`, `size` | File-state cache for change detection and stale-path cleanup |
| `mdocs` | `path`, `fnode`, `title`, `title_lc`, `topo_depth` | Searchable node cache, reference resolution, persisted topo depth |
| `mdoc_edges` | `src_path`, `src_fnode`, `dst_fnode`, `ord` | Dependency edges in source order |
| `mdoc_issues` | `path`, `kind`, `ref_fnode`, `error` | Structural problems: `invalid`, `duplicate`, `missing` |
| `mdoc_in_degree` | `fnode`, `in_degree` | Precomputed in-degree for root detection |
| `mdoc_weak_component` | `fnode`, `component_size` | Weak connected-component size for each node |
| `mdoc_index_state` | `graph_epoch`, `weak_component_dirty`, `bootstrapped`, `index_digest` | Epoch, weak-component dirty state, bootstrap state, and full-refresh semantic digest |
| `mdoc_scc_result` | `graph_epoch`, `cycles_json` | One representative cycle per cyclic SCC for an epoch; it does not persist the complete SCC decomposition |

`mdoc_valid_edges` is the schema-owned view used by graph reads and derived-data
refreshes. It contains `mdoc_edges` rows whose source path has no `invalid` or
`duplicate` issue. A `missing` target issue does not suppress the source edge.
The source and destination indexes on `mdoc_edges`, plus the path-leading primary
key on `mdoc_issues`, support the view predicates.

`IndCache::search(query, limit)` is the canonical ranked search and returns
`NodeSummary` values. Dependency-candidate search reuses its match patterns,
ranking, and summary projection before applying dependency-specific filters.
`all_node_summaries()` is the unbounded non-search query used by the full graph API.

## Cache Refresh Model

There are two broad refresh paths.

`discover_workspace_changes()` is the fast path used by most commands. It enumerates
workspace `.mdoc` files and compares cached `(mtime_ns, size)` metadata. Add/update
operations trigger incremental topo-depth refreshes. Any deletion triggers full
topo-depth backfill because ancestor depths may decrease and there may be no single
safe starting node. Same-metadata edits are intentionally deferred to strong refresh
paths such as `mdc sync` and `mdc graph check`.

`refresh_all()` is the strong full-rescan path used by `mdc sync`, `mdc graph
check`, and metrics. It descriptor-relatively reads and reparses every discovered
`.mdoc`, then updates changed `(mtime_ns, size)` file states. A deterministic
`index_digest` covers sorted paths, recovered identity, title, dependency order,
and parse status, but not source-block bodies. If the digest matches, refresh
keeps the existing node, edge, issue, in-degree, epoch, and derived rows. If it
differs, refresh globally constructs invalid, duplicate, and missing issues in
memory, replaces base rows with multi-value inserts, rebuilds in-degree in one
grouped query, and eagerly rebuilds topological depths and weak components. Graph
changes invalidate the epoch-keyed SCC/cycle cache, which
`graph_check_report()` rebuilds lazily. Incremental path upserts and deletions
clear `index_digest` so the next strong refresh must reconcile the complete graph.

Those descriptions are steady-state behavior. The first `IndCache::open()` after
database creation or schema rebuild performs a full bootstrap. Node creation and
dependency writes also run `refresh_all()` under the workspace mutation lock
before their final duplicate, snapshot, and cycle checks.

### Derived Data

`mdocs.topo_depth` and `mdoc_weak_component` are derived summaries of the graph.

`topo_depth` is persisted in `mdocs`. In regions not blocked by a reachable cycle,
leaves have depth `0` and every parent has `1 + max(depth(dependency))`. Nodes in
cycles, and noncyclic nodes that transitively depend on them, may retain partial
Kahn accumulation. Cycle reporting, not `topo_depth`, is authoritative there.
Node summaries and root lists read the column directly, which keeps display
queries cheap.

Bulk paths call `derived::refresh_all_derived_data()`, which loads the graph once,
runs the core Kahn-style topo-depth and weak-component algorithms, and persists
both results.

Incremental paths call `refresh_topo_depth_upward_from(fnode)`. It recomputes the
changed node and all reverse-reachable ancestors in dependency-first order. If
the affected subgraph contains a cycle and therefore cannot be ordered, it falls
back to a full topo-depth backfill.

When one node's `@dep` list changes and its `@fnode` stays the same, the update is
therefore targeted, not full-graph: rewrite that node's edges, recompute its
`topo_depth`, then propagate upward through its referrers.

`weak_component_dirty` belongs to weak connected components, not topo depth. A
graph change calls `derived::bump_graph_epoch()`, which increments `graph_epoch`
and sets `weak_component_dirty = 1`. The `IndCache::global_root_items()` facade
opens a transaction and asks `derived` to ensure weak components before the pure
root query reads `topo_depth` and `component_size` with a JOIN.

`mdoc_scc_result` is invalidated by `graph_epoch`. The
`IndCache::graph_check_report()` facade asks `derived` to read or refresh the SCC
cache in its transaction, then passes the cycles to the pure report query.

### Per-Command Cache Behavior

| Command | Discovery | Content refresh | Notes |
| --- | --- | --- | --- |
| `mdc init` | none | none | `workspace::initialize()` creates `.mdc/` and the template from `config`; does not touch the index |
| `mdc new` | none | `refresh_all()`, then cache-owned create/upsert | Full refresh and duplicate check happen under the mutation lock before creation |
| `mdc edit` | `discover_workspace_changes()` | `upsert_path()` after editor exits | Opens `$EDITOR` |
| `mdc sync` | none | `refresh_all()` | Full rescan, then mirrors all five source types under `.mdc/<srctype>/Lib/<relative-path>.<ext>` |
| `mdc search` | `discover_workspace_changes()` | none | Enumerates and stats discovered `.mdoc` files; reparses only new or metadata-changed paths |
| `mdc dep add` | `discover_workspace_changes()`, then source `upsert_path()` in `DepGraph::from_ref` | `refresh_all()` under the mutation lock before the final write | `--target` uniquely resolves one target before cycle checking |
| `mdc dep rm` | `discover_workspace_changes()`, then source `upsert_path()` in `DepGraph::from_ref` | `refresh_all()` under the mutation lock before the final write | `--target` also resolves prefixes of dangling direct dependencies |
| `mdc dep show` | `discover_workspace_changes()` | `refresh_reachable_from_path()` on source | Targeted reachable refresh; exits `1` if cycles are reported |
| `mdc dep leaf` | `discover_workspace_changes()` | `refresh_reachable_from_path()` on source | Targeted reachable refresh; exits `1` if cycles are reported |
| `mdc dep refs` | `discover_workspace_changes()` | `upsert_path()` on target | Target refreshed before reverse-edge query |
| `mdc graph check` | none | `refresh_all()` | Reports missing targets, invalid issues (including duplicate fnodes), and representative cycles |
| `mdc graph roots` | `discover_workspace_changes()` | none | Reads persisted `topo_depth`; graph load only if weak components are dirty |
| `mdc graph tui` | `discover_workspace_changes()` | DepGraph mutation APIs plus post-op discovery | TUI add/rm/create delegate to DepGraph; start refs also accept a unique exact title |
| `mdc metric ior` | none | `refresh_all()` | Resolves one valid node and prints `ln1p(in_degree) - ln1p(out_degree)` using valid-source edges |
| `mdc work` | `discover_workspace_changes()` | workspace-wide `workdraft::sync()` before selected-node compilation | Any sync conflict aborts compilation; dirty mirrors are compiled without being overwritten; no targets is a successful no-op |
| `mdc back` | `discover_workspace_changes()` before write | `refresh_all()` when mdocs changed | Imports dirty mirrors and refreshes the index |
| `mdc serve` | `discover_workspace_changes()` on read handlers | guarded cache replacement after direct node edits | A process-wide mutation mutex serializes each complete resolve/load/mutate/save/reindex operation; column navigation gets detail, referrers, and children from one cache lock; dep writes route through `DepGraph`; start refs also accept a unique exact title. |

### File Change Detection

CLI-managed writes should call `upsert_path()` directly, or route through a
DepGraph mutation that calls it. `mdc new`, `mdc edit`, `mdc dep add`, and
`mdc dep rm` follow this rule.

External adds, deletes, and renames are detected by `discover_workspace_changes()`
through workspace enumeration and file metadata.

External content edits to already-known files are not guaranteed to change a
directory mtime. Commands that need fresh dependency content use
`refresh_reachable_from_path()`, and `mdc sync` uses `refresh_all()`. Commands such
as `mdc search` and `mdc graph roots` avoid reparsing unchanged known files, but
discovery still enumerates and stats every discovered `.mdoc`.

After refreshing the index, `mdc sync` exports every structurally parseable
workspace `.mdoc` into exactly five source mirrors under
`.mdc/<srctype>/Lib/`, preserving relative paths. Duplicate-fnode files are still
exported because duplication is an index issue; structurally unparseable files
retain their previous mirrors. Missing blocks become empty files.
`.mdc/source-blocks.json` tracks which source paths were generated so deleted or
renamed sources can be cleaned without touching unrelated files. It also records
the content digest and block-presence baseline for every source/srctype pair.
`mdc sync` updates a mirror only when it still matches that baseline; `mdc back`
performs the inverse check before updating mdoc. Independent changes on both
sides are preserved and reported. Structurally unparseable documents retain
their previous files.

File state is stored as `mtime_ns` and `size`, where `mtime_ns` is
`secs * 1_000_000_000 + subsec_nanos`.

## DepGraph

`DepGraph` is the primary API for mutating one document's dependency list. It owns
one root `MdocNode` and borrows an `IndCache`; traversal and reachability stay in the
SQLite-backed index rather than a second in-memory graph.

Important constructors and operations:

- `DepGraph::from_ref(cache, ref_str, cwd)` resolves a path/fnode/prefix, indexes
  the resolved file before duplicate checks, and loads the root node.
- `DepGraph::create_root(...)` creates a new `.mdoc`, indexes it, and returns a
  graph rooted at the new node.
- `add_direct_dependencies()` and `create_and_add_dependency()` reject cycles by
  checking whether the candidate dependency can already reach the root in the
  indexed graph.
- `remove_direct_dependencies()` saves the root node and reindexes it.

Direct file edits can still create cycles. `mdc graph check` is the authoritative
reporting path for cycles introduced outside the mutation API.

## Metrics

Metrics are a read-only layer over `IndCache`, not part of `DepGraph`.
`DepGraph` owns a mutable root-node dependency session, while metrics consume
indexed graph primitives without loading or mutating an `MdocNode`.

`src/metric/mod.rs` defines `MetricContext`, the `NodeMetric` interface, and the
compile-time `NodeMetricKind` registry. `src/metric/function.rs` intentionally
keeps every concrete metric formula together instead of creating one module per
metric. Adding another node metric consists of adding its function there, wiring
its implementation and enum mapping in `mod.rs`, and adding its typed Clap
variant and dispatch arm in `src/cli/`.

`IndCache::node_degrees()` is the shared data boundary. It reads precomputed
in-degree and indexed valid-source out-degree for one valid, uniquely indexed
node. IOR is evaluated as `ln1p(i) - ln1p(o)`, which is equivalent to
`ln((i + 1) / (o + 1))` without constructing the intermediate ratio. The CLI
uses `refresh_all()` before evaluation and emits only the finite `f64` value.

## Profiling

The Clap-level global `--prof` flag enables the low-overhead scopes in
`src/profile.rs`. At process exit, the CLI writes an inclusive elapsed-time tree
to stderr, grouping separate worker-thread trees when needed. Normal stdout
remains unchanged, including numeric-only metric output.
The current scopes separate CLI dispatch, cache open/bootstrap, workspace scan,
digest comparison, issue construction, bulk row replacement, in-degree rebuild,
derived graph algorithms, and SQLite commits. Scopes are intentionally inclusive,
so a parent's duration contains all reported child durations; do not sum an
entire report as though entries were disjoint.

## Web Frontend (`mdc serve`)

`mdc serve` runs an axum HTTP server that serves a JSON API over
`IndCache`/`DepGraph` and a Svelte 5 SPA. The SPA is a browser-based alternative
to `mdc graph tui`: it provides a three-column browser/editor and a force-directed
full-graph view. Cards use clicking or normal browser button focus; overlays own
their documented keyboard controls. Navigation uses the View Transitions API
with a synchronous fallback.

### Architecture

- `src/web/mod.rs` defines `AppState`, a `Clone` struct holding an
  `Arc<Mutex<IndCache>>` and a separate process-wide mutation mutex. Every
  handler locks the cache for the duration of its synchronous work; no handler
  holds the lock across `.await`.
- `src/web/api.rs` contains the handlers. `node_view` performs one discovery and
  reference resolution while holding one cache lock, then returns the focused
  `NodeDetail`, direct referrers, and direct dependencies together. The separate
  detail and children reads remain for editor/force-graph refresh and dependency
  removal. `node_put_block`, `node_delete_block`, and `node_put_title` share one
  local resolve/snapshot/identity-check/mutate/replace/detail transaction helper;
  dependency writes continue through `DepGraph`.
- `src/web/server.rs` builds the router, binds a free port (or a
  caller-supplied `--bind` address), opens the browser, and handles
  graceful shutdown on SIGINT/SIGTERM.
- `src/web/assets.rs` embeds `web/dist` via `rust-embed`. With the
  `dev-web` cargo feature, `tower-http::ServeDir` serves
  `$MDC_WEB_DIR/dist` (default `web/dist`). Vite requests port 5173 for HMR and
  falls back to another port if occupied; it proxies `/api` to `MDC_API_PROXY`
  (default `http://127.0.0.1:7599`).
- `web/` is a standalone Vite + Svelte 5 + TypeScript project.
  `web/src/lib/state.svelte.ts` holds navigation state in runes;
  column navigation uses one `nodeView` API request;
  `web/src/components/BlockEditor.svelte` wraps a CodeMirror 6 editor
  per source block.

### API Surface

```
GET  /api/graph/roots
GET  /api/graph/check
GET  /api/graph/full
GET  /api/search?q=&n=
GET  /api/resolve?ref=
GET  /api/node/:fnode
GET  /api/node/:fnode/view
GET  /api/node/:fnode/children
GET  /api/node/:fnode/dep/candidates?q=&n=
PUT  /api/node/:fnode/title                 { title }
PUT  /api/node/:fnode/block/:srctype        { content }   # create-or-replace
DELETE /api/node/:fnode/block/:srctype
POST /api/node/:fnode/dep/add               { dep_fnode }
POST /api/node/:fnode/dep/rm                { dep_fnodes: [] }
POST /api/node/new                          { title, file?, parent_fnode? }
```

All `:fnode` path params accept an exact fnode, a unique fnode prefix,
or a path-like ref (resolved via `IndCache::resolve_ref`). Write
handlers return the canonical `NodeDetail` of the affected node so the
SPA can refresh without a second round-trip.

Additional contracts:

- Search and candidate `n` values are capped at 200. `/api/resolve` does not use
  the title fallback.
- `/api/graph/full` includes only non-broken nodes and edges whose endpoints
  remain in that set.
- The always-mounted force-graph component defers `/api/graph/full` and D3
  simulation construction until the force view is first activated. Its base node
  radius is `6 * (max(0, ln1p(in_degree) - ln1p(out_degree)) + 1)`, using the
  directed edges rendered by that endpoint; selection and hover add their visual
  emphasis afterward.
- `dep/add.dep_fnode` and `node/new.parent_fnode` are resolved as normal
  path/fnode/prefix references despite their names; neither uses title fallback.
  Self or existing dependency additions return `422`.
- `dep/rm.dep_fnodes` contains literal, case-sensitive fnode values. Exact direct
  dependencies are removed; nonmatches are ignored if at least one value matches,
  and `422` is returned when none match.
- `node/new` returns the new node when creating alone, but returns the parent
  after creating and linking a child with `parent_fnode`.
- Block PUT preserves existing metadata but cannot set it; a new block starts
  with empty metadata.
- Requests that pass Host validation normalize `/api` errors to
  `{ "error": string }`. Common statuses are `400` malformed input, `404` not
  found, `405` unsupported method, `422` rejected mutation, `409` generation
  conflict, and `500` internal/infrastructure failure. The outer Host check is an
  exception and returns plain-text `421` before API normalization.

### Concurrency Model

`IndCache` owns a single SQLite connection. The web server wraps it in
`Arc<Mutex<IndCache>>`, so cache operations serialize on that mutex. Dependency
handlers construct `DepGraph` by borrowing the same cache. The separate mutation
mutex serializes each complete write operation, while the workspace mutation lock
coordinates writes with other mdc processes. The direct title/block helper holds
both boundaries across resolve, source snapshot/parse, fnode recheck, mutation,
conflict-aware replacement, index update, and committed response construction.
The router also validates `Host` as loopback/localhost and installs no CORS
permission. These controls reduce browser cross-origin and DNS-rebinding exposure
but are not authentication against local HTTP clients.

### Frontend Build

Release builds embed the committed production output in `web/dist` at compile
time. Frontend source changes must include a fresh `cd web && npm run build` so
the hashed JS/CSS assets and `index.html` remain synchronized.

For development, use the `dev-web` cargo feature and run Vite in a
second terminal — see the Commands section above.

## Work/Back and Compilers

`mdc work <source>` first performs workspace-wide reconciliation into the stable
`Lib/` trees. Sync may commit unrelated clean mirror and manifest changes even
when another pair conflicts. Any remaining conflict makes `work` skip all
compilers and exit `1`; dirty-mirror warnings do not. Otherwise it compiles
selected-node types represented by either a present block or a nonempty mirror.
No targets is a successful no-op. It has no depth or compile mode and does not
traverse the mdoc dependency graph; language imports or includes define compiler
dependencies.

`mdc back` performs the inverse mirror-to-mdoc reconciliation. The manifest
baseline prevents either direction from overwriting independently modified
content. Clean imports can commit while another pair reports a conflict. Mirror
deletion removes a block; empty placeholders distinguish the normal five-file
layout using the stored block-presence bit.

Lean source files live under `.mdc/lean/Lib/` using the standard
`lake init Lib lib` library layout. `lakefile.toml` owns declarative dependencies
and retains its `[[lean_lib]] name = "Lib"` declaration. Lean also manages
`lean-toolchain`, rejects `lakefile.lean`, and preserves existing TOML/toolchain
files. Before a build, mdc
writes a single absolute import for the selected module to `Lib.lean`, then runs
`lake build +Lib`. Lake follows the module's imports and reuses `.lake/build`
artifacts; mdc never cleans `.olean` files.
For LaTeX, mdc writes the selected mirror as an input in `Lib.tex` and compiles
the user-editable `Main.tex` directly inside `.mdc/latex/`, without a separate
build tree; the default `Main.tex` is created only when absent.
Python executes the selected mirror directly. Rocq `.vo` files live in a parallel
`build/` tree rather than beside editable mirrors. Rocq digests all
`Lib/**/*.v`; a changed digest removes the complete build tree before compiling.
It compiles only the selected module, not its import closure.

`src/compiler/mod.rs` owns the core compiler request/result DTOs, `SrcCompiler`,
the registry, language registration, and `CompilerWorkspace`. The workspace
centralizes compiler-root validation, source resolution, and primitive generated
file operations. Language modules retain orchestration helpers for setup, driver
generation, cleanup, and inventory management. `SrcCompiler` has two required methods:
`srctype()` and `compile(req)`, and the default registry includes `text`,
`python`, `latex`, `lean`, and `rocq`. A result succeeds exactly when `rtcode` is
zero; interruption remains a separate flag so the CLI can stop compiling later
source types.

`src/compiler/process.rs` owns synchronous process execution, status mapping,
signal interception, process-group termination, timeout enforcement, bounded
stdout/stderr draining, and process diagnostics. Every command starts in its own
Unix process group. Normal completion, timeout, interruption, and I/O cleanup all
terminate ordinary descendants; deliberately escaped process groups are outside
that containment boundary. Output drains retain bounded head/tail context while
continuing to consume the pipes so large output cannot deadlock the wait loop.
Each stdout/stderr stream retains at most 1 MiB: the first 512 KiB and last
512 KiB, separated by an omission marker.

Per-srctype overrides are deserialized from `.mdc/config.toml`
`[src.<srctype>]` sections into positive integer timeout values. `Config::load()`
reports malformed values with their source section, and `Config::src_config()`
merges them with `default_for_srctype()` before `CompilerReq` is constructed.
Language compilers therefore receive typed, validated seconds and do not inspect
TOML values or apply defaults during compilation. When either managed Lean setup
file is missing, `setup_timeout_sec` gives `lake init` and
`lake env lean --version` separate deadlines. If both files already exist,
neither setup subprocess runs. `timeout_sec` applies to each subprocess-backed
language compiler.

## Reference Resolution

`IndCache::resolve_ref(raw_ref, cwd)` handles:

- Path-like references: contains `/`, ends in `.mdoc`, or starts with `.`.
- Exact fnode matches.
- Unique fnode prefixes.

Path-like refs are resolved against the current working directory and the
workspace root. Files under nested `.mdc/` roots are rejected.

`IndCache::resolve_start_ref(raw_ref, cwd)` adds a unique, case-insensitive exact
title fallback. `mdc serve [source]` and `mdc graph tui [source]` share this start
resolver; other command references retain the path/fnode/prefix contract.

`resolve_edit_target_path()` is the path-returning variant used by edit/refresh
commands. It can resolve existing files even if they are not yet indexed.

## VS Code Extension

The VS Code extension in `editors/vscode/` is a declaration-only language support
extension for `.mdoc` files. It contributes language registration, folding markers,
TextMate grammar, and embedded-language mappings for source blocks.
Embedded highlighting currently recognizes canonical lowercase
`@src: <type>` headers without metadata; valid case variants and metadata-bearing
headers require future grammar maintenance.

Local install from a packaged VSIX:

```bash
code --install-extension editors/vscode/mdc-mdoc-0.1.0.vsix --force
```

Package from source:

```bash
cd editors/vscode
npx @vscode/vsce package
```

Publish to Marketplace:

```bash
cd editors/vscode
npx @vscode/vsce login mdc
npx @vscode/vsce publish
```

For token-based publishing:

```bash
cd editors/vscode
npx @vscode/vsce publish -p "$VSCE_PAT"
```

Before public publishing, verify that `package.json` has the correct `publisher`,
bump the extension `version`, and consider adding Marketplace metadata such as
`repository`, `LICENSE`, and either `.vscodeignore` or a `files` allowlist.

## Documentation Roles

Keep `README.md` focused on using `mdc`. Keep this guide focused on development,
architecture, cache behavior, and release maintenance.
