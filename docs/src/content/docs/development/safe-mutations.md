---
title: Versioned mutations
---

Every browser mutation requires a quoted If-Match node revision; missing revisions return 428 and stale revisions return 412. Database transactions additionally compare TerminusDB data versions, preventing races with external writers. Graph constraints are validated before a commit.

Node creation with a parent writes the new node and parent edge atomically. Import validates all nodes and the environment before publishing the bundle. Existing UUIDs are never overwritten by import.

Lean checks capture immutable source/dependency inputs, release the graph lock while checking, and confirm current inputs before returning certification. A concurrently changed Lean input yields a conflict. The local server rejects non-loopback Host and cross-origin browser requests.
