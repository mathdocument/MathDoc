---
title: Architecture
---

## Processes

`coordinator/src/main.ts` has three modes:

| Mode | Does |
| --- | --- |
| `migrate` | Creates or updates the PostgreSQL tables under an advisory lock. Needs only `MDC_DATABASE_URL`. |
| `serve` | One HTTP server: the editor, the document API, the collaboration API and the LaTeX renderers. |
| `worker` | A loop that schedules periodic syncs and runs queued jobs against LeanGround and TerminusDB. |

Several workers may run; jobs are claimed under a PostgreSQL lock, at most one
job per project runs at a time, and a running job renews its claim so a crashed
worker's job is picked up again. The worker keeps no state of its own.

## Source layout

| File | Responsibility |
| --- | --- |
| `config.ts` | Environment configuration ([reference](../../reference/configuration/)). |
| `http.ts` | Authentication, `/api/me`, the collaboration routes, proof environments, document-bound project creation, drafts, and the rule that every project route rechecks document read access. |
| `docs/routes.ts` | Document front: same-origin check, workspace authorization, directory actions, per-branch services loaded on demand, static files of `web/dist`. |
| `docs/service.ts` | Per-branch document API: graph queries, node writes with revision checks, import/export, LaTeX routes. |
| `docs/graph.ts`, `docs/model.ts`, `docs/terminus.ts` | In-memory snapshot and validation, the node/project data model, TerminusDB HTTP client (schema, commits with data-version guards, branches, history). |
| `docs/workspace.ts` | Workspace records and the permission matrix. |
| `docs/latex.ts` | One lazy Python renderer process per branch (JSON lines, bounded concurrency, 10 s timeout); installs the pinned runtime when `MDC_LATEX_PYTHON` is unset. |
| `domain.ts` | The project board and all commands as pure state transitions: goals, facts, tasks, attempts, budgets, route selection. |
| `store.ts` | PostgreSQL: boards, idempotent command log, events, job queue. |
| `worker.ts` | Jobs: `provision` (LeanGround project and binding), `submit_node`, `resolve`, `submit`, `import`, `sync`, `assemble`, `writeback`. |
| `lean-source.ts` | Conservative Lean scanner: top-level commands, context lines, declarations, node classification. |
| `proof.ts` | Proof environments, node conversion and submission drafts, proof-request contracts. |
| `writeback.ts` | Writeback batches. |
| `certification.ts` | The per-node certification status shown in document views. |
| `contracts.ts` | Versioned contracts (`coordinator/contracts/v1/`) and their schemas. |
| `remote.ts` | LeanGround HTTP client. |
| `cli.ts` | The `mdc` client. |

## Requests

The document front handles `/`, static files, `/api/status`, `/api/projects` and
`/p/DB/BRANCH/…`. For API paths it checks Host/Origin, authenticates the token,
maps the route to a workspace action and authorizes it, then calls the branch's
`BranchService`. Everything else under `/api/` goes to the collaboration handler.

A `BranchService` is created on the first request to a branch and kept. Each
request takes the branch lock, looks up the current TerminusDB commit and
reloads the snapshot only if another writer moved the branch; the service's own
writes update the snapshot in place. The snapshot keeps nodes with full source
blocks, reverse edges and depths, so memory and reload time grow with total
source size. LaTeX rendering runs outside the lock and rechecks its inputs before
answering. Document views attach each node's certification, computed from the
proof requests bound to the branch.

## Collaboration state

A project board is one JSON document per project in PostgreSQL. A command runs in
one transaction: lock the board, replay the stored response if the
(actor, idempotency key) pair was seen, check the revision, apply the pure
transition from `domain.ts`, enqueue a job if needed, bump the revision and log an
event. Goal statuses are recomputed from facts; route selection considers only
active facts.

Jobs talk to LeanGround's fact API. Their results are written back into the board
with a checkpoint that rechecks the job's claim. Periodic sync jobs are enqueued
for ready projects whose last sync is older than 15 seconds.

## Data compatibility

The TerminusDB schema is unchanged from the earlier Rust application, so existing
databases open as they are. Legacy fields (node `module`, the Lean project
document) are preserved. New data (workspaces, environments, projects) lives only
in PostgreSQL. Databases without a workspace record are visible to administrators
until adopted with `set_owner`.
