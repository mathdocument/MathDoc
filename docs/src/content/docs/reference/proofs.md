---
title: Proofs with LeanGround
---

MathDoc stores Lean source; LeanGround checks it. This page covers the proof
environment, how nodes are converted and submitted, what the statuses mean, the
board commands and jobs, and how results are written back. The collaboration
model (goals, tasks, leases, review) is explained in
[Collaboration](../../concepts/collaboration/); the request flow for agents in
[Agent interface](../agents/).

## Proof environment

Each database has at most one proof environment, stored in PostgreSQL and shared
by all its branches. It fixes what the project's goals mean:

| Field | Meaning |
| --- | --- |
| `base_key` | Key of a LeanGround base (Lean version and library). MathDoc reads the base from LeanGround's `/v1/bases/<key>` and stores it. |
| `context` | Lines every node's context must equal: `opens`, `local_options` (`set_option …`), `universes`, `variables`. The schema also accepts `raw`, `local_notation`, `local_attrs`; `namespaces` must be empty. |
| `options` | Lean options, a JSON object of string, number or boolean values. |
| `minimum_trust` | `claimed`, `audited` (default) or `trusted`; weaker certificates are ignored. |
| `mode` | `declarations` (default): nodes are converted declaration by declaration. `modules`: every Lean node is a whole Lean file, and `context` and `options` must be empty ([Module projects](#module-projects)). |

```sh
mdc proof env -p myproject/main
cat <<'EOF' | mdc proof env set -p myproject/main
{"base_key": 1, "context": {"opens": ["Nat"]}, "options": {}, "minimum_trust": "audited"}
EOF
```

`-p` only names the database here; any branch selects the same environment.
Reading needs read access on `main`; setting needs owner or admin (a delegated
token also needs `maintain`). The response adds `base`, `environment_id` (a
digest of base, context, options and trust, and of the mode when it is `modules`), `updated_by` and, after a change,
`replaces_environment_id`. A database without an environment of its own uses the
one MathDoc stored for its `main` branch before environments moved to databases.

Changing the environment changes every GoalKey, so it **supersedes** the
current project board: it stays readable but accepts no commands, and the next
submission creates a new board. Setting an earlier environment again revives the
board made in it. Instances and classes cannot be registered;
they must already exist in the base.

## Submitting a branch

**Submit to LeanGround**, `mdc proof submit REF`, or `POST
/api/coordination/projects` ([HTTP API](../http-api/#collaboration)) on a branch
either creates the database's project or attaches the branch to it
([Collaboration](../../concepts/collaboration/#projects-and-branches)):

| Case | Requirements | Response |
| --- | --- | --- |
| No project in the current environment | `owner` on `main` and `editor` on the branch (a delegated token needs `maintain` and `edit`); `REF` is a node of the branch (`document_not_found`) with a Lean block (`root_has_no_lean_block`); members are known actors (`unknown_member`) who can read `main` (`member_lacks_document_access`) | `201`, the new board; the worker registers it in LeanGround with its service actor and members, then converts the branch |
| A project exists | `editor` on the branch (token: `edit`), the root node on the branch (`root_not_on_branch`), membership (`403 not_a_project_member`) | `200`, the board; an `attach` job converts the branch from the root node |

Both need a configured environment (`environment_not_configured`). The body is
`{document: {database, branch, node}, budget, members?, title?}`; `budget`,
`members` and `title` (default: the node's title) count only when the project is
created. The request's `Idempotency-Key` makes a retry return the first outcome
without another conversion. A board's `branches` map holds, per attached branch,
its `request` (root node, `bound`, `stale`, `error`, contract), converted
`nodes`, `picks`, `writebacks` and the last `accepted` writeback.

## Node conversion

The worker collects the root and every node it reaches through dependencies that
have a Lean block; nodes without one are informal, and the walk does not continue
through them. Each Lean block is split into top-level commands and classified:

| Block contents | Treatment |
| --- | --- |
| Only `def`, `abbrev`, `noncomputable def`, `structure`, `inductive` | **Definition node**: registered in LeanGround in dependency order (`define`), giving definition IDs. |
| Exactly one `theorem`/`lemma`, or the metadata `lean_conclusion` names one | **Theorem node**: its statement is resolved to a GoalKey together with the IDs of the definitions it uses. |
| Theorem whose proof is `sorry` | Open goal with a `prove` task; nothing is submitted. |
| Theorem with a proof | Submitted: theorem dependencies become premise placeholders (their statement with proof `sorry`) ahead of the node's declarations. With premises it goes to `lean-worker-sketch` in `sketch` mode, otherwise to `lean-worker` in `leaf` mode. |
| Anything else | Rejected on that node with a reason (below). |

Further rules:

- `import` lines are dropped; dependencies come from graph edges and definition IDs.
- A node may omit `open`, `set_option`, `universe` and `variable` lines; each
  kind it does contain must equal the environment context's lines of that kind,
  in order (`local_context_mismatch`).
- A definition node cannot depend on a theorem node.
- The mapping from nodes to definition IDs is kept in the project, not written into
  the document; definition IDs are content-addressed, so registering the same text
  again gives the same ID.

Rejection reasons recorded on a node include `multiple_conclusions`,
`mixed_declarations`, `unsupported_declaration` (`instance`, `class`, `namespace`,
attributes, `open … in`, …), `local_context_mismatch`, `premise_name_collision`,
`definition_name_taken` (a different definition with the same name in this
project, on any branch), `ambiguous_binding` and
`unregistered_statement_definition`. Nodes that depend on a rejected node are
rejected too, with the node named in `details`. LeanGround applies its own source
rules on submission, for example it rejects declaration names containing `'`;
its reason is recorded on the node.

If the root itself is rejected, the branch stays unbound (`request.bound` is
false) and its `request.error` is `root_<reason>` (also `root_not_on_branch`,
`root_has_no_lean_block` and `root_statement_differs`). Fix the node, then
submit the branch again.

**Resubmit** (`submit_node` with the branch) re-reads one theorem node from the
branch, resolves it and its premises again and submits its current proof; its
certificate becomes the branch's pick for its goal. Use it after replacing a
`sorry` in the document; a node still at `sorry` is rejected with `open_goal`.
The branch must be bound (`branch_not_bound`) and the node a converted theorem
node (`not_a_theorem_node`).

## Module projects

In a `modules` environment every node with a Lean block is a whole Lean file, which LeanGround compiles
as written: any Lean is allowed, namespaces, instances, attributes and `set_option` included. The design
and its measurements are in `docs/mathdoc.md` of the repository (§2.7.10, §3.11 and §4.13).

- **Module names.** A node's file is the module its Lean block metadata names (`lean_module`, or
  `import_module` for an imported file), else the node's own module, such as `Lib.N_…`. Other files
  import it by that name.
- **Attaching a branch.** The root node's file states the theorem: exactly one theorem whose proof is
  `sorry`, or the one `lean_conclusion` names. The Lean nodes the root reaches by dependencies or by its
  imports are registered together as one shared module; they may hold lemmas as well as definitions. A
  node that cannot be a file says why (`invalid_module_name`, `module_name_conflict`, or LeanGround's
  reason such as `compile_failed` with Lean's messages).
- **Contributions.** A proof of a goal is a set of files (`mdc pull` writes a directory,
  [Workspace commands](../workspace-commands/#module-projects)). LeanGround compiles them in a sandbox
  and connects their root theorem to the goal's statement by the kernel. Theorems the root uses whose
  proof is `sorry` are its premises and become goals; their statements may use only the base and shared
  modules. A contribution cannot import another one.
- **Review.** New decompositions are `active` at once; the owner can pause or retire one later.
- **Assembly** always uses the `lean-modules` component: certificates are merged at the constant level
  and replayed from an empty environment.
- **Writeback** creates a node for every file of every certificate on the route (block metadata
  `contribution`, `module_path`, `certification_id`), with edges for its imports and from a
  decomposition to the proofs of its premises, and nodes for shared modules a decomposition registered.
  The root node keeps its text, the statement, and records the final certificate.
- `submit`, `resolve`, `submit_node` and `negate` are refused (`not_in_module_mode`): a module goal has no
  proposition text, so it cannot be refuted by registering its negation.

## Statuses

A node's LeanGround certification comes from its own branch's conversion on the
database's project board (the current one, else a superseded one); failing that,
from the same node with the same Lean text (and definitions) on another attached
branch, current board first; failing that, from a per-branch proof request of
earlier MathDoc versions. `branch` in the status names whose conversion is shown.

| Status | Meaning |
| --- | --- |
| `not_submitted` | The node has a Lean block but no conversion of it applies. |
| `insufficient` | No accepted chain of certificates proves its goal yet. |
| `derivable` | Its goal follows from conditional certificates whose leaves are complete. |
| `certified` | A complete certificate for its goal exists. |
| `stale` | The node's Lean text, or a definition it uses, changed after submission. |
| `rejected` | The converter or LeanGround refused the node; `reason` and `details` say why. |
| `registered` | A definition node whose definitions LeanGround registered (`definition_ids`); a definition still waiting for its own dependencies shows `not_submitted`. |

Nodes created by a writeback (premises and definitions that had no node) are
recorded as converted on the branch as written, so they show their goal's status
or `registered` at once.

Nodes without a Lean block have no certification. The local-check dimension is
always `unverified` or `no_code`: MathDoc performs no local checks. Statuses are
derived from project-visible certificates without calling LeanGround's query, so
`truncated` is always `false`.

The worker syncs each ready, current project board about every 15 seconds (and on
`sync`): it rereads known certificates, reads new LeanGround events, marks
certificates that are no longer shared as unavailable (they stop counting),
records expired leases and checks every attached branch for staleness. A deleted
branch is marked stale.

## Commands

Project commands are `POST /api/coordination/projects/<id>/commands` bodies with a
`type`, sent with an `Idempotency-Key` (1–200 characters) and an `If-Match`
header; both headers are required (`400 invalid_request`).

- The lease and job commands (`claim`, `heartbeat`, `finish`, `submit`,
  `resolve`, `import`, `sync`, `submit_node`, `negate`) and the note commands
  (`note`, `edit_note`, `delete_note`) are checked against the board as it is:
  send them with `If-Match: *`. They are never refused for an outdated revision,
  and a replayed key is matched on the body alone. Of these, only the note
  commands and `finish` with a note change the board's revision when applied;
  the others' effects arrive with their jobs.
- Every other command needs the revision the client read (`If-Match:
  "REVISION"`; `428 revision_required` for `*`, `409 revision_conflict` if it
  changed).
- The response is `{revision, result, job}`. Commands marked *queued* also return
  a `job` ID and are carried out by the worker ([Jobs](#jobs)); their `result` is
  `{queued: true}`, or the new run for `assemble` and `retry`.

Every command needs membership (`404 not_found`) and read access on `main`;
owner commands from other members get `403 owner_required`. A command naming a
`branch` needs read access to it (`attach`: `editor`; a non-owner's `pick`:
write). On a delegated token, owner commands need `maintain`, `attach` needs
`edit`, `accept` needs both, and the rest need `work` (`403
token_scope_required`). Until its first conversion a board accepts only `sync`
(`project_initializing`). Superseded boards and per-branch proof requests of
earlier versions refuse every command (`project_superseded`,
`proof_request_read_only`).

| Type | Who | Fields | Effect |
| --- | --- | --- | --- |
| `members` | owner | `members` (up to 100 actor names) | Queued: replace the member list (the owner always stays) and register it in LeanGround; removed members' leases are revoked. The board is initializing until the job ends. |
| `notes` | member | `notes` (up to 100000 characters) | Save free-form project notes. |
| `task` | owner | `goal`, `kind` (`prove`/`decompose`/`formalize`), `priority` (−100 to 100, default 0) | Add a task; one open task per goal and kind (`duplicate_task`). |
| `claim` | member | `task`, `allocation`, `ttl` (30–3600, default 300) | Take a lease ([Collaboration](../../concepts/collaboration/#tasks-leases-and-budgets)); the result is the attempt. Refused with `task_unavailable`, `inactive_goal`, `already_certified` or `budget_exhausted`. |
| `heartbeat` | lease holder | `attempt`, `epoch`, `ttl` | Move the lease's end to `ttl` seconds from now (`stale_lease`). |
| `finish` | lease holder; to release, also the person behind the holder's token, or the owner (a token needs `maintain`) | `attempt`, `epoch`, `spent`, `outcome`, `note?` | End the attempt: `outcome` is `submitted`, `no_progress`, `execution_failed` or `released`; `spent` at most the allocation (`allocation_exceeded`); a note is kept on the task's goal. Anyone but the holder may only release, spending `0`: this frees a task a stopped session left before its lease expires (the web page's Release). |
| `note` | member | `goal`, `text` (1–500 characters) | Keep a note on a goal; the latest 50 stay. |
| `edit_note` | the note's author (or their person) | `goal`, `note` (`{actor, at}`, naming the note), `text` | Replace a note's text; the note records `edited`. `404 unknown_note`; `403 note_author_required`. |
| `delete_note` | the note's author (or their person), or the owner | `goal`, `note` (`{actor, at}`) | Remove a note. |
| `negate` | member | `goal` | Queued: register the goal's negation as a goal (job result `{goal, negation, name}`); answers `{goal, negation}` at once when one is registered ([Refutation](../../concepts/collaboration/#refutation)). `goal_unresolved` for a goal without a statement, `already_refuted`. |
| `resolve` | member | `goal`, `proposition`, `definitions` | Queued: give a known GoalKey its statement; the job fails with `identity_mismatch` if the proposition resolves to another key. |
| `submit` | member | `goal`, `source`, `expected_root`, `component`, `mode`, `definitions`, `new_definitions`, `description?` | Queued: submit a proof or sketch for a goal with a statement (`goal_identity_unavailable`); a sketch's new premises get their statements ([below](#new-subgoals)). Needs no lease. Not in module projects. |
| `submit_modules` | member | `goal`, `files` (path → text, 1–64), `root`, `shared`, `new_shared?`, `mode` (`leaf`/`sketch`), `description?` | Module projects only (`not_a_module_project`): queued, LeanGround checks the files as a contribution to the goal ([Module projects](#module-projects)); `new_shared` files are registered as a shared module first. Needs no lease. |
| `submit_node` | member | `node`, `branch` | Queued: resubmit a theorem node of a bound branch. |
| `attach` | member, `editor` on the branch | `branch` | Queued: convert the branch from the root node (again). |
| `import` | member | `certificate` | Queued: bring a certificate that is public or shared with this project into it (`fact_not_shared`, `environment_mismatch`, `trust_rejected`). |
| `sync` | member | — | Queued: sync now. |
| `decomposition` | owner | `certificate`, `state` (`active`/`paused`/`retired`), `comment?` (up to 2000 characters) | Change a certificate's state, recorded as its `review`; reviewing a `proposed` one is setting it `active` or `retired`. A `refuted` one stays (`certificate_refuted`). |
| `policy` | owner | `review` (`required`/`none`) | Whether new decompositions wait for review ([Review](../../concepts/collaboration/#review)). Project boards only. |
| `pick` | owner, or editor of `branch` | `branch`, `goal`, `certificate` (or `null`) | Set or clear the branch's pick for a goal; the certificate must prove `goal` (`certificate_goal_mismatch`). The owner needs `maintain` on a delegated token; a member who may write `branch` (or an owner's token without `maintain`) needs `edit` (`mdc pick`). |
| `assemble` | owner | `component` (`lean-worker`), `branch` | Queued: assemble the bound branch's route ([below](#assembly-and-writeback)). |
| `retry` | owner | `run` | Queued: retry a failed, retryable assembly with the same plan (`not_retryable`). |
| `accept` | owner, `editor` on the run's branch | `run` | Queued: write a certified assembly into its branch (`not_certified`, `proof_request_stale`). |

`component` and `mode` must pair: `lean-worker-sketch` with `sketch`,
`lean-worker` with `leaf` (`component_mode_mismatch`). `goal` may be omitted in
`submit` only when the conclusion uses a definition introduced by
`new_definitions` in the same submission (`goal_required`).
`description` (up to 2000 characters) says what a decomposition does; the first
one given is kept on the certificate (`facts.ID.description`), with the
`submitter` and `submitted_at`. A decomposition submitted this way is `proposed`
until the owner reviews it, unless the project's `policy` waives review; its
`submit` job result has its `state`. Agents usually submit through
[`mdc prove` and `mdc split`](../workspace-commands/#proof-work).

### New subgoals

LeanGround names a sketch's premises only by GoalKey. When the `submit` job
records a sketch, it states each premise whose goal has no statement yet: it
takes the placeholder declaration with that premise's name from the submitted
source, resolves it with the certificate's definitions and keeps it only if the
GoalKey matches. The goal then has an identity (so proofs of it can be
submitted) and a `declaration` (`name`, `binders`, `type`) from which
[goal drafts](../http-api/#collaboration) are written. A mismatch or a
LeanGround rejection is recorded in the job's `result` and the premise stays
without a statement; `resolve` can still give it one. Goals record only
definition IDs that LeanGround reports for their statements, never IDs taken
from a request, because goal drafts return the definitions' source.

## Assembly and writeback

`assemble` selects the branch's route through active certificates, from the root
to complete proofs: at a goal the branch picked, only the picked certificate;
elsewhere, preferring certificates with fewer premises. The branch must be bound
(`branch_not_bound`), and no other assembly may be queued or running
(`assembly_in_progress`). A route that does not close fails at once with
`no_active_route`. Before LeanGround fixes a plan, the worker rereads the
route's certificates and fails the run with `route_paused` if one is no longer
active. LeanGround combines the route into one source without premises and
checks it from scratch. The run (with its `branch`) becomes `certified`, or
`failed` with a reason, LeanGround's explanation as `details` when it gives
one, and a `retryable` flag; a job that fails for good fails its run the same
way.

`accept` builds one writeback batch (`writeback-batch.v1`) for the run's branch,
with that branch's converted nodes; no other branch is written:

1. Every existing node on the route must still have the Lean text it had when it
   was converted; otherwise the batch is `aborted` with `document_conflict` and
   nothing is written.
2. A node whose own text was certified keeps its text and gets metadata only.
   A node proved by a participant gets the certified declarations, keeping its
   `import` and context lines. The conclusion name must survive
   (`public_declaration_renamed` otherwise).
3. Subgoals without a node become new goal nodes titled by their conclusion
   name; definitions without a node become definition nodes with metadata
   `definition_id`. New node UUIDs derive from the batch ID, so replays never
   duplicate them. Edges are added to the providing nodes and definitions.
4. Written Lean blocks get `certification_id`, `certified_lean_sha256` and
   `coordination_operation` (the batch ID); the root's `certification_id` is the
   final assembly certificate.
5. The batch is one TerminusDB commit guarded by the data version it read; a
   concurrent write makes the worker retry and check again. A batch already
   committed is recognized by the root's `coordination_operation`.

The batch is recorded in `branches.BRANCH.writebacks`: `committed` with the new
data version and the `created` and `updated` nodes, or `aborted` with a `reason`
(`document_conflict`, `public_declaration_renamed`, `document_rejected` when
TerminusDB refuses the changes, …) and `details`.

`accept` is refused on a stale branch (`proof_request_stale`). A branch is stale
when the Lean text of its root, its direct Lean dependencies or the definitions
those depend on differs from the text when it was converted. Edit deeper nodes,
or submit proofs through the API, to keep a branch usable; changing the root or
its direct dependencies means submitting the branch again. Writeback's own edits
become the new baseline.

## Jobs

Queued commands, branch submissions and the periodic sync run as worker jobs,
one at a time per project. Their kinds are `provision` (creating the board,
`members`), `attach`, `submit_node`, `submit`, `resolve`, `negate`, `import`,
`sync`, `assemble` (also for `retry`) and `writeback` (`accept`). Only `submit`
(`{certificate, goal, mode, state, premises}`) and `negate` (`{goal, negation,
name}`) have a `result`; the others show their effect on the board.

`GET …/jobs/JOB_ID` shows `status` (`queued`, `running`, `done`, `failed`),
`tries`, `error`, `details` and `result`. `details` keeps LeanGround's
explanation of a refusal, for a proof Lean's errors, cut at 8000 characters; a
node LeanGround refuses keeps it as the binding's `details`. A failure that may pass (LeanGround timed out or
unreachable, `leanground_timeout` or `leanground_unavailable`; a LeanGround or
TerminusDB server error; a concurrent document write) is retried after 5 s,
15 s, 45 s, …, at most 15 minutes apart, 8 tries in all; meanwhile the job is
`queued` with the last `error`. Other failures end the job at once with their
code, for example a LeanGround rejection or `identity_mismatch`. A failed `sync`
sets the board's `sync_error`, a failed `attach` the branch's `request.error`.
A worker that stops mid-job loses the job after 90 seconds, and another worker
takes it over.

## Running LeanGround

MathDoc calls LeanGround's fact API (`/v1/facts/…`) and `/v1/bases/<key>` with
`LEANGROUND_FACT_TOKEN`; `LEANGROUND_ACTOR` is added to every LeanGround project
the worker registers. Calls that run Lean (`resolve`, `submit`, `define`,
`assemble`) may take `LEANGROUND_TIMEOUT_S` seconds (default 960), other calls
30 seconds ([Configuration](../configuration/#server-serve-and-worker)).
Requirements on the LeanGround side are listed in
[Installation](../../getting-started/installation/#requirements). Each project
board is one LeanGround project, with a random ID (`mdc-…`), so definitions and
certificates of all its branches share one scope.
