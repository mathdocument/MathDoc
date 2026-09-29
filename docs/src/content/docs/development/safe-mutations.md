---
title: Versioned mutations
---

## Documents

Node edits, renames, deletions, dependency changes and linked creation require a
quoted `If-Match` node revision; linked creation guards the parent. LaTeX (and
legacy Lean) project writes use the branch data revision from their `GET`.
Missing or unquoted revisions return `428`; stale ones return `412`. CLI
`--revision` supplies the revision from an earlier read; otherwise the CLI reads
it just before writing.

Unlinked creation, whole-branch import and branch creation take no caller
revision. All writes run under the branch lock and commit against the TerminusDB
data version they read, so a concurrent writer elsewhere (another API process, or
a writeback by the worker) cannot be silently overwritten. Branch creation forks
the source head at request time; there is no merge operation.

Graph constraints and block types are validated before commit. Linked creation
writes the new node and the parent edge in one commit. Batch dependency removal
resolves every target first, so an unknown target cannot cause a partial removal.
Import validates the complete bundle and commits only into an empty branch.

## Collaboration

Project commands are guarded twice:

- `If-Match: "REVISION"` must equal the board's current revision
  (`409 revision_conflict` otherwise).
- `Idempotency-Key` identifies the intent. The server stores each
  (project, actor, key) response; the same request again returns the stored
  response without applying it twice, and a different request under the same key
  fails with `idempotency_conflict`. Project creation uses the same mechanism.

Leases carry an epoch, so a participant whose lease expired or was reassigned
gets `stale_lease` instead of overwriting the new holder. Worker jobs recheck
their own claim before writing results.

Writeback checks every node against the text it was converted from, then commits
the whole batch at once with the data version it read; any mismatch aborts the
batch without writing, and a concurrent commit makes the job retry.

## Trust boundary

Requests need an access token; roles limit what each actor can do; the same-origin
check blocks cross-site browser requests to the document API. The server runs no
Lean and no TeX: the LaTeX renderer expands macros with plasTeX, and TikZ diagrams
are rendered by a WebAssembly TeX in the reader's browser. See [HTTP API](../../reference/http-api/) for status codes and limits.
