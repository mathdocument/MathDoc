---
title: Dependency commands
---

```sh
mdc dep add 'Theorem' --target 'Lemma'
mdc dep rm 'Theorem' --target 'Lemma'
mdc dep show 'Theorem' --depth 1
mdc dep refs 'Lemma' --depth 1
mdc dep leaf 'Theorem'
```

`add` and `rm` mutate the source node's direct dependency set. Targets resolve by exact name or complete UUID. The browser provides the same operations, including atomic creation of a linked node. A transaction introducing a cycle is rejected. Lean imports remain author-written and must match these links for certification.
