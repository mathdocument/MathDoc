---
title: HTTP API
---

One API process serves the editor, the document API and the collaboration API.
The CLI and the browser use the same endpoints. `GET /` and `GET /p/DB/BRANCH/…`
outside `/p/DB/BRANCH/api/` serve the editor's files and need no token
(`/p/DB/BRANCH` redirects to `/p/DB/BRANCH/`; other methods get `405`).

## Authentication

Every `/api/…` and `/p/DB/BRANCH/api/…` request needs
`Authorization: Bearer TOKEN` with a token from `MDC_ACTORS` or a live
[delegated token](../../concepts/workspaces/#delegated-tokens); otherwise `401`.
Document requests also pass the same-origin check
([Configuration](../configuration/#reverse-proxy)); a rejected Host or Origin
gets `403`. `GET /api/me` returns `{actor, principal, admin, scopes}`; `scopes` is
`null` for one's own token.

| Method and path | Result |
| --- | --- |
| `GET /api/tokens` | Your delegated tokens, revoked ones included: `name`, `actor`, `scopes`, `created_at`, `expires_at`, `revoked_at`. |
| `POST /api/tokens` | Body `{name, scopes?, expires_at?}`: `name` is 1–64 letters, digits, `_` or `-`; `scopes` is a subset of `work`, `maintain`, `edit` (default `["work"]`); `expires_at` an ISO time with offset, in the future (`400 token_expired` otherwise). `201 {token, actor, name, scopes, expires_at}`; the token is not shown again. `409 token_name_taken` while a token of that name is not revoked. |
| `DELETE /api/tokens/NAME` | Revoke it: `{name, actor, revoked_at}`; `404 not_found` if there is no live token of that name. |

These need one's own token (`403 own_token_required`); other methods get
`405 method_not_allowed`. A delegated token whose
scopes do not allow an action gets `403 token_scope_required` on collaboration
routes and `403` naming the scope on document routes.

Access to a branch follows the [workspace roles](../../concepts/workspaces/#workspaces-and-roles).
Without read access a branch answers `404 project not found`; with too low a
role, `403`.

## Directory

| Method and path | Result |
| --- | --- |
| `GET /api/status` | `{server: {running, port, url, on_demand: true}, projects}` where `projects` maps each readable `DB/BRANCH` to `{running: true, url, role}`. |
| `GET /api/projects` | The same, plus `nodes` and `edges` for branches already loaded. |
| `POST /api/projects` | A directory action, below. |

| Body | Permission | Effect and reply |
| --- | --- | --- |
| `{"action": "init", "name": DB}` | admin | Create database `DB` with `main`; the caller owns it. `{project, initialized: true}`. |
| `{"action": "remove", "database": DB}` | admin | Delete the database, its history and its workspace record. `{database, deleted: true}`. |
| `{"action": "new_branch", "project": "DB/BRANCH", "name": NEW}` | editor+ on the source | Fork `DB/BRANCH` as `DB/NEW`, copying its role rules. `{project, created: true}`. |
| `{"action": "delete_branch", "project": "DB/BRANCH"}` | owner+ | Delete a non-`main` branch. `{project, deleted: true}`. |
| `{"action": "grant", "project": "DB/BRANCH", "actor": A, "role": R}` | owner+ | `R` is `owner`, `editor`, `viewer` or `none`; `A` must be a name in `MDC_ACTORS`. `{project, actor, role}`. |
| `{"action": "set_owner", "database": DB, "owner": A}` | admin | Set the workspace owner (a name in `MDC_ACTORS`); creates the workspace record for databases that lack one. `{database, owner}`. |

Unknown fields are rejected (`422`). `start` and `stop` are answered with an
error: branches load on demand. A delegated token needs the `edit` scope for
`new_branch` and `maintain` for the other actions.

## Branch API

Paths are relative to `/p/DB/BRANCH/api`, for example
`/p/myproject/main/api/graph/check`. Node IDs in paths should be full UUIDs.
Permissions: `GET` and the LaTeX preview need read (history/export likewise),
other writes and `POST /branches` need `editor`, import needs `owner`. A delegated
token needs the `edit` scope for every route that does not only read.

| Method and path | CLI | Result or behavior |
| --- | --- | --- |
| `GET /graph/check` | `graph check` | `{nodes, edges}`: node and edge counts. |
| `GET /graph/roots` | `graph roots` | Unreferenced nodes, deepest first: `fnode`, `title`, `depth`, `topo_depth` (equal to `depth`), `component_size`. |
| `GET /graph/full` | `graph full` | Node summaries with `lean` (`no_code`/`unverified`), `lean_role` and `certification` (the status of `lean_certification`, or `null`), and index-pair edges. |
| `GET /search?q=TEXT&n=N` | `search` | Title/UUID matches as `{fnode, title, depth}`; default and cap 200. |
| `GET /resolve?ref=TITLE_OR_UUID` | node commands | `{fnode, title}`; `404` if nothing matches, `422` for a title shared by several nodes. |
| `GET /node/ID/view` | `show` | `{node, referrers, children}`; see below. Referrers and children have `fnode`, `title`, `depth` and `formalization`. |
| `POST /node/new` | `new` | Body `{title, parent_fnode?}`; with a parent (guarded by its revision) the edge is added in the same commit and the parent is returned. |
| `DELETE /node/ID` | `del` | `{fnode, deleted: true, removed_edges}`. |
| `PUT /node/ID/title` | `rename` | Body `{title}`. |
| `PUT /node/ID/block/TYPE` | `edit` | Body `{content}`; TYPE is `text`, `lean`, `rocq` or `latex`. Block metadata is kept. |
| `DELETE /node/ID/block/TYPE` | `edit --delete` | Remove the block. |
| `POST /node/ID/dep/add` | `dep add` | Body `{dep_fnode}`; returns the source node. |
| `POST /node/ID/dep/rm` | `dep rm` | Body `{dep_fnodes: [...]}`; one commit; returns the source node. |
| `GET /node/ID/dep?mode=show\|refs\|leaf&depth=D` | `dep show/refs/leaf` | Reachable nodes as `{fnode, title, depth}`, `depth` being the distance; both parameters required, D is -1 (unlimited) or nonnegative. |
| `GET /node/ID/dep/candidates?q=TEXT&n=N` | `dep candidates` | `{nodes, empty}`; default 50, cap 200. `empty` is `null` when `nodes` is not, else `{kind: "result_limit", available}`, `{kind: "excluded", source, existing_dependencies}` or `{kind: "no_match"}`. |
| `GET /node/ID/metric/ior` | `metric ior` | `{fnode, in_degree, out_degree, ior}` ([Graph and metrics](../graph-and-metrics/)). |
| `GET /project/latex` | `project latex show` | `{revision, project}`. |
| `PUT /project/latex` | `project latex set` | `{preamble_name, preamble, bibliography_name, bibliography}`, guarded by the branch revision. Names are `.tex`/`.cls` and `.bib` without paths; preamble at most 2 MiB, bibliography 16 MiB. |
| `GET /project/latex/catalog` | — | Citation and macro completions with `project_key`; `?known=KEY` returns `{unchanged: true}` while the key matches. |
| `GET /node/ID/latex/context` | — | Imports and permitted labels with `context_key`; `?known=KEY` returns `{unchanged: true}` while the key matches. |
| `POST /node/ID/latex/preview` | — | Body `{source}` (max 2 MiB); HTML, labels, diagnostics; nothing is saved. Needs read only. |
| `GET /project/lean`, `PUT /project/lean` | — | Legacy Lean project settings, preserved but unused. `PUT` is guarded by the branch revision. |
| `GET /export` | `export` | `{nodes, project, latex_project}`. |
| `POST /import` | `import` | Body `{nodes, project, latex_project?}` into an empty branch; returns `{nodes, edges}`. |
| `GET /history` | `history` | The latest 50 TerminusDB commits. |
| `POST /branches` | — | Body `{name}`; fork this branch (as `new_branch`); returns TerminusDB's reply. |

A node in `view`, `show` and write responses has `fnode`, `title`, `depth`,
`revision`, `depens`, `blocks`, `module` and `formalization`:

```json
{"lean": "unverified", "rocq": "no_code",
 "lean_certification": {"status": "certified", "project": "mdc-…", "goal": "…",
   "minimum_trust": "audited", "checked_at": "2026-09-29T10:00:00.000Z", "truncated": false}}
```

`lean_certification` is `null` without a Lean block, `{"status": "not_submitted"}`
when no project conversion applies to the node, and otherwise one of the
[statuses](../proofs/#statuses), with `branch` (whose conversion is shown),
`reason`/`details` when rejected or stale, and `definition_ids` when registered.
A definition counts as registered also after an accepted writeback, which marks
every node it wrote `certified`. `lean_role` is `definition` or `theorem`: as the
shown conversion binds the node, else as its Lean block declares it (its
`lean_role` metadata, else `theorem` when it states a theorem or lemma, else
`definition` when it defines something); `null` without a Lean block or when the
scanner cannot read it.

### Revisions, errors and limits

Node writes (title, blocks, dependencies, deletion, linked creation) need a
quoted `If-Match: "REVISION"` with the node revision; `PUT /project/latex` and
`PUT /project/lean` use the branch revision from their `GET`. Missing: `428`;
stale: `412`. Write replies with a node carry its revision as `ETag: "REVISION"`.
Unlinked creation, import and branch creation take no revision; all writes still
commit against the TerminusDB data version they read.

| Status | Meaning |
| --- | --- |
| 400 | Malformed query or malformed JSON body. |
| 401 | Missing or unknown token. |
| 403 | Rejected Host/Origin, role too low, or a delegated token without the scope. |
| 404 | Unknown node or path, or no read access to the branch. |
| 409 | Nonempty import destination, changed LaTeX context during preview, or a concurrent commit. |
| 412 | Stale revision. |
| 413 | Request body or LaTeX preview source too large. |
| 422 | Invalid body, name, graph, block type or project settings, or an ambiguous title. |
| 428 | Missing `If-Match`. |
| 500 | Unexpected failure (`internal error`). |
| 502 | TerminusDB unreachable or failed. |

Errors are `{"error": "message"}`. Request bodies of branch routes are limited to
256 MiB (whole-graph imports); `/api/projects` and `POST /branches` bodies to
2,200,000 bytes. A larger body gets `413`, malformed JSON `400`; an empty body
reads as `{}`.

## Collaboration

Paths are under `/api/coordination`. Errors are `{"reason": CODE}`, plus
`issues` for invalid bodies (`400 invalid_request`) and `retryable` for
LeanGround failures. A refused document permission check answers
`{"reason": "not_found" | "document_permission_denied" | "document_error", "error": MESSAGE}`.
Without TerminusDB configured, routes that check document roles answer
`503 documents_not_configured`.

| Method and path | Permission | Behavior |
| --- | --- | --- |
| `GET /environments/DB` | read on `main` | The database's proof environment, or `404 environment_not_configured`. |
| `PUT /environments/DB` | owner+ on `main` | Body `{base_key, context?, options?, minimum_trust?, mode?}` (`minimum_trust` is `claimed`, `audited` or `trusted`, default `audited`; `mode` is `declarations`, the default, or `modules`, which takes no `context` or `options`: [Module projects](../proofs/#module-projects)). A changed environment supersedes the current project board. |
| `GET /projects` | member | Boards you are a member of whose home branch you can read. |
| `GET /projects?database=DB&branch=BRANCH[&node=UUID]` | read | The database's project boards (current first, then superseded), then the branch's per-branch proof requests of earlier versions; with `node`, those that converted it on that branch. |
| `POST /projects` | editor+ on the branch | Body `{document: {database, branch, node}, members?, budget, title?}`; header `Idempotency-Key`. Creates the database's project rooted at `node` (`201`, owner+ on `main`) or attaches the branch to it (`200`, members only); the same key and body again returns the first outcome ([Submitting a branch](../proofs/#submitting-a-branch)). |
| `GET /projects/ID` | member, read | The board, without the branches you cannot read. |
| `POST /projects/ID/commands` | member, read | Headers `If-Match: "REVISION"` (or `*` for lease and job commands, [Commands](../proofs/#commands)), `Idempotency-Key`. Returns `{revision, result, job}`. |
| `GET /projects/ID/runs/RUN/preview` | member, read (and read on the run's branch) | What accepting a certified assembly writes, computed as the writeback job computes it and not committed: `{status: "ready", assembled, version, changes}` with each change's `kind`, `node`, `title`, `before` (the node's Lean text now, `null` for a new node) and `after`; `{status: "conflict", reason, details}` when the batch would abort; `{status: "written", writeback}` once it is committed. `409 not_certified` for another run. |
| `POST /projects/ID/check` | member, `work` scope | Body `{source}`, a whole Lean file (at most 2 MB). LeanGround checks it on the project's base and MathDoc answers `{ok, stage?, diagnostics, axioms, trust, missing_imports, ms}` (`mdc check`). Nothing is recorded; `409 environment_not_configured` without a proof environment. In a module project the body is `{files, shared?, goal?, root?}` and the answer is LeanGround's: `{ok, compiled, diagnostics}` (`file`, `line`, `column`, `severity`, `message`), and with a goal also `violations`, `premises`, `placeholders` and `axioms`. |
| `GET /projects/ID/search?q=QUERY[&q=…][&kind=K&limit=N]` | member, read | `{hits}`: declarations of the project's base whose names contain one of 1–10 queries (`kind` `theorem`, `definition` or `all`, default `theorem`; `limit` 1–100, default 20), each with `name`, `kind`, `type`, `trust` and `matched_query` (`mdc lemma`). |
| `GET /projects/ID/next[?branch=BRANCH&limit=N]` | member, read (and read on `BRANCH`) | `{revision, candidates}`: free goals worth working on, best first (default 20, at most 500); each with `goal`, `task`, `score`, `branches` needing it, `failures`, `statement` and `negates` ([What to do next](../../concepts/collaboration/#what-to-do-next)). |
| `GET /projects/ID/goals[?status=S&branch=BRANCH]` | member, read | `{revision, goals}`: one entry per goal of a project board, the root first (`mdc goals`): `goal`, `name`, `statement`, `status`, `refuted`, `negates`, `root`, `needed_by` (branches whose routes reach it), `certificates` (count by state), `task` (its open prove task, with `holder` and `expires` while leased), and the counts `notes` and `failures`. `status` filters by `insufficient`, `derivable`, `certified` or `refuted`; `branch` keeps the goals that branch's route reaches (`404` for a branch you cannot read). |
| `GET /projects/ID/goals/KEY` | member, read | One goal in full (`mdc goal`): statement, `proposition`, `declaration`, `definitions`, `status`, `refuted`, `negates`/`negated_by`, `needed_by`, `picks` (branch → certificate), `certificates` (each with `state`, `premises`, `picked_by`, `description`, `submitter`, `review`, `retired_reason`, `refuted_by`), `premise_of`, `tasks` with their `attempts`, `failures` and `notes`. |
| `GET /projects/ID/decompositions[?state=S]` | member, read | `{revision, decompositions}`: certificates with premises (in state S if given), oldest submission first, each with its `goal`, premises, description, submitter, review and the branches needing the goal (`mdc review`). |
| `GET /projects/ID/jobs` | member, read | The latest 100 jobs, newest first: `id`, `actor`, `kind`, `status`, `tries`, `error`, `details`, `result`, `created_at`. |
| `GET /projects/ID/jobs/JOB_ID` | member, read | One job: `kind`, `status` (`queued`, `running`, `done`, `failed`), `tries`, `error` with `details` (LeanGround's explanation of a refusal, such as Lean's errors; at most 8000 characters) and, when done, `result`. A `submit` job's result is `{certificate, goal, mode, premises, state}` (`state`: the certificate's state on the board, e.g. `proposed`); each premise has `goal`, its placeholder `name`, `new` (not on the board before) and `statement` (`resolved`, `known`, `identity_mismatch`, `placeholder_not_found` or a LeanGround reason). A `submit_modules` job's result is the same, with `statement` `resolved` (LeanGround returned the premise's identity) or `identity_missing`, and `shared` when it registered a shared module. |
| `GET /projects/ID/history` | member, read | The latest 100 events, newest first: `sequence`, `actor`, `kind`, `revision`, `created_at`. |
| `GET /projects/ID/facts/CERT_ID` | member, read | A certificate of this project, with source, from LeanGround, and `definition_sources`: each definition it uses with `id`, `name` and `source` (definitions registered with a sketch are not in its source). |
| `GET /projects/ID/nodes/UUID/draft?branch=BRANCH` | member, read on the branch | Submission draft for a theorem node of an attached branch ([Agent interface](../agents/#3-get-the-draft)); `branch` is required for a project board (`400 branch_required`). |
| `GET /projects/ID/goals/KEY/draft?branch=BRANCH[&with=KEY,…]` | member, read (and read on `BRANCH`) | In a module project: `{format: "mdc-module-draft/1", statement: {const, from: "node"\|"decomposition", node?, certificate?, files}, shared: [{id, modules, files}], base, environment_id, revision, notes, task}` (`409 statement_changed` when no attached branch has the root's text as converted). Otherwise the material of a goal file (`mdc pull`) for any goal with a statement: `root` (`name`, `binders`, `type`, `definitions`, and `from`: `node` for an unchanged converted node of `BRANCH`, `declaration` for the declaration recorded on the board, `proposition` for a bare resolved proposition), available `premises` (the node's premise nodes, then the `with` goals), `definitions` (`id`, `name`, `source`, dependencies first), `root_module`, `context`, `options`, `environment_id`, `revision`, `notes` (the latest 20, newest first) and the goal's prove `task` with its lease. At most 100 `with` goals (`400 too_many_premises`); `404 unknown_goal`; `409 goal_unresolved` for a goal without a statement. |

For a project board, "read" means read access on the database's `main` branch;
for a per-branch proof request, on its branch. Others get `404 not_found`.
Command bodies and their permissions are listed in
[Proofs with LeanGround](../proofs/#commands).

Some routes serve projects created from a raw
proposition: admin-only `POST /projects` bodies without `document` and
`GET /documents?database=DB&branch=BRANCH`; `GET` and `PUT /projects/ID/documents`,
`GET /projects/ID/document-history` and `POST /projects/ID/branches` (`PUT` and
`POST` for the owner). The editor does not use them.

`/api/me` and `/api/tokens` are also served under `/api/coordination/`.
