---
title: Source workflow
---

Each node has at most one block of each supported type. The browser always
displays them in `text`, `latex`, `lean`, `rocq` order, regardless of when they
were added. Edit in the browser or send a complete block on stdin:

```sh
printf 'Explanation.\n' | mdc edit Example -p myproject/main --type text
mdc edit Example -p myproject/main --type text --delete
```

`edit` creates or replaces the block; an empty string is still an existing block.
`--delete` removes it and does not read stdin. Source edits and compilation are
separate operations. Only Lean has integrated compilation and language services.

Read `mdc show Example -p myproject/main` before editing. Pass its `revision` using
`--revision` to `edit`, `rename`, or dependency mutations to guard work based on
that read. Without it, the CLI fetches a revision immediately before writing.
Browser saves carry the revision they loaded. A stale write fails and leaves the
newer database state intact; browser drafts remain available for reconciliation.
See [Versioned mutations](../../development/safe-mutations/).

Lean Server elaborates changed documents incrementally. After **Save**, the
browser reuses that editor's native diagnostics and import information to certify
the exact saved source and compiled dependencies. Unsaved drafts cannot update
certification of different database contents. Native Lake builds imported
artifacts and, on `mdc lean check Example -p myproject/main --build`, the target
`.olean`. Matching artifacts are reused rather than rebuilt unconditionally.

The Lean block's **Lean import** disclosure shows the module name to use from
other nodes. The graph and the node's Lean status use the same colors: gray for
no Lean source (including an empty block), yellow for unchecked/failed checks or
native `sorry` warnings, and green for a current successful check without those
warnings. Rocq does not affect graph colors. Colors describe the saved node's
own Lean result, not a transitive axiom audit; a complete proof may still depend
on an admitted result. Comments and strings containing `sorry` do not count.

Older certificates without sorry evidence remain yellow until checked again.
Graph queries use in-memory certificates and never launch Lean. Certificates
are restored once at service startup; saving or certifying Lean refreshes graph
colors while preserving the graph viewport.

Managed Lean imports must agree with declared graph dependencies. Maintain edges
with `mdc dep`, and edit the corresponding imports in Lean source. Generated
compiler files are service-owned; they are not a second editing interface.
