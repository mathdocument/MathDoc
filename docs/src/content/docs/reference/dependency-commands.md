---
title: Dependency commands
---

All commands require `-p/--proj DATABASE/BRANCH`. References are exact node names
or full UUIDs; use UUIDs when names are ambiguous.

```sh
mdc dep add 'Theorem' -t 'Lemma' -p myproject/main --revision NODE_REV
mdc dep rm 'Theorem' -t 'Lemma' 'Another lemma' -p myproject/main --revision NODE_REV
mdc dep show 'Theorem' -d 1 -p myproject/main
mdc dep refs 'Lemma' -d -1 -p myproject/main
mdc dep leaf 'Theorem' -p myproject/main
mdc dep candidates 'Theorem' lemma -n 20 -p myproject/main
```

`add` adds one direct edge. `rm` removes one or more targets in a single
transaction. Every target is resolved before writing; one invalid target or a
stale source revision rejects the whole operation. Repeating an already-present
add or removing an absent edge does not change the dependency set.

`show` traverses outgoing dependencies; `refs` traverses incoming referrers.
`-d/--depth` defaults to 1, accepts 0 for no neighbors and -1 for unlimited depth.
Results exclude the source node, deduplicate reachable nodes and report BFS
distance in `depth`. `leaf` follows all dependencies and returns reachable nodes
with no outgoing edges, also excluding the source itself.

`candidates SOURCE [QUERY]` searches titles and UUIDs case-insensitively. It excludes
the source and its existing direct dependencies, and returns `{nodes, empty}`.
`-n/--max-results` is 0–200, default 200; the query defaults to empty. Candidates
are suggestions, not proof that adding the edge is acyclic. The write validates
cycles and other graph constraints before committing.

`mdc new -t TITLE --parent SOURCE` creates a node and its parent edge atomically.
For `new --parent`, `dep add` and `dep rm`, `--revision` refers to the parent/source
revision from `show`. Without it the CLI reads a current revision before writing.
Lean imports remain author-written and must match declared dependencies for
certification; graph mutations do not rewrite source code.
