---
title: Graph & Metrics
description: Reference for graph validation, roots, interactive browsing, and node metrics.
---

## `mdc graph check`

Perform a strong repository-wide validation:

```bash
mdc graph check
mdc graph check --prof
```

The report contains:

- discovered `.mdoc` count
- valid-source edge count
- missing dependency targets
- invalid files, including duplicate fnodes
- one representative cycle from each cyclic strongly connected component

The strong refresh reads and parses every document, replaces the base graph rows,
rebuilds in-degree, and recomputes all topological depths. Representative cycles are
then calculated directly for the report.

## `mdc graph roots`

List nodes with no incoming valid edge:

```bash
mdc graph roots
```

Recoverable broken entries remain visible. Results are ordered by descending
topological depth, then weak-component size, path, and fnode. The command discovers
filesystem changes using cached `(mtime, size)` metadata and reads persisted topological
depths; weak-component sizes are calculated directly for the query.
An external same-size edit that retains its timestamp may remain stale until a strong
refresh such as `mdc graph check` or `mdc sync`.

## `mdc graph tui`

Open the terminal graph browser:

```bash
mdc graph tui
mdc graph tui <ref>
```

Without a start reference, the TUI selects the first root in `graph roots` ordering,
which is the deepest root rather than necessarily the largest subtree. The optional
start value also accepts a unique case-insensitive exact title.

The TUI supports graph navigation, ranked search, source preview, `$EDITOR` launch,
dependency addition and removal, and creation of linked nodes.

## `mdc metric ior`

Evaluate the smoothed in-degree/out-degree ratio for one node:

```bash
mdc metric ior <source>
```

For node `p`, the metric is:

```text
ln((in_degree(p) + 1) / (out_degree(p) + 1))
```

It is positive when a node has more incoming than outgoing edges, negative in the
opposite case, and zero when the degrees match. Internally MathDoc evaluates
`ln1p(in_degree) - ln1p(out_degree)` to avoid constructing an intermediate ratio.

The command strongly refreshes the complete index and accepts the normal
path/fnode/prefix references. Edges from an invalid or duplicate source are excluded;
an outgoing edge to a missing target still contributes to out-degree. The selected
source must resolve to one valid, uniquely indexed node.

Standard output contains only the finite numeric value, making the command suitable for
scripts. Diagnostics and `--prof` output go to standard error.
