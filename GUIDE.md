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
```

Integration tests live in `tests/`. Unit tests are inline in source files.

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
| `src/depgraph/` | In-memory dependency graph and mutation API (`DepGraph`) |
| `src/depgraph/workback.rs` | Work-file merge and extraction logic for `mdc work` / `mdc back` |
| `src/compiler/` | Synchronous subprocess compilers and `CompilerRegistry` |
| `src/cli/` | `clap` command definitions, command handlers, and terminal output |
| `src/core/` | Shared models and graph algorithms: topo order, cycle detection, SCC |
| `src/config.rs` | `.mdc/config.toml`, srctype defaults, preamble/postamble files |
| `src/workspace.rs` | Workspace discovery, `.mdoc` iteration, relative path helpers |
| `editors/vscode/` | VS Code language extension for `.mdoc` files |

## `.mdoc` Parsing

`MdocNode::load` fully parses a file, including block contents. `MdocNode::load_head`
parses only headers and dependency structure. The index uses head parsing so cache
refreshes do not read large source blocks unnecessarily.

Invalid files are represented in `mdoc_issues` where possible. `read_mdoc_head`
is intentionally lenient and can recover `(fnode, title)` from a broken file so
the cache can report useful invalid/duplicate diagnostics.

## IndCache

The SQLite database lives at `.mdc/index.db`. Current schema version is `8`.
The DB is opened with WAL mode and foreign keys enabled.

Internal module boundaries are strict:

- `schema.rs`: table DDL and schema migrations (`PRAGMA user_version`).
- `queries.rs`: read queries and read-derived computations. Functions take
  `&Connection` and do not own transactions.
- `refresh.rs`: write/upsert/delete operations and derived-data maintenance.
  Functions take `&Connection` and do not own transactions.
- `discovery.rs`: directory-mtime-based workspace discovery.

`IndCache` owns the SQLite connection and all transaction boundaries. Multi-step
mutations should be wrapped as `conn.transaction()` followed by `tx.commit()` in
`src/indcache/mod.rs`.

`CHUNK_SIZE = 500` is used for SQL `IN (...)` chunks and bulk inserts to stay
under SQLite variable limits.

### Database Tables

| Table | Key columns | Purpose |
| --- | --- | --- |
| `mdoc_dirs` | `path`, `mtime_ns` | Directory mtime cache for cheap add/delete/rename detection |
| `mdoc_files` | `path`, `mtime_sec`, `mtime_ns`, `size` | File-state cache for change detection and stale-path cleanup |
| `mdocs` | `path`, `fnode`, `title`, `title_lc`, `mtime_sec`, `mtime_ns`, `size`, `topo_depth` | Searchable node cache, reference resolution, persisted topo depth |
| `mdoc_edges` | `src_path`, `src_fnode`, `dst_fnode`, `ord` | Dependency edges in source order |
| `mdoc_issues` | `path`, `kind`, `ref_fnode`, `error` | Structural problems: `invalid`, `duplicate`, `missing` |
| `mdoc_in_degree` | `fnode`, `in_degree` | Precomputed in-degree for root detection |
| `mdoc_weak_component` | `fnode`, `component_id`, `component_size` | Weak connected components with stable representative and size |
| `mdoc_index_state` | `graph_epoch`, `weak_component_dirty`, `bootstrapped`, `topo_depth_backfilled` | Epoch, dirty flags, bootstrap and migration state |
| `mdoc_scc_result` | `graph_epoch`, `cycles_json` | Cached SCC/cycle result, invalidated by epoch change |

## Cache Refresh Model

There are two broad refresh paths.

`discover_workspace_changes()` is the fast path used by most commands. It compares
cached directory `mtime_ns` values with filesystem directory mtimes. Unchanged
directories are not re-statted file-by-file; changed directories are scanned for
added, updated, deleted, and moved `.mdoc` files. Add/update operations trigger
incremental topo-depth refreshes. Any deletion triggers full topo-depth backfill,
because ancestor depths may decrease and there may be no single safe starting
node.

`refresh_all()` is the full rescan path used by `mdc sync`. It walks the whole
workspace, stats every `.mdoc`, reconciles stale paths, and rebuilds derived data.

`refresh_workspace_index()` sits between those two. It discovers new/deleted files
and also re-stats all already-indexed paths. `mdc graph check` uses it so graph
validation is based on actual file content, not only directory discovery.

### Derived Data

`mdocs.topo_depth` and `mdoc_weak_component` are derived summaries of the graph.

`topo_depth` is persisted in `mdocs`. Leaves have depth `0`; every other node has
`1 + max(depth(dependency))`. `all_topo_depths()` is a plain `SELECT`, not a graph
walk. This keeps TUI and root-list displays cheap.

Bulk paths call `refresh_all_derived_data()`, which loads the graph once, runs
Kahn-style topo-depth computation, recomputes weak components with BFS, and
persists both results.

Incremental paths call `refresh_topo_depth_upward_from(fnode)`. It recomputes the
changed node, compares old and new `topo_depth`, and only if the value changed
walks reverse edges to nodes that depend on it. The propagation stops at any
ancestor whose depth remains unchanged.

When one node's `@dep` list changes and its `@fnode` stays the same, the update is
therefore targeted, not full-graph: rewrite that node's edges, recompute its
`topo_depth`, then propagate upward through referrers only as needed.

`weak_component_dirty` belongs to weak connected components, not topo depth. A
graph change calls `bump_graph_epoch()`, which increments `graph_epoch` and sets
`weak_component_dirty = 1`. `global_root_items()` checks the dirty bit; if set, it
recomputes weak components before reading. If clear, it reads `topo_depth` and
`component_size` directly with a JOIN.

`upsert_path()` additionally attempts an incremental weak-component update for a
single-file upsert. Edge additions are handled with component union. Edge removals
run a split check within the old component. Fnode changes and deletions fall back
to full weak-component recompute.

`mdoc_scc_result` is invalidated by `graph_epoch`. `graph_check_report()` reuses
cached cycles when the stored epoch matches the current epoch; otherwise it
recomputes SCCs and representative cycles.

`topo_depth_backfilled` is a migration recovery flag. `open_db()` returns whether
`topo_depth` needs a backfill. `IndCache::open` backfills and sets the flag in one
transaction before serving reads, so a crash between migration and backfill is
recovered on the next open.

### Per-Command Cache Behavior

| Command | Discovery | Content refresh | Notes |
| --- | --- | --- | --- |
| `mdc init` | none | none | Creates `.mdc/` and config files; does not touch the index |
| `mdc new` | none | `upsert_path()` on created file | New file is indexed immediately |
| `mdc edit` | `discover_workspace_changes()` | `upsert_path()` after editor exits | Opens `$EDITOR` |
| `mdc sync` | none | `refresh_all()` | Intentional full-rescan escape hatch |
| `mdc search` | `discover_workspace_changes()` | none | Reads `mdocs`; does not re-stat unchanged known files |
| `mdc dep add` | through `DepGraph::from_ref` | inside `add_direct_dependencies()` or `create_and_add_dependency()` | Cycle-creating dependencies are rejected before write |
| `mdc dep rm` | through `DepGraph::from_ref` | inside `remove_direct_dependencies()` | Index update is part of DepGraph mutation |
| `mdc dep show` | `discover_workspace_changes()` | `refresh_reachable_from_path()` on source | Targeted reachable refresh; exits `1` if cycles are reported |
| `mdc dep leaf` | `discover_workspace_changes()` | `refresh_reachable_from_path()` on source | Targeted reachable refresh; exits `1` if cycles are reported |
| `mdc dep refs` | `discover_workspace_changes()` | `upsert_path()` on target | Target refreshed before reverse-edge query |
| `mdc graph check` | `refresh_workspace_index()` | Re-stats all indexed paths | Reports missing, invalid, duplicate, and cycle issues |
| `mdc graph roots` | `discover_workspace_changes()` | none | Reads persisted `topo_depth`; graph load only if weak components are dirty |
| `mdc graph tui` | `discover_workspace_changes()` | DepGraph mutation APIs plus post-op discovery | TUI add/rm/create delegate to DepGraph |
| `mdc work` | `discover_workspace_changes()` | `upsert_path()` on source and `refresh_reachable_from_path()` | Skips work files with unsaved edits |
| `mdc back` | `discover_workspace_changes()` before write | none after write | Writes `.mdoc` files; cache metadata may remain stale until targeted refresh or `mdc sync` |

### File Change Detection

CLI-managed writes should call `upsert_path()` directly, or route through a
DepGraph mutation that calls it. `mdc new`, `mdc edit`, `mdc dep add`, and
`mdc dep rm` follow this rule.

External adds, deletes, and renames are detected by `discover_workspace_changes()`
through directory mtime tracking.

External content edits to already-known files are not guaranteed to change a
directory mtime. Commands that need fresh dependency content use
`refresh_reachable_from_path()`, and `mdc sync` uses `refresh_all()`. Commands that
only need search/display data, such as `mdc search` and `mdc graph roots`, avoid
file-by-file re-stat work for latency.

Timestamp state is stored as `mtime_sec` and `mtime_ns`, where `mtime_ns` is
`secs * 1_000_000_000 + subsec_nanos`.

## DepGraph

`DepGraph` is the primary API for mutating one document's dependency tree. It owns
an `IndCache` plus `GraphState`, which stores loaded `MdocNode`s and an in-memory
`HashMap<fnode, Vec<fnode>>` dependency graph.

Important constructors and operations:

- `DepGraph::new(root, fnode)` opens the cache, bootstraps it, and loads a root.
- `DepGraph::from_ref(cache, ref_str, cwd)` resolves a path/fnode/prefix, indexes
  the resolved file before duplicate checks, and loads the root node.
- `DepGraph::create_root(...)` creates a new `.mdoc`, indexes it, and returns a
  graph rooted at the new node.
- `DepGraph::scan_all()` loads every workspace `.mdoc` into memory for full-graph
  checks.
- `add_direct_dependencies()` and `create_and_add_dependency()` reject cycles by
  checking whether the candidate dependency can already reach the root in the
  indexed graph.
- `remove_direct_dependencies()` saves the root node and reindexes it.
- `ordered_nodes(depth)` expands the root subgraph, checks for cycles, and returns
  dependency-first order using `topo_dependencies_first()`.

Direct file edits can still create cycles. `mdc graph check` is the authoritative
reporting path for cycles introduced outside the mutation API.

## Work/Back and Compilers

`mdc work` builds one work file per srctype present in the dependency subgraph.
Files are written under `.mdc/<srctype>/MdcWork.<ext>`, with `.MdcWork.hash` as a
sidecar for unsaved-edit detection.

`depens` controls whether ancestor blocks of the same srctype are included.
`reverse_depens` controls order: `true` puts the root first, `false` puts deepest
dependencies first and the root last.

Preamble and postamble live in `.mdc/<srctype>/preamble.<ext>` and
`.mdc/<srctype>/postamble.<ext>`. Only LaTeX has non-empty built-in defaults.

`mdc back` parses `MdcWork` marker sections, refuses to sync structurally suspect
files, treats `title:` marker lines as read-only, writes changed node block content
back to `.mdoc` files, and updates hashes only on clean sync.

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
