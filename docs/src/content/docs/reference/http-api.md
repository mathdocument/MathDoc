---
title: HTTP API and CLI mapping
---

The shared entry serves inventory at `GET /api/status`, with the same
`{server, projects}` result as [`mdc status`](../workspace-commands/).
The directory uses `GET /api/projects`: the same inventory plus `nodes` and
`edges` on running branches, read from their loaded graph snapshots. Stopped
branches have no counts and are never loaded just to display the directory.
Branch endpoints use `/p/DATABASE/BRANCH/api`, for example
`http://127.0.0.1:17843/p/myproject/main/api/graph/check`.
Tables below give paths relative to that prefix. The browser and CLI use the
same handlers. Node path IDs should be complete
UUIDs; CLI references are resolved by exact name or UUID first.

## Graph and authoring

| Method and path (after `/api`) | CLI | Result or behavior |
| --- | --- | --- |
| `GET /graph/check` | `graph check` | Node/edge counts and graph issues. |
| `GET /graph/roots` | `graph roots` | Root summaries with depth and component size. |
| `GET /graph/full` | `graph full` | Node summaries with `lean` status (`no_code`, `unverified`, `verified`) and index-pair edges. |
| `GET /search?q=TEXT&n=N` | `search TEXT -n N` | Title/UUID matches; default/cap 200. |
| `GET /resolve?ref=NAME_OR_UUID` | Used by node commands | `{fnode, title}`. |
| `GET /node/ID/view` | `show` and `dep` reads | `{node, referrers, children}`; all include `formalization: {lean, rocq}` status. CLI `show` returns `node`. |
| `POST /node/new` | `new -t TITLE [--parent REF]` | Body `{title, parent_fnode?}`; optionally creates an edge atomically. |
| `DELETE /node/ID` | `del SOURCE` | Atomically delete the node and detach all referrers; `{fnode, deleted: true, removed_edges}`. |
| `PUT /node/ID/title` | `rename` | Body `{title}`; updated node. |
| `PUT /node/ID/block/TYPE` | `edit --type TYPE` | Body `{content}`; updated node. |
| `DELETE /node/ID/block/TYPE` | `edit --type TYPE --delete` | Delete text/lean/rocq/latex block; updated node. |
| `POST /node/ID/dep/add` | `dep add -t TARGET` | Body `{dep_fnode}`; updated source node. |
| `POST /node/ID/dep/rm` | `dep rm -t TARGET...` | Body `{dep_fnodes:[...]}`; one atomic removal. |
| `GET /node/ID/dep?mode=MODE&depth=D` | `dep show`, `refs`, `leaf` | MODE is show/refs/leaf; depth -1 or nonnegative. |
| `GET /node/ID/dep/candidates?q=TEXT&n=N` | `dep candidates` | `{nodes, empty}`; HTTP default 50, cap 200; CLI explicitly defaults to 200. |
| `GET /node/ID/metric/ior` | `metric ior` | Degrees and IOR value. |
| `POST /node/ID/lean/check` | `lean check [--build]` | Body `{build:bool}`; certification and cache result. |
| `POST /node/ID/lean/goals` | `lean goals` | Body `{line, character}` in zero-based LSP coordinates. |
| `GET /project/lean` | `project show` | `{revision, project}`. |
| `PUT /project/lean` | `project set` | Complete project object; updated revision and configuration. |
| `GET /project/latex` | `project latex show` | `{revision, project}` with the shared macro file and bibliography. |
| `PUT /project/latex` | `project latex set` | Replace `{preamble_name, preamble, bibliography_name, bibliography}` using the branch revision. |
| `GET /project/latex/catalog` | — | Shared citations and macro completions; `?known=PROJECT_KEY` returns `unchanged` when current. |
| `GET /node/:id/latex/context` | — | Direct imports and permitted labels; `?known=CONTEXT_KEY` avoids repeating unchanged context. |
| `POST /node/:id/latex/preview` | — | Render `{source}` as HTML and return labels and diagnostics, without saving the draft. |
| `GET /export` | `export` | `{nodes, project}` for the entire branch snapshot. |
| `POST /import` | `import FILE` | Complete bundle into an empty branch; graph report. |
| `GET /history` | `history` | Latest 50 TerminusDB commits. |
| `POST /branches` | `branch new NAME` | Body `{name}`; forks the current head. |

For linked creation, the HTTP response is the updated parent, as used by the
browser. The CLI identifies its newly added dependency and returns the created
node, keeping `new` output consistent. A normal unlinked creation returns the
new node directly. Deleting a node retains all other nodes and their source
blocks; only dependency edges incident to the deleted node are removed.

## Local management and editor sessions

`init`, `status`, and `branch del` contact TerminusDB directly. `start` owns its
background process launch. Loading a branch calls
`POST /p/DATABASE/BRANCH/api/service/start` with the server's local token.
`stop DATABASE/BRANCH` calls `POST /p/DATABASE/BRANCH/api/service/stop` with that
branch's token; bare `stop` calls `POST /api/service/stop` with the server token.
Stop waits for the relevant cache lease to be released. All calls use the same
listening port; no branch HTTP process or forwarding hop exists. None of these
operations require a hidden CLI command.

`remove DATABASE` calls `POST /api/service/remove/DATABASE` with the server token
when the entry server is running. Removal excludes concurrent lifecycle changes,
stops all of this project's branches, then deletes the database and clears its
local caches. With no server, the CLI holds the entry server lease and performs
the same deletion directly. It also removes this project's foreground restore
entries. The result is `{database, deleted: true}`.

The project directory sends JSON to `POST /api/projects`. This endpoint requires
a same-origin browser `Origin` header and supports these bodies:

| Body | Behavior |
| --- | --- |
| `{action: "init", name}` | Create a database and its stopped `main` branch. |
| `{action: "remove", database}` | Stop every branch of this project and delete its database, history and local branch caches, as with `mdc remove DATABASE`. |
| `{action: "start", project: "DATABASE/BRANCH"}` | Load a branch into the existing entry server. |
| `{action: "stop", project}` | Stop that branch, including its editor sessions. |
| `{action: "new_branch", project, name}` | Fork the specified branch's current head, including a stopped source. The new branch is stopped. |
| `{action: "delete_branch", project}` | Delete an already stopped non-`main` branch and its compiler caches, using the same exclusive cache lease as CLI deletion. |

Browser management does not disclose or replace the private CLI service tokens.
TerminusDB protects `main`: neither the browser nor CLI can delete it, regardless
of how many branches exist. Rejected `main` deletions leave its caches intact.
Deleting another branch preserves the database and immutable history.

The following endpoints maintain browser draft sessions rather than saved graph
objects. CLI agents use `lean check` and `lean goals` instead of managing sessions.

| Method and path (after `/api`) | Purpose |
| --- | --- |
| `POST /node/ID/lean/session` | Create a native editor session for the guarded node revision. |
| `GET /lean/session/SESSION` | Read editor session metadata and source. |
| `GET /lean/session/SESSION/ws` | WebSocket carrying native Lean JSON-RPC and mdc editor messages. |
| `DELETE /lean/session/SESSION` | Close the session and release its slot. |

Each browser session keeps one connection, isolated drafts and two warm document
workers. The branch currently allows eight browser sessions. See
[Compiler internals](../../development/compiler-internals/) for native evidence,
certification and cleanup.

## Revisions, errors and limits

Node mutations, linked creation, Lean checks/goals and editor creation require a
quoted `If-Match` node revision. `PUT /project/lean` instead uses the branch data
revision returned by `GET /project/lean`. Unlinked creation, import and branch
creation have no caller-supplied `If-Match`. Node, project and import transactions
still use current TerminusDB data-version guards. Branch creation asks TerminusDB
to fork the origin branch head when it processes the request. Import validates
the whole bundle before one commit and rejects nonempty destinations.

| HTTP status | Meaning |
| --- | --- |
| 400 | Malformed query or request syntax. |
| 403 | Rejected Host/origin or missing stop token. |
| 404 | Unknown node, session or API path. |
| 409 | Transaction conflict, changed service token, changed Lean inputs, or nonempty import destination. |
| 412 | Caller supplied a stale revision. |
| 413 | Request body exceeds the configured limit. |
| 415 | JSON request has an unsupported content type. |
| 422 | Invalid graph, source type, project configuration or request body. |
| 428 | Missing quoted If-Match where required. |
| 502 | TerminusDB request failure. |
| 503 | Requested branch is stopped, starting or stopping. |

Application errors use `{error: string}`; framework rejections, such as malformed
JSON or oversized bodies, may be plain text. Check the status before decoding.
The HTTP body limit is 256 MiB, including graph imports. A larger exported bundle
cannot currently be restored through this API.
There is no streaming import/export or pagination over the full graph. The
service holds the complete source snapshot in memory, so startup and external
commit reload costs grow with source size.

Host and browser-origin checks restrict requests to loopback or the configured
`public_origin`. See [reverse proxy configuration](../configuration/). CLI
requests carry `x-mdc-service` from the private local service record, preventing
a reloaded branch or reused port from silently accepting a stale CLI command.
The server selects the current branch router directly, including native
WebSocket upgrades, without reconstructing HTTP requests.
This is a trusted-author deployment; remote user authentication, per-project
permissions and compiler sandboxing are not provided by MathDoc.
