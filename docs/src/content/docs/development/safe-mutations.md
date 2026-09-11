---
title: Versioned mutations
---

Node edits, renames, dependency mutations, Lean checks/goals and editor creation
require a quoted `If-Match` node revision. Linked node creation guards the parent
revision. Project configuration writes use the branch data revision from
`GET /api/project/lean` instead. Missing or unquoted revisions return 428; stale
revisions return 412. CLI `--revision` supplies the read version explicitly;
otherwise the CLI fetches it before writing.

Unlinked node creation, whole-graph import and branch creation do not accept a
caller revision. Node, project and import transactions compare TerminusDB data
versions when committing, so concurrent external writes cannot silently overwrite
one another. Branch creation asks TerminusDB to fork the source branch's head at
request processing time. There is no caller-selected historical commit or merge
operation in this CLI.

Graph constraints and supported block types are validated before commit. Linked
creation writes the new node and parent edge atomically. Batch dependency removal
resolves every target before mutation, so an unknown target cannot produce a
partial removal. Import validates the complete graph and environment, then
commits only into an empty destination. Existing UUIDs are never overwritten by
import.

Lean checks capture immutable source/dependency inputs, release the graph lock
while checking, and confirm current inputs before returning certification. A
changed Lean input yields a conflict. Saved editor evidence is bound to the exact
native document version and dependency environment; the browser cannot provide
its own successful verdict.

The local server rejects non-loopback Host and cross-origin browser requests.
CLI service tokens also detect port reuse after a restart. This protects a
trusted-local-author workflow; Lean execution is not an OS sandbox and the API
has no remote multi-user authentication. See [HTTP API](../../reference/http-api/)
for route coverage, status codes and request limits.
