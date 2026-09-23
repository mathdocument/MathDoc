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
other nodes. The graph and the node's Lean status use the same colors:

| Label | Color | Meaning |
| --- | --- | --- |
| Unverified | Gray | No Lean source (including an empty block), or unchecked, failed, stale or incomplete compiler evidence. |
| Sorry | Red | A current certified module has native `sorry` evidence. |
| Conditional | Yellow | A current certified module has no own `sorry`, but a direct or transitive managed dependency does. |
| Verified | Green | Current complete evidence reports no `sorry` in the module or its managed dependency closure. |

Rocq does not affect graph colors. Comments and strings containing `sorry` do not
count. Changing a dependency invalidates downstream statuses until checked again.
This tracks managed graph dependencies, not arbitrary axioms in external libraries.

Older certificates without sorry evidence remain Unverified until checked again.
When Lake restores artifacts without diagnostic logs, MathDoc inspects their
compiled proof bodies once with the pinned Lean compiler. Large closures use up
to four inspector processes, capped across the service; small closures use one.
The resulting certificates are persisted and shared across matching branches.
Graph queries use in-memory certificates and never launch Lean. Certificates
are restored once at service startup; saving or certifying Lean refreshes graph
colors while preserving the graph viewport.
Certification runs separately from LSP message forwarding and does not lock graph
operations. It uses the configured Lean timeout; a timeout reports a check error
without closing the editor connection, so checking can be retried in place.

Managed Lean imports must agree with declared graph dependencies. Maintain edges
with `mdc dep`, and edit the corresponding imports in Lean source. Generated
compiler files are service-owned; they are not a second editing interface.
