---
title: Architecture
---

## Processes

`coordinator/src/main.ts` has three modes:

| Mode | Does |
| --- | --- |
| `migrate` | Creates or updates the PostgreSQL tables under an advisory lock. Needs only `MDC_DATABASE_URL` (and `MDC_DATABASE_PASSWORD` if the password is kept apart). |
| `serve` | One HTTP server: the editor, the document API, the collaboration API and the LaTeX renderers. |
| `worker` | A loop that schedules periodic syncs and runs queued jobs against LeanGround and TerminusDB, one job at a time. |

Several workers may run; jobs are claimed under a PostgreSQL lock, at most one
job per project runs at a time, and a running job renews its claim so a crashed
worker's job is picked up again. The worker keeps no state of its own.

## Source layout

Paths are under `coordinator/src/`.

| File | Responsibility |
| --- | --- |
| `main.ts` | Entry point for the three modes. |
| `config.ts` | Environment configuration, including `*_FILE` secrets ([reference](../../reference/configuration/)). |
| `identity.ts` | Access tokens: own tokens from `MDC_ACTORS`, delegated tokens (`person/name`, scopes `work`, `maintain`, `edit`; only their SHA-256 is stored, in `mdc_token`), and the scopes each command needs. |
| `http.ts` | `/api/me`, `/api/tokens` and the collaboration API under `/api/coordination/`: proof environments, document-bound project creation, commands, jobs, history, goal and decomposition views, `next`, node and goal drafts. Every project route rechecks document read access and hides branches the caller cannot read. |
| `docs/routes.ts` | Document front: same-origin check, authentication, delegated-token scopes, workspace authorization, directory actions, per-branch services loaded on demand, static files of `web/dist`. |
| `docs/service.ts` | Per-branch document API: graph queries, node writes with revision checks, import/export, LaTeX routes. |
| `docs/graph.ts`, `docs/model.ts`, `docs/terminus.ts` | In-memory snapshot and validation, the node/project data model and node revisions, TerminusDB HTTP client (schema, commits with data-version guards, branches, history; a transaction TerminusDB rolled back after exhausting its own retries is resent up to three times). |
| `docs/names.ts`, `docs/toml.ts` | Name and identifier rules and canonical JSON; a fail-closed TOML subset reader for the legacy Lake project file. |
| `docs/workspace.ts` | Workspace records and the permission matrix. |
| `docs/latex.ts` | One lazy Python renderer process per branch (JSON lines, bounded concurrency, 10 s timeout); installs the pinned runtime when `MDC_LATEX_PYTHON` is unset. |
| `domain.ts` | The project board and all commands as pure state transitions: goals, facts, tasks, attempts, leases, budgets, review, notes, refutation, route selection and the `next` ranking. |
| `goal-view.ts` | Read views behind `mdc goals`, `mdc goal` and `mdc review`, computed from the part of the board the caller may read. |
| `goal-draft.ts` | Goal drafts: what `mdc pull` writes into a goal file, for any goal of a project board. |
| `store.ts` | PostgreSQL: migrations, board documents and their `meta`, task and attempt rows, the idempotent command log, events, the job queue (claims, checkpoints, backoff), project creation, branch attachment and superseding. |
| `worker.ts` | Jobs: `provision` (LeanGround project, then the branches submitted so far), `attach` (convert one branch), `submit_node`, `resolve`, `submit`, `negate` (register a goal's negation), `import`, `sync`, `assemble`, `writeback`, and `accept` (statement boards, see `documents.ts`). |
| `remote.ts` | LeanGround HTTP client: calls that run Lean (`resolve`, `submit`, `define`, `assemble`) wait up to `LEANGROUND_TIMEOUT_S` (default 960 s), others 30 s; timeouts, unreachable servers, `5xx` and `429` are retryable. |
| `documents.ts` | Small TerminusDB node adapter for boards an administrator created from a statement with an optional `node_ref` (no database binding): load, guarded write, history, branch, `accept`. |
| `lean-source.ts` | Conservative Lean scanner: top-level commands, context lines, declarations, node classification. |
| `proof.ts` | Proof environments, node conversion and submission drafts, proof-request contracts. |
| `writeback.ts` | Writeback batches. |
| `certification.ts` | The per-node certification status shown in document views. |
| `contracts.ts` | Zod schemas of the four v1 contracts. `coordinator/scripts/contract-schemas.ts` derives their JSON Schemas; the contracts test checks they equal `coordinator/contracts/v1/`. |
| `cli.ts` | The `mdc` client: argument parsing and dispatch; document and board commands are one or two HTTP requests. |
| `mdc/client.ts` | One HTTP request with the caller's token; failures carry a stable `reason`. |
| `mdc/goal-file.ts`, `mdc/workspace.ts` | The goal file format `mdc-goal-file/1`; the `.mdc/` records (goal, lease, submissions) next to goal files. |
| `mdc/verbs.ts` | `pull`, `prove`, `split`, `done`, `note`, `refute`, `next`: claims, submissions, lease renewal. |
| `mdc/maintain.ts` | `goals`, `goal`, `review`, `pick`, `branch new --pick`, `proof command`; replay records in `MDC_STATE_DIR`. |
| `mdc/mcp.ts` | `mdc mcp`: the work commands as Model Context Protocol tools over stdio. |

## Requests

The document front handles `/`, static files, `/api/status`, `/api/projects` and
`/p/DB/BRANCH/…`. For API paths it checks Host/Origin, authenticates the token,
maps the route to a workspace action and authorizes it (writes also need a
delegated token's `edit` scope), then calls the branch's `BranchService`.
Everything else under `/api/` goes to `http.ts`, which serves `/api/me`,
`/api/tokens` and `/api/coordination/…`.

A `BranchService` is created on the first request to a branch and kept. Each
request takes the branch lock, looks up the current TerminusDB commit and
reloads the snapshot only if another writer moved the branch; the service's own
writes update the snapshot in place. The snapshot keeps nodes with full source
blocks, reverse edges and depths, so memory and reload time grow with total
source size. LaTeX rendering runs outside the lock and rechecks its inputs before
answering. Document views attach each node's certification, computed from the
database's project boards (and earlier per-branch proof requests).

## Collaboration state

A project board is a row of `mdc_project`: the board document (JSONB) and its
`meta`, the few fields that permission checks and leases read (revision, owner,
members, budget, claimable and certified goals, …). Tasks and attempts are rows of
`mdc_task` and `mdc_attempt`; spent budget is the sum of closed attempts. The
other tables are `mdc_command` (idempotency records), `mdc_event`, `mdc_job`,
`mdc_token`, `mdc_workspace` and `mdc_project_environment` (`mdc_environment`
keeps the per-branch environments of earlier versions, read only). `migrate`
creates them and moves tasks and attempts out of boards written before.

A command runs in one transaction. A board command locks the board, replays the
stored response if the (project, actor, idempotency key) was seen, checks the
revision, applies the pure transition from `domain.ts`, enqueues a job if needed,
bumps the revision, writes the changed task and attempt rows and logs an event.
Lease and job commands are checked against the board as it is instead of the
client's revision ([Commands](../../reference/proofs/#commands)). `claim`,
`heartbeat`, `finish` without a note and the commands that only queue a job take
the lease path: they read `meta`, replay as above, change only their own task and
attempt rows or queue their job, and leave the board document and its revision
alone. Claims of one project serialize
on an advisory lock, so reservations never exceed the budget. Goal statuses are
recomputed from facts; route selection considers only active facts.

Jobs talk to LeanGround's fact API and TerminusDB. A claim lasts 90 seconds and
is renewed every 20 seconds while the job runs. Results are written into the board
with a checkpoint that rechecks the claim; a checkpoint that changes nothing a
client sees (an idle sync) keeps the revision. Retryable failures are retried
after 5 s, 15 s, 45 s, …, at most 15 minutes apart, for 8 tries in all. Periodic
sync jobs are enqueued for ready boards (not superseded, not per-branch proof
requests of earlier versions) whose last sync is older than 15 seconds; a sync
skips branches whose data version did not change.

## Data compatibility

The TerminusDB schema is that of the removed Rust application, so databases it
wrote open as they are, and node revisions are computed byte for byte as before.
Legacy fields (node `module`, the Lean project document) are preserved;
collaboration markers (certification, batch and definition IDs) go only into
block `metadata`, never into source. New data (workspaces, environments,
projects, tokens) lives only in PostgreSQL. Databases without a workspace record
are visible to administrators until adopted with `set_owner`.
