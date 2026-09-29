---
title: HTTP API
---

One API process serves the editor, the document API and the collaboration API.
The CLI and the browser use the same endpoints.

## Authentication

Every `/api/…` and `/p/DB/BRANCH/api/…` request needs
`Authorization: Bearer TOKEN` with a token from `MDC_ACTORS`; otherwise `401`.
Document requests also pass the same-origin check
([Configuration](../configuration/#reverse-proxy)); a rejected Host or Origin
gets `403`. `GET /api/me` returns `{actor, admin}`.

Access to a branch follows the [workspace roles](../../concepts/workspaces/#workspaces-and-roles).
Without read access a branch answers `404 project not found`; with too low a
role, `403`.

## Directory

| Method and path | Result |
| --- | --- |
| `GET /api/status` | `{server: {running, port, url, on_demand: true}, projects}` where `projects` maps each readable `DB/BRANCH` to `{running: true, url, role}`. |
| `GET /api/projects` | The same, plus `nodes` and `edges` for branches already loaded. |
| `POST /api/projects` | A directory action, below. |

| Body | Permission | Effect |
| --- | --- | --- |
| `{"action": "init", "name": DB}` | admin | Create database `DB` with `main`; the caller owns it. |
| `{"action": "remove", "database": DB}` | admin | Delete the database, its history and its workspace record. |
| `{"action": "new_branch", "project": "DB/BRANCH", "name": NEW}` | editor+ on the source | Fork `DB/BRANCH` as `DB/NEW`, copying its role rules. |
| `{"action": "delete_branch", "project": "DB/BRANCH"}` | owner+ | Delete a non-`main` branch. |
| `{"action": "grant", "project": "DB/BRANCH", "actor": A, "role": R}` | owner+ | `R` is `owner`, `editor`, `viewer` or `none`. |
| `{"action": "set_owner", "database": DB, "owner": A}` | admin | Set the workspace owner; creates the workspace record for databases that lack one. |

Unknown fields are rejected. `start` and `stop` are answered with an error:
branches load on demand.

## Branch API

Paths are relative to `/p/DB/BRANCH/api`, for example
`/p/myproject/main/api/graph/check`. Node IDs in paths should be full UUIDs.
Permissions: `GET` needs read (history/export likewise), writes need `editor`,
import needs `owner`.

| Method and path | CLI | Result or behavior |
| --- | --- | --- |
| `GET /graph/check` | `graph check` | Counts and graph issues. |
| `GET /graph/roots` | `graph roots` | Roots with depth and component size. |
| `GET /graph/full` | `graph full` | Node summaries with `lean` (`no_code`/`unverified`) and index-pair edges. |
| `GET /search?q=TEXT&n=N` | `search` | Title/UUID matches; default and cap 200. |
| `GET /resolve?ref=TITLE_OR_UUID` | node commands | `{fnode, title}`. |
| `GET /node/ID/view` | `show` | `{node, referrers, children}`; see below. |
| `POST /node/new` | `new` | Body `{title, parent_fnode?}`; with a parent (guarded by its revision) the edge is added in the same commit and the parent is returned. |
| `DELETE /node/ID` | `del` | `{fnode, deleted: true, removed_edges}`. |
| `PUT /node/ID/title` | `rename` | Body `{title}`. |
| `PUT /node/ID/block/TYPE` | `edit` | Body `{content}`; TYPE is `text`, `lean`, `rocq` or `latex`. Block metadata is kept. |
| `DELETE /node/ID/block/TYPE` | `edit --delete` | Remove the block. |
| `POST /node/ID/dep/add` | `dep add` | Body `{dep_fnode}`. |
| `POST /node/ID/dep/rm` | `dep rm` | Body `{dep_fnodes: [...]}`; one commit. |
| `GET /node/ID/dep?mode=show\|refs\|leaf&depth=D` | `dep show/refs/leaf` | Reachable nodes with `depth`; D is -1 or nonnegative. |
| `GET /node/ID/dep/candidates?q=TEXT&n=N` | `dep candidates` | `{nodes, empty}`; default 50, cap 200. |
| `GET /node/ID/metric/ior` | `metric ior` | Degrees and IOR. |
| `GET /project/latex` | `project latex show` | `{revision, project}`. |
| `PUT /project/latex` | `project latex set` | `{preamble_name, preamble, bibliography_name, bibliography}`, guarded by the branch revision. |
| `GET /project/latex/catalog` | — | Citation and macro completions; `?known=KEY` returns `unchanged`. |
| `GET /node/ID/latex/context` | — | Imports and permitted labels; `?known=KEY` returns `unchanged`. |
| `POST /node/ID/latex/preview` | — | Body `{source}` (max 2 MiB); HTML, labels, diagnostics; nothing is saved. Needs read only. |
| `GET /project/lean`, `PUT /project/lean` | — | Legacy Lean project settings, preserved but unused. `PUT` is guarded by the branch revision. |
| `GET /export` | `export` | `{nodes, project, latex_project}`. |
| `POST /import` | `import` | A bundle into an empty branch; returns the graph report. |
| `GET /history` | `history` | The latest 50 commits. |
| `POST /branches` | — | Body `{name}`; fork this branch (as `new_branch`). |

A node in `view`, `show` and write responses has `fnode`, `title`, `depth`,
`revision`, `depens`, `blocks`, `module` and `formalization`:

```json
{"lean": "unverified", "rocq": "no_code",
 "lean_certification": {"status": "certified", "project": "mdc-…", "goal": "…",
   "minimum_trust": "audited", "checked_at": "2026-09-29T10:00:00.000Z", "truncated": false}}
```

`lean_certification` is `null` without a Lean block, `{"status": "not_submitted"}`
outside proof requests, and otherwise one of the
[statuses](../proofs/#statuses), with `reason`/`details` when rejected or stale.

### Revisions, errors and limits

Node writes (title, blocks, dependencies, deletion, linked creation) need a
quoted `If-Match: "REVISION"` with the node revision; `PUT /project/latex` and
`PUT /project/lean` use the branch revision from their `GET`. Missing: `428`;
stale: `412`. Unlinked creation, import and branch creation take no revision; all
writes still commit against the TerminusDB data version they read.

| Status | Meaning |
| --- | --- |
| 400 | Malformed query. |
| 401 | Missing or unknown token. |
| 403 | Rejected Host/Origin, or role too low. |
| 404 | Unknown node or path, or no read access to the branch. |
| 409 | Nonempty import destination, changed LaTeX context during preview, or a concurrent commit. |
| 412 | Stale revision. |
| 413 | LaTeX source too large. |
| 422 | Invalid body, graph, block type or project settings. |
| 428 | Missing `If-Match`. |
| 502 | TerminusDB unreachable or failed. |

Errors are `{"error": "message"}`. Request bodies are limited to 2,200,000 bytes
(this includes imports); larger or malformed JSON bodies are currently answered
with `500 internal error` rather than a specific status.

## Collaboration

Paths are under `/api/coordination`. Errors are `{"reason": CODE}`, plus
`issues` for invalid bodies and `retryable` for LeanGround failures.

| Method and path | Permission | Behavior |
| --- | --- | --- |
| `GET /environments/DB/BRANCH` | read | The proof environment, or `404 environment_not_configured`. |
| `PUT /environments/DB/BRANCH` | owner+ | Body `{base_key, context?, options?, minimum_trust?}`. |
| `GET /projects` | — | Proof requests you are a member of. |
| `GET /projects?database=DB&branch=BRANCH[&node=UUID]` | read | Those bound to the branch, or containing the node. |
| `POST /projects` | editor+ | Body `{document: {database, branch, node}, members?, budget, title?}`; header `Idempotency-Key`. `201` with the board; the same key and body again returns it with `200`. |
| `GET /projects/ID` | member, read | The board. |
| `POST /projects/ID/commands` | member, read | Headers `If-Match: "REVISION"`, `Idempotency-Key`. Returns `{revision, result, job}`. |
| `GET /projects/ID/jobs` | member, read | The latest 100 jobs. |
| `GET /projects/ID/history` | member, read | The latest 100 events. |
| `GET /projects/ID/facts/CERT_ID` | member, read | A certificate of this project, with source, from LeanGround. |
| `GET /projects/ID/nodes/UUID/draft` | member, read | Submission draft for a theorem node ([Agent interface](../agents/#3-get-the-draft)). |

Non-members and actors without read access on the bound branch get
`404 not_found`. Command bodies and their permissions are listed in
[Proofs with LeanGround](../proofs/#commands).

Some routes serve the archived prototype's projects created from a raw
proposition (admin-only `POST /projects` bodies without `document`, and
`GET /documents`, `/projects/ID/documents`, `/document-history`, `/branches`).
The editor does not use them.
