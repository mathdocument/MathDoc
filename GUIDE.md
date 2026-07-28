# Development Guide

`README.md` is the user guide. This file is the developer guide: architecture,
maintenance commands, cache semantics, and editor-extension maintenance.

After completing a development task, suggest a conventional-commit message and
say whether the Cargo package version should be bumped.

## Commands

```bash
cargo build                     # debug build
cargo build --release           # release build; binary: target/release/mdc
cargo test                      # all tests, including integration tests
cargo test <name>               # run tests matching a name substring
cargo test --test test_indcache # run one integration test target
cargo fmt                       # format Rust code
cargo clippy                    # lint

# Web frontend (mdc serve)
cd web && npm install           # first-time setup
cd web && npm run build         # write web/dist/ (embedded by cargo build --release)
cd web && npm run check         # svelte-check type/syntax pass
cargo run --features dev-web -- serve --bind 127.0.0.1:7599 --no-open
cd web && npm run dev                  # separate terminal; open Vite HMR on :5173
```

Integration tests live in `tests/`. Unit tests are inline in source files.
Release builds embed the committed `web/dist/` output. Keep it in sync with web
source changes by running `npm run build`; the build intentionally bundles only
the five supported source languages and one theme.

## Product Model

`mdc` manages a workspace of `.mdoc` files. A workspace is any directory that
contains `.mdc/`, created by `mdc init`.

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

- `@fnode` is the stable node ID. It is normally a UUID string; the first 8
  characters are used for short display.
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
| `src/compiler/` | Synchronous subprocess compilers and `CompilerRegistry` |
| `src/cli/` | `clap` command definitions, command handlers, and terminal output |
| `src/web/` | `mdc serve` HTTP server (axum): JSON API over `IndCache`/`DepGraph` + SPA asset serving |
| `src/core/` | Shared models and database-independent graph algorithms (topo depth, SCCs, and cycles) |
| `src/config.rs` | `.mdc/config.toml`, its initialization template, and compiler defaults |
| `src/workspace/` | Workspace initialization/discovery, path validation, mutation locking, and atomic file updates |
| `src/workdraft/` | Source mirror manifest, safe layout, transactional updates, and `sync`/`back` reconciliation policy |
| `editors/vscode/` | VS Code language extension for `.mdoc` files |
| `web/` | Svelte 5 + Vite + TypeScript frontend; built output is embedded into the `mdc` binary via `rust-embed` |

## `.mdoc` Parsing

`MdocNode::load` fully parses a file, including block contents. `MdocNode::load_head`
parses only headers and dependency structure. The index does not retain source
block contents.

Invalid files are represented in `mdoc_issues` where possible. `MdocIdentity`
is intentionally lenient and can recover `(fnode, title)` from a broken file so
the cache can report useful invalid/duplicate diagnostics.

## IndCache

The SQLite database lives at `.mdc/index.db`. Current schema version is `15`.
The DB is opened with WAL mode and foreign keys enabled.

Internal module boundaries are strict:

- `schema.rs`: table DDL and schema migrations (`PRAGMA user_version`).
- `queries.rs`: read queries and read-derived computations. Functions take
  `&Connection` and do not own transactions.
- `refresh.rs`: write/upsert/delete operations and derived-data maintenance.
  Functions take `&Connection` and do not own transactions.
- `discovery.rs`: workspace enumeration and `(mtime_ns, size)` change discovery.

`IndCache` owns the SQLite connection and all transaction boundaries. Multi-step
mutations should be wrapped as `conn.transaction()` followed by `tx.commit()` in
`src/indcache/mod.rs`.

Indexed node creation is also cache-owned: `IndCache::create_node()` validates the
node against the cache root and workspace mutation lock, renders it, creates any
parents, revalidates the final path, and atomically writes against
`FileSnapshot::Missing`. Index failure rolls the file back and repairs the path's
index state. Standalone `MdocNode::save_new()` remains available outside this
production indexed-creation path.

`CHUNK_SIZE = 500` is used for SQL `IN (...)` chunks and bulk inserts to stay
under SQLite variable limits.

### Database Tables

| Table | Key columns | Purpose |
| --- | --- | --- |
| `mdoc_files` | `path`, `mtime_ns`, `size` | File-state cache for change detection and stale-path cleanup |
| `mdocs` | `path`, `fnode`, `title`, `title_lc`, `topo_depth` | Searchable node cache, reference resolution, persisted topo depth |
| `mdoc_edges` | `src_path`, `src_fnode`, `dst_fnode`, `ord` | Dependency edges in source order |
| `mdoc_issues` | `path`, `kind`, `ref_fnode`, `error` | Structural problems: `invalid`, `duplicate`, `missing` |
| `mdoc_in_degree` | `fnode`, `in_degree` | Precomputed in-degree for root detection |
| `mdoc_weak_component` | `fnode`, `component_size` | Weak connected-component size for each node |
| `mdoc_index_state` | `graph_epoch`, `weak_component_dirty`, `bootstrapped` | Epoch, weak-component dirty state, and bootstrap state |
| `mdoc_scc_result` | `graph_epoch`, `cycles_json` | Cached SCC/cycle result, invalidated by epoch change |

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

`refresh_all()` is the full rescan path used by `mdc sync`. It walks the whole
workspace, stats every `.mdoc`, reconciles stale paths, and rebuilds derived data.

### Derived Data

`mdocs.topo_depth` and `mdoc_weak_component` are derived summaries of the graph.

`topo_depth` is persisted in `mdocs`. Leaves have depth `0`; every other node has
`1 + max(depth(dependency))`. `all_topo_depths()` is a plain `SELECT`, not a graph
walk. This keeps TUI and root-list displays cheap.

Bulk paths call `refresh_all_derived_data()`, which loads the graph once, runs
the core Kahn-style topo-depth algorithm, recomputes weak components with BFS, and
persists both results.

Incremental paths call `refresh_topo_depth_upward_from(fnode)`. It recomputes the
changed node and all reverse-reachable ancestors in dependency-first order. If
the affected subgraph contains a cycle and therefore cannot be ordered, it falls
back to a full topo-depth backfill.

When one node's `@dep` list changes and its `@fnode` stays the same, the update is
therefore targeted, not full-graph: rewrite that node's edges, recompute its
`topo_depth`, then propagate upward through its referrers.

`weak_component_dirty` belongs to weak connected components, not topo depth. A
graph change calls `bump_graph_epoch()`, which increments `graph_epoch` and sets
`weak_component_dirty = 1`. `global_root_items()` checks the dirty bit; if set, it
recomputes weak components before reading. If clear, it reads `topo_depth` and
`component_size` directly with a JOIN.

`mdoc_scc_result` is invalidated by `graph_epoch`. `graph_check_report()` reuses
cached cycles when the stored epoch matches the current epoch; otherwise it
recomputes SCCs and representative cycles.

### Per-Command Cache Behavior

| Command | Discovery | Content refresh | Notes |
| --- | --- | --- | --- |
| `mdc init` | none | none | `workspace::initialize()` creates `.mdc/` and the template from `config`; does not touch the index |
| `mdc new` | none | `upsert_path()` on created file | New file is indexed immediately |
| `mdc edit` | `discover_workspace_changes()` | `upsert_path()` after editor exits | Opens `$EDITOR` |
| `mdc sync` | none | `refresh_all()` | Full rescan, then mirrors all five source types under `.mdc/<srctype>/Lib/<relative-path>.<ext>` |
| `mdc search` | `discover_workspace_changes()` | none | Reads `mdocs`; does not re-stat unchanged known files |
| `mdc dep add` | through `DepGraph::from_ref` | inside `add_direct_dependencies()` or `create_and_add_dependency()` | `--target` refreshes and uniquely resolves one target before cycle checking |
| `mdc dep rm` | through `DepGraph::from_ref` | inside `remove_direct_dependencies()` | `--target` also resolves prefixes of dangling direct dependencies |
| `mdc dep show` | `discover_workspace_changes()` | `refresh_reachable_from_path()` on source | Targeted reachable refresh; exits `1` if cycles are reported |
| `mdc dep leaf` | `discover_workspace_changes()` | `refresh_reachable_from_path()` on source | Targeted reachable refresh; exits `1` if cycles are reported |
| `mdc dep refs` | `discover_workspace_changes()` | `upsert_path()` on target | Target refreshed before reverse-edge query |
| `mdc graph check` | none | `refresh_all()` | Reports missing, invalid, duplicate, and cycle issues from a strong full refresh |
| `mdc graph roots` | `discover_workspace_changes()` | none | Reads persisted `topo_depth`; graph load only if weak components are dirty |
| `mdc graph tui` | `discover_workspace_changes()` | DepGraph mutation APIs plus post-op discovery | TUI add/rm/create delegate to DepGraph; start refs also accept a unique exact title |
| `mdc work` | `discover_workspace_changes()` | `workdraft::sync()` before compiling the selected mirrors | Dirty mirrors are compiled without being overwritten |
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
`refresh_reachable_from_path()`, and `mdc sync` uses `refresh_all()`. Commands that
only need search/display data, such as `mdc search` and `mdc graph roots`, avoid
file-by-file re-stat work for latency.

After refreshing the index, `mdc sync` parses every valid workspace `.mdoc` and
exports exactly five source mirrors under `.mdc/<srctype>/Lib/`, preserving the
source's relative directory structure. Missing blocks become empty files.
`.mdc/source-blocks.json` tracks which source paths were generated so deleted or
renamed sources can be cleaned without touching unrelated files. It also records
the content digest and block-presence baseline for every source/srctype pair.
`mdc sync` updates a mirror only when it still matches that baseline; `mdc back`
performs the inverse check before updating mdoc. Independent changes on both
sides are preserved and reported. Invalid documents retain their previous
files.

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

## Web Frontend (`mdc serve`)

`mdc serve` runs an axum HTTP server that serves a JSON API over
`IndCache`/`DepGraph` and a Svelte 5 SPA. The SPA is the interactive
replacement for `mdc graph tui`: a vertical three-column layout
(upstream referrers on the left, focused-node editor in the center,
downstream dependencies on the right). Clicking a card or pressing
`h`/`j`/`k`/`l`/`Enter` navigates between nodes; navigation is animated
via the View Transitions API with a synchronous fallback.

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
  `dev-web` cargo feature, `tower-http::ServeDir` serves `web/`
  directly so Vite HMR works.
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

### Concurrency Model

`IndCache` owns a single SQLite connection. The web server wraps it in
`Arc<Mutex<IndCache>>`, so cache operations serialize on that mutex. Dependency
handlers construct `DepGraph` by borrowing the same cache. The separate mutation
mutex serializes each complete write operation, while the workspace mutation lock
coordinates writes with other mdc processes. The direct title/block helper holds
both boundaries across resolve, source snapshot/parse, fnode recheck, mutation,
conflict-aware replacement, index update, and committed response construction.

### Frontend Build

Release builds embed the committed production output in `web/dist` at compile
time. Frontend source changes must include a fresh `cd web && npm run build` so
the hashed JS/CSS assets and `index.html` remain synchronized.

For development, use the `dev-web` cargo feature and run Vite in a
second terminal — see the Commands section above.

## Work/Back and Compilers

`mdc work <source>` first reconciles mdoc blocks into the stable `Lib/` trees and then
always compiles the selected node's present source types. It has no depth or
compile mode and does not traverse the mdoc dependency graph. Language imports or
includes in mirrored files define compiler dependencies.

`mdc back` performs the inverse mirror-to-mdoc reconciliation. The manifest baseline
prevents either direction from overwriting independently modified content. Mirror
deletion removes a block; empty placeholders distinguish the normal
five-file layout using the stored block-presence bit.

Lean source files live under `.mdc/lean/Lib/` using the standard
`lake init Lib lib` library layout. `lakefile.toml` owns declarative dependencies
and retains its `[[lean_lib]] name = "Lib"` declaration. Before a build, mdc
writes a single absolute import for the selected module to `Lib.lean`, then runs
`lake build +Lib`. Lake follows the module's imports and reuses `.lake/build`
artifacts; mdc never cleans `.olean` files.
For LaTeX, mdc writes the selected mirror as an input in `Lib.tex` and compiles
the user-editable `Main.tex` directly inside `.mdc/latex/`, without a separate
build tree; the default `Main.tex` is created only when absent.
Python executes the selected mirror directly. Rocq `.vo` files live in a parallel
`build/` tree rather than beside editable mirrors.

`SrcCompiler` has two required methods: `srctype()` and `compile(req)`. The default
registry includes `text`, `python`, `latex`, `lean`, and `rocq`. Compilers are
synchronous subprocess runners. `run_process()` drains stdout and stderr in
background threads immediately after spawn so large outputs cannot deadlock the
timeout loop.

Per-srctype config is read from `.mdc/config.toml` `[src.<srctype>]` sections and
merged with built-in defaults from `default_for_srctype()`.

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
