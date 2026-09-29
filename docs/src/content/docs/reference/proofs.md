---
title: Proofs with LeanGround
---

MathDoc stores Lean source; LeanGround checks it. This page covers the proof
environment, how nodes are converted and submitted, what the statuses mean and
how results are written back. The collaboration model (goals, tasks, leases) is
explained in [Collaboration](../../concepts/collaboration/); the request flow for
agents in [Agent interface](../agents/).

## Proof environment

Each branch has at most one proof environment, stored in PostgreSQL. It fixes
what every new proof request on that branch means:

| Field | Meaning |
| --- | --- |
| `base_key` | Key of a LeanGround base (Lean version and library). MathDoc reads the base from LeanGround's `/v1/bases/<key>` and stores it. |
| `context` | Lines every node's context must equal: `opens`, `local_options` (`set_option …`), `universes`, `variables`. The schema also accepts `raw`, `local_notation`, `local_attrs`; `namespaces` must be empty. |
| `options` | Lean options, a JSON object of string, number or boolean values. |
| `minimum_trust` | `claimed`, `audited` (default) or `trusted`; weaker certificates are ignored. |

```sh
mdc proof env -p myproject/main
cat <<'EOF' | mdc proof env set -p myproject/main
{"base_key": 1, "context": {"opens": ["Nat"]}, "options": {}, "minimum_trust": "audited"}
EOF
```

Reading needs read access; setting needs owner or admin. The response adds
`base`, `environment_id` (a digest of base, context, options and trust),
`updated_by` and, after a change, `replaces_environment_id`. Changing the
environment affects new proof requests only. Instances and classes cannot be
registered; they must already exist in the base.

## Submitting a node

Create a proof request rooted at a Lean theorem node with **Submit to LeanGround**,
`mdc proof submit REF`, or `POST /api/coordination/projects`
([HTTP API](../http-api/#collaboration)). Requirements: `editor` or better on the
branch, a configured environment (`environment_not_configured`), a Lean block on
the root (`root_has_no_lean_block`), and members who are known actors
(`unknown_member`) and can read the branch (`member_lacks_document_access`).

The request is created at once (`201`); the worker then registers the project in
LeanGround with its service actor and members, and converts the document.

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
- `open`, `set_option`, `universe` and `variable` lines in a node must equal the
  environment context line for line (`local_context_mismatch`).
- A definition node cannot depend on a theorem node.
- The mapping from nodes to definition IDs is kept in the project, not written into
  the document; definition IDs are content-addressed, so registering the same text
  again gives the same ID.

Rejection reasons recorded on a node include `multiple_conclusions`,
`mixed_declarations`, `unsupported_declaration` (`instance`, `class`, `namespace`,
attributes, `open … in`, …), `local_context_mismatch`, `premise_name_collision`,
`definition_name_taken` (a different definition with the same name in this
project), `ambiguous_binding` and `unregistered_statement_definition`. Nodes that
depend on a rejected node are rejected too, with the node named in `details`.
LeanGround applies its own source rules on submission, for example it rejects
declaration names containing `'`.

If the root itself is rejected, the request stays initializing with a sync error
`root_<reason>`. Fix the node, then send `sync` to convert again.

**Resubmit** (`submit_node`) re-reads one theorem node from the document, resolves
it and its premises again and submits its current proof. Use it after replacing a
`sorry` in the document; a node still at `sorry` is rejected with `open_goal`.

## Statuses

A node's LeanGround certification comes from the latest proof request that
contains it:

| Status | Meaning |
| --- | --- |
| `not_submitted` | The node has a Lean block but is in no proof request. |
| `insufficient` | No accepted chain of certificates proves its goal yet. |
| `derivable` | Its goal follows from conditional certificates whose leaves are complete. |
| `certified` | A complete certificate for its goal exists. |
| `stale` | The node's Lean text, or a definition it uses, changed after submission. |
| `rejected` | The converter or LeanGround refused the node; `reason` and `details` say why. |

Nodes without a Lean block have no certification. The local-check dimension is
always `unverified` or `no_code`: MathDoc performs no local checks. Statuses are
derived from project-visible certificates without calling LeanGround's query, so
`truncated` is always `false`.

The worker syncs each ready project about every 15 seconds (and on `sync`):
it rereads known certificates, reads new LeanGround events, drops certificates
that are no longer shared, records expired leases and checks staleness.

## Commands

Project commands are `POST /api/coordination/projects/<id>/commands` bodies with a
`type`. Commands marked *queued* return `{queued: true}` and are carried out by
the worker; watch `/jobs` or the board.

| Type | Who | Fields | Effect |
| --- | --- | --- | --- |
| `members` | owner | `members` | Replace the member list; queued re-registration in LeanGround. |
| `notes` | member | `notes` | Save free-form project notes. |
| `task` | owner | `goal`, `kind` (`prove`/`decompose`/`formalize`), `priority` | Add a task. |
| `claim` | member | `task`, `allocation`, `ttl` | Take a lease ([Collaboration](../../concepts/collaboration/#tasks-leases-and-budgets)). |
| `heartbeat` | lease holder | `attempt`, `epoch`, `ttl` | Extend the lease. |
| `finish` | lease holder | `attempt`, `epoch`, `spent`, `outcome` | End the attempt. |
| `resolve` | member | `goal`, `proposition`, `definitions` | Queued: give a known GoalKey its statement. |
| `submit` | member | `goal`, `source`, `expected_root`, `component`, `mode`, `definitions`, `new_definitions` | Queued: submit a proof or sketch. |
| `submit_node` | member | `node` | Queued: resubmit a document theorem node. |
| `import` | member | `certificate` | Queued: bring a shared certificate into the project. |
| `sync` | member | — | Queued: sync now. |
| `decomposition` | owner | `certificate`, `state` (`active`/`paused`/`retired`) | Change a certificate's state. |
| `assemble` | owner | `component` (`lean-worker`) | Queued: assemble a route. |
| `retry` | owner | `run` | Queued: retry a failed, retryable assembly with the same plan. |
| `accept` | owner | `run` | Queued: write a certified assembly back. |

`component` and `mode` must pair: `lean-worker-sketch` with `sketch`,
`lean-worker` with `leaf` (`component_mode_mismatch`). `goal` may be omitted in
`submit` only when the conclusion uses a definition introduced by
`new_definitions` in the same submission.

## Assembly and writeback

`assemble` picks a route through active certificates, from the root to complete
proofs, preferring certificates with fewer premises. LeanGround combines the
route into one source without premises and checks it from scratch. The run
becomes `certified`, or `failed` with a reason and a `retryable` flag.

`accept` builds one writeback batch (`writeback-batch.v1`) for the bound branch:

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

`accept` is refused on a stale request (`proof_request_stale`). A request is stale
when the Lean text of its root, its direct Lean dependencies or the definitions
those depend on differs from the text at binding time. Edit deeper nodes, or
submit proofs through the API, to keep a request usable; changing the root or its
direct dependencies means submitting the root again. Writeback's own edits
become the new baseline.

## Running LeanGround

The worker calls LeanGround's fact API (`/v1/facts/…`) and `/v1/bases/<key>` with
`LEANGROUND_FACT_TOKEN`; `LEANGROUND_ACTOR` is added to every LeanGround project
the worker registers. Requirements on the LeanGround side are listed in
[Installation](../../getting-started/installation/#requirements). Project IDs are
derived from the creating actor and the request's `Idempotency-Key`; two MathDoc
deployments sharing one LeanGround should not reuse keys for the same actor
name, or their requests land in the same LeanGround project.
