---
title: Source workflow
---

## Blocks

Each node has at most one block of each type: `text`, `latex`, `lean`, `rocq`.
The browser always shows them in that order. Edit in the browser, or send a
complete block on stdin:

```sh
printf 'Explanation.\n' | mdc edit Example -p myproject/main --type text
mdc edit Example -p myproject/main --type text --delete
```

`edit` creates or replaces the block (`--type` defaults to `lean`); an empty
string is still an existing block. Replacing a block keeps its metadata.
`--delete` removes the block and reads no stdin. Saving stores source only;
MathDoc never compiles Lean or Rocq.

## Revisions

A node's revision is a SHA-256 of its own content: title, `module`,
dependencies and blocks with their metadata. Read `mdc show Example -p
myproject/main` before editing and pass its `revision` with `--revision` to
`edit`, `rename`, `del`, `new --parent` or the dependency commands. The write
fails with `412` if the node changed since that read, leaving the newer state
intact. Without `--revision` the CLI reads the current revision just before
writing, which protects only against changes in between. Browser saves carry the
revision they loaded; on conflict the draft stays in the editor for
reconciliation. See [Versioned mutations](../../development/safe-mutations/).

## Lean blocks

Lean blocks are ordinary Monaco source editors with Lean syntax highlighting.
MathDoc runs no Lean; proofs are checked only by LeanGround. A Lean node has two
status dimensions:

- **Local check**: never performed (reported as `unverified` when a Lean block
  exists, `no_code` otherwise).
- **LeanGround certification**: `not_submitted`, `insufficient`, `derivable`,
  `certified`, `stale`, `rejected`, or `registered` for a definition node. See
  [Proofs with LeanGround](../../reference/proofs/#statuses).

To check a proof, submit the node to LeanGround ([Proofs](../../reference/proofs/)).
For that, write each Lean block in the form the converter accepts:

- Start every top-level command in column 0 and indent its continuation lines;
  the converter splits a block at unindented lines without elaborating it.
- One node, one role: either definitions only (`def`, `abbrev`,
  `noncomputable def`, `structure`, `inductive`) or exactly one `theorem`/`lemma`
  as the node's conclusion. Blocks with several theorems need the block metadata
  `lean_conclusion`; mixed blocks also need `lean_role`. Block metadata can
  currently be set only through an [import bundle](../import-export/); editing a
  block keeps its metadata. Splitting such a node is usually simpler.
- Leave a proof as `sorry` (or `by sorry`) to make it an open goal for others.
- Depend on other nodes through graph edges, not `import` lines: imports are
  dropped on submission. A theorem node's direct theorem dependencies become its
  premises; its definition dependencies are registered in LeanGround. A
  definition node cannot depend on a theorem node. The names declared in a
  theorem node and its premises' conclusion names must all differ.
- `open`, `set_option`, `universe` and `variable` lines may be left out. Lines of
  a kind the block does contain must equal the database's proof environment
  context of that kind, line for line and in order.
- Any other top-level command is rejected with a reason on the node, for example
  `instance` and `class` (they must already be in the proof environment's base),
  `namespace`, `section`, attributes such as `@[simp]`, and `open … in`.

Graph edges are the only dependency information MathDoc uses: nodes without a
Lean block are informal and are not followed when collecting a proof's Lean
dependencies.
