---
title: Dependency commands
---

```sh
mdc dep add --proj myproject/main 'Theorem' --target 'Lemma'
mdc dep rm --proj myproject/main 'Theorem' --target 'Lemma'
mdc dep show --proj myproject/main 'Theorem' --depth 1
mdc dep refs --proj myproject/main 'Lemma' --depth 1
mdc dep leaf --proj myproject/main 'Theorem'
```

`add` and `rm` mutate the source node's direct dependency set. Targets resolve by exact name or complete UUID. The browser provides the same operations, including atomic creation of a linked node. A transaction introducing a cycle is rejected. Lean imports remain author-written and must match these links for certification.
