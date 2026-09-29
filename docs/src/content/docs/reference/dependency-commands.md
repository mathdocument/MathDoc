---
title: Dependency commands
---

All commands work on the branch from `-p DATABASE/BRANCH` or `MDC_PROJECT`.
References are exact node titles or full UUIDs; use UUIDs when titles repeat.

```sh
mdc dep add 'Theorem' -t 'Lemma' -p myproject/main --revision NODE_REV
mdc dep rm 'Theorem' -t 'Lemma' 'Another lemma' -p myproject/main --revision NODE_REV
mdc dep show 'Theorem' -d 1 -p myproject/main
mdc dep refs 'Lemma' -d -1 -p myproject/main
mdc dep leaf 'Theorem' -p myproject/main
mdc dep candidates 'Theorem' lemma -n 20 -p myproject/main
```

`add` adds one direct edge: the source depends on the target. `rm` removes one or
more targets in a single commit. Every target is resolved before writing; one
unknown target or a stale source revision rejects the whole operation. Adding an
existing edge or removing an absent one changes nothing.

`show` follows outgoing dependencies; `refs` follows incoming referrers.
`-d/--depth` defaults to 1; 0 returns nothing and -1 means unlimited. Results
exclude the source node, list each reachable node once and report its
breadth-first distance in `depth`. `leaf` follows all dependencies and returns
the reachable nodes without outgoing edges.

`candidates SOURCE [QUERY]` searches titles and UUIDs case-insensitively,
excluding the source and its existing direct dependencies, and returns
`{nodes, empty}`. `-n` is capped at 200; the CLI asks for 200 by default. A
candidate is a suggestion only: the write itself rejects cycles.

`mdc new -t TITLE --parent SOURCE` creates a node and the edge from SOURCE in one
commit. For `new --parent`, `dep add` and `dep rm`, `--revision` is the revision
of the parent or source node from `show`.

Edges decide how a proof request treats Lean nodes: direct theorem dependencies
become premises and definition dependencies are registered
([Proofs with LeanGround](../proofs/#node-conversion)). Changing edges does not
rewrite any source.
