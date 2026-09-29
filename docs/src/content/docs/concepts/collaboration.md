---
title: Collaboration
---

A collaboration project lets several people and agents complete the proof of one
Lean node together. Everyone uses the same commands, whether through the
browser's **Collaboration** dialog, `mdc proof command` or the
[HTTP API](../../reference/agents/). LeanGround checks every proof; MathDoc keeps
track of goals, tasks, budgets and which results are used.

## Proof requests

A proof request is a collaboration project bound to a document node, its
**root**. It is created with **Submit to LeanGround**, `mdc proof submit REF` or
`POST /api/coordination/projects`, and fixes at creation:

- the branch's **proof environment**: LeanGround base, context, options and
  minimum trust ([Proofs](../../reference/proofs/#proof-environment));
- the fingerprints of the Lean text of the root, its direct premises and the
  definitions they use, to detect later edits;
- the **owner** (the creator), **members** and a **budget**.

The worker then converts the root and every Lean node it depends on: definition
nodes are registered in LeanGround, theorem statements are resolved, and nodes
with a proof are submitted. Nodes whose proof is `sorry` become open goals. The
rules are in [Proofs with LeanGround](../../reference/proofs/#node-conversion).
Until this first step finishes, the project is *initializing* and only `sync` is
accepted.

## Goals and GoalKey sharing

A **goal** is a proposition to prove. LeanGround assigns each proposition a
**GoalKey** computed from the statement and its environment; the same statement
gets the same key. Goals in a project are deduplicated by GoalKey, so two nodes,
or two decompositions, that need the same lemma share one goal, one set of tasks
and every result for it.

Each goal has a derived status:

| Status | Meaning |
| --- | --- |
| `insufficient` | No chain of accepted certificates proves it yet. |
| `derivable` | Provable by combining conditional certificates whose leaves are complete proofs. |
| `certified` | A complete certificate (no premises) exists. |

A **conditional certificate** (a *sketch*) proves a goal from premises that are
themselves goals. A cycle of sketches without a complete proof at its leaves
never makes a goal `derivable`.

## Tasks, leases and budgets

Every new goal gets a `prove` task; the owner can add `decompose` or `formalize`
tasks. Work on a task follows a lease:

1. `claim` a ready task with an **allocation** (budget reserved) and a lease
   time of 30–3600 seconds (default 300). Only one participant holds a task's
   lease; concurrent claims yield one winner.
2. `heartbeat` extends the lease while working.
3. `finish` reports what was spent (at most the allocation) and an outcome:
   `submitted`, `no_progress`, `execution_failed` or `released`. The task becomes
   ready again.

A lease that expires is recorded at the next state change: the attempt is marked
`expired`, charged its whole reservation, and the task becomes ready. Expiry only
withdraws the right to schedule the task; a proof submitted late is still
accepted if it checks. Claims fail with `budget_exhausted` once spent plus
reserved plus the new allocation would exceed the project budget. Budget units
are whatever the team agrees on; MathDoc only adds up what participants report.

Leases use an epoch: `heartbeat` and `finish` must quote the attempt's `epoch`,
so a participant whose lease was reassigned cannot overwrite the new holder
(`stale_lease`).

## Decompositions

A **decomposition** is a conditional certificate for a goal: its source proves
the goal from new premise statements written as `sorry` theorems, and may
introduce new definitions. LeanGround's sketch component checks that the
premises imply the conclusion. New premises arrive in the project as goals with
only a GoalKey; someone writes out the statement and `resolve`s it before proving
it ([Agent interface](../../reference/agents/#new-subgoals)).

The owner sets each certificate's state. Only `active` certificates count for
scheduling and assembly: `paused` and `retired` ones stay recorded but are never
chosen, and either can be made active again. No state deletes a fact. Tasks can
be claimed only for goals reachable from the root through active certificates
(otherwise `inactive_goal`).

## Assembly

When the root is `derivable` or `certified`, the owner **assembles**: MathDoc
selects a route through active certificates only, from the root down to complete
proofs, and asks LeanGround to combine them into one source without premises,
checked from scratch by `lean-worker`. A failed assembly marked retryable can be
retried with the same plan; a new route is a new assembly.

## Writeback

Accepting a certified assembly writes the certified sources back into the bound
branch as one batch, in a single TerminusDB commit:

- Every existing node on the route must still have the Lean text it had when it
  was converted; otherwise the whole batch is `aborted` with
  `document_conflict` and nothing is written. Restore or resubmit, then accept
  again.
- Nodes whose own text was certified get only metadata. Nodes proved by a
  participant get the certified declarations, keeping their `import` and context
  lines; the conclusion's name must stay the same.
- Subgoals from decompositions without a node, and definitions without a node,
  become new nodes with edges to what they use.
- Lean block metadata records `certification_id`, `certified_lean_sha256` and the
  batch ID; the root records the final assembly certificate.

Replaying a batch never creates nodes twice. See
[Proofs with LeanGround](../../reference/proofs/#assembly-and-writeback) for details.

## Staleness

A request becomes **stale** when a sync finds that the Lean text of the root, a
direct premise or a definition it uses has changed. Its results stay in history,
but `accept` is refused (`proof_request_stale`); submit the root again to start a
new request. Independently, a node whose text or definitions changed after
submission shows **Changed since submission** instead of Certified. A writeback's
own changes are the new baseline and do not make the request stale.

## Permissions

Collaboration never widens document access:

- Creating a request needs `editor` or better on the branch.
- Every member must be able to read the branch; adding one who cannot fails with
  `member_lacks_document_access`.
- Every project route rechecks the caller's read access; without it the project
  reads as `404`.
- The owner alone changes members, adds tasks, manages decompositions, assembles,
  retries and accepts. Members claim tasks, submit, resolve and sync.

Changing members revokes the open leases of removed members (charged their
reservation).
