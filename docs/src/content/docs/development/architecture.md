---
title: Architecture
description: Product model, Rust module boundaries, parsers, graph APIs, and reference resolution.
---

MathDoc is a Rust CLI and local application built around three durable artifacts:
plain-text `.mdoc` sources, a rebuildable SQLite index, and editable language mirrors.

## Product model

A workspace is recognized by a real, non-symlink `.mdc/` directory. `mdc init` creates
the control directory and configuration template; discovery depends on filesystem
properties rather than a marker recording how it was created.

Each `.mdoc` has a stable fnode, searchable title, ordered direct dependencies, and up
to one source block for each built-in type. Normal references are paths, exact fnodes,
or unique fnode prefixes.

## Module map

| Module | Responsibility |
| --- | --- |
| `src/mdocnode/` | `.mdoc` parser, serializer, and `MdocNode` model |
| `src/indcache/` | SQLite-backed workspace index (`IndCache`) |
| `src/depgraph/` | One-root dependency mutation session (`DepGraph`) |
| `src/metric/` | Read-only metric registry, context, and formulas |
| `src/compiler/` | Compiler registry, language implementations, subprocess control |
| `src/cli/` | Clap definitions, dispatch, command handlers, and terminal output |
| `src/web/` | Axum JSON API and embedded SPA asset serving |
| `src/core/` | Database-independent graph models and algorithms |
| `src/config.rs` | Source descriptors, config template, parsing, and defaults |
| `src/workspace/` | Discovery, path validation, mutation locking, safe file updates |
| `src/workdraft/` | Mirror manifest, reconciliation, batched application, rollback |
| `editors/vscode/` | Declaration-only VS Code language extension |
| `web/` | Svelte 5, Vite, TypeScript, CodeMirror, and graph frontend |

`src/core/` owns topological depth, weak-component, strongly connected component, and
cycle algorithms without depending on SQLite. `IndCache` adapts database rows into
those algorithms and persists derived results.

## Parsing boundaries

`MdocNode::load` fully parses a document and retains source block bodies. Index refresh
paths instead open regular files descriptor-relatively without following symlinks, read
each file once, and call `MdocHead::load_bytes`.

`MdocHead` validates the entire structure, including source headers and terminators,
but does not allocate or retain source bodies. `MdocNode::upsert_source_block()`
normalizes nonempty mutation content to the same trailing-newline representation the
parser produces.

Invalid documents are represented in `mdoc_issues` where possible. `MdocIdentity` is
intentionally lenient and can recover `(fnode, title)` from a broken document for useful
invalid and duplicate diagnostics.

## Dependency mutation

`DepGraph` is the primary API for changing one document's dependency list. It owns one
root `MdocNode` and borrows an `IndCache`; traversal and reachability remain in the
SQLite-backed index rather than a second in-memory graph.

Important entry points include:

- `DepGraph::from_ref(cache, ref_str, cwd)` resolves and refreshes a source before
  duplicate checks, then loads its root node.
- `DepGraph::create_root(...)` creates and indexes a new document and returns a graph
  rooted there.
- `add_direct_dependencies()` and `create_and_add_dependency()` reject cycles by
  checking whether a candidate can already reach the root.
- `remove_direct_dependencies()` saves and reindexes the root.

Direct file edits can still create cycles. `mdc graph check` remains authoritative for
state introduced outside the mutation API.

## Metrics

Metrics are read-only consumers of `IndCache`, not part of `DepGraph`.
`src/metric/mod.rs` defines `MetricContext`, `NodeMetric`, and the compile-time
`NodeMetricKind` registry. Concrete formulas stay together in `function.rs` rather than
using one module per metric.

Adding a metric requires a formula in `function.rs`, implementation and enum mapping in
`mod.rs`, and a typed Clap variant plus dispatch arm in `src/cli/`.
`IndCache::node_degrees()` is the shared data boundary for IOR.

## Reference resolution

`IndCache::resolve_ref(raw_ref, cwd)` classifies a value as path-like when it contains
`/`, ends in `.mdoc`, or starts with `.`. Paths are resolved against both the current
directory and workspace root. Nested `.mdc/` roots are rejected.

Non-path values resolve as exact fnodes, then unique fnode prefixes.
`resolve_start_ref()` adds a unique case-insensitive exact-title fallback for
`mdc serve` and `mdc graph tui`. `resolve_edit_target_path()` is the path-returning form
for edit and refresh commands and can resolve files not yet indexed.
