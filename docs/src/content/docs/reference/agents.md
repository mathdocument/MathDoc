---
title: Agent interface
---

Agents use the same HTTP API and commands as people. An agent acts with a
token: best a [delegated token](../../concepts/workspaces/#delegated-tokens) of
the person running it (`mdc token new codex-1`, acting as `you/codex-1` with the
`work` scope, which covers everything on this page), or an actor of its own in
`MDC_ACTORS`. Either way it needs a role on the database's branches (at least
`viewer` on `main` to work on tasks, `editor` on a branch to attach it or edit its
documents) and membership in the database's project
([Collaboration](../../concepts/collaboration/#projects-and-branches)); a
delegated token has its person's. This page shows the request flow; placeholders
are in capitals and no value here is a real token.

## With `mdc`

An agent that can run commands needs only four:
`mdc pull GOAL`, `mdc split FILE -m DESCRIPTION`, `mdc prove FILE` and `mdc done`
([Proof work](../workspace-commands/#proof-work)). They claim and renew leases,
write a goal file with the project's context, definitions and the goal's notes,
check the file's form locally (the Lean check is LeanGround's), and make every
submission safe to rerun after a lost reply. `mdc note` (or `mdc done --note`)
leaves what you learned for the next agent. `mdc next` says which goal is most
worth working on (`--claim` pulls it), `mdc refute GOAL` pulls a goal's negation
to prove the goal false, and `mdc goal GOAL` shows a goal's statement, notes and
decompositions
([Goals, review and routes](../workspace-commands/#goals-review-and-routes)).
`mdc check FILE` has LeanGround check a draft without submitting it, and
`mdc lemma QUERY` looks library declarations up by name; both go through
MathDoc, so an agent needs no LeanGround credential. Chat clients without a
shell get `pull`, `prove`, `split`, `check`, `lemma`, `done` and `note` as
tools of `mdc mcp` ([MCP](../workspace-commands/#mcp)). The rest of this page is
the HTTP flow underneath, for agents that call the API directly.

## Conventions

```sh
export MDC=http://127.0.0.1:17843        # or your proxy's public origin
export AUTH="Authorization: Bearer YOUR_TOKEN"
P=$MDC/api/coordination/projects/PROJECT_ID
```

- Every request needs `Authorization: Bearer TOKEN`.
- Each command is a `POST $P/commands` with an `If-Match` header and
  `Idempotency-Key: KEY` (1–200 characters). The commands on this page
  (`claim`, `heartbeat`, `finish`, `submit`, `resolve`, `note`, `negate`) are
  checked against the board as it is: send them with `If-Match: *`, without
  reading the board first. Other commands, such as the owner's, need
  `If-Match: "REVISION"` (the board's `revision`, quoted) and fail with
  `409 revision_conflict` once the board has changed: read it again and decide
  again ([Commands](../proofs/#commands)).
- Resending the same key with the same body returns the first response without
  running the command again; the same key with a different body (or, for
  revision-checked commands, a different revision) fails with
  `idempotency_conflict`. Keys belong to your identity; use a new key for each
  new intent.
- The response is `{revision, result, job}`. `job` is set for queued commands;
  their effect appears on the board after the worker runs.
  `GET $P/jobs/JOB_ID` shows the job's `status` (`queued`, `running`, `done`,
  `failed`), its `result`, or its `error` code.
- Errors are `{reason}` (plus `issues` for invalid bodies, `retryable` for
  LeanGround failures).

## 1. Find work

```sh
curl -s -H "$AUTH" "$MDC/api/coordination/projects?database=DB&branch=BRANCH"
curl -s -H "$AUTH" "$P/next"
curl -s -H "$AUTH" "$P"
```

In the list, the database's current project comes first: the board with
`project` set and `project.superseded` unset. Its `id` is PROJECT_ID.

`$P/next` returns `{revision, candidates}`: free goals worth working on, best
first ([What to do next](../../concepts/collaboration/#what-to-do-next)), each
with its `goal`, the `task` to claim, `score`, the `branches` whose routes need
it, `failures`, `statement` and `negates` (for a negation, the goal it refutes).
`?branch=BRANCH` ranks for that branch's route only; `?limit=N` (default 20, at
most 500). `mdc next` prints it.

The board lists `goals` (key, `statement`, `status`, `notes`, `refuted`),
`tasks` (`goal`, `kind`, `state` `ready`, `leased` or `cancelled`), `attempts`,
`facts` (certificates with their `state`), `negations`, `branches` (per attached
branch: `request`, `nodes` with each converted document node's `goal`, `role`,
`state`, and `picks`) and `revision`. Goals and tasks are shared by all
branches.

## 2. Claim

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: *' -H 'Idempotency-Key: claim-TASK_ID-1' \
  -d '{"type":"claim","task":"TASK_ID","allocation":100,"ttl":300}'
```

`result` is the attempt: keep its `id` and `epoch` (`expires` is in
milliseconds). Only one claimant wins a task; the others get `task_unavailable`.
A claim is also refused for a goal not reachable through active decompositions
(`inactive_goal`), for a `certified` goal (`already_certified`) and when the
budget would be exceeded (`budget_exhausted`). While working, extend the lease
before it expires, with a new key each time:

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: *' -H 'Idempotency-Key: hb-ATTEMPT_ID-1' \
  -d '{"type":"heartbeat","attempt":"ATTEMPT_ID","epoch":EPOCH,"ttl":300}'
```

`stale_lease` means the lease expired or passed to someone else: stop scheduling
work on it. Submitting needs no lease: a proof you still submit is accepted if it
checks.

## 3. Get the draft

For any goal with a statement:

```sh
curl -s -H "$AUTH" "$P/goals/GOAL_KEY/draft?branch=BRANCH"
```

The goal draft is what `mdc pull` writes into a goal file. `root` is the
conclusion to prove (`name`, `binders`, `type`); `premises` are available goals
stated the same way (for a goal that a converted node of BRANCH states, that
node's premise nodes; add more with `&with=KEY1,KEY2`); `definitions` have
`id`, `name` and `source`, dependencies first. It also has `context`,
`options`, `root_module`, the latest 20 `notes` and the goal's prove `task` with
its lease. Write `theorem NAME BINDERS : TYPE := PROOF`, preceded by the premises
you use as `theorem … := sorry` placeholders. Send only declarations: the
context comes from the project and definitions are passed by ID. A goal without
a statement gives `409 goal_unresolved`.

For a theorem node of an attached branch (see `branches.BRANCH.nodes`), the node
draft has a ready source:

```sh
curl -s -H "$AUTH" "$P/nodes/NODE_UUID/draft?branch=BRANCH"
```

It has `goal` (the GoalKey), `proposition`, `premises` (node, name, goal),
`definitions` (definition IDs), `source` (premise placeholders followed by the
node's current declarations), `expected_root`, `component`, `mode` and the
node's conversion `state`. Replace the conclusion's `sorry` in `source` with a
proof. You may use the premises by name.

Either way, a different split is also allowed: add new `sorry` theorems as
premises (a sketch), or define new functions in the source and list in
`new_definitions` those a premise statement uses, so that they are registered
with the submission.

## 4. Submit

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: *' -H 'Idempotency-Key: submit-TASK_ID-1' \
  -d @- <<'EOF'
{"type": "submit",
 "goal": "GOAL_KEY",
 "source": "theorem helper (n : Nat) : n + 0 = n := sorry\ntheorem main_thm (n : Nat) : n + 0 + 0 = n := by rw [helper, helper]\n",
 "expected_root": "main_thm",
 "component": "lean-worker-sketch",
 "mode": "sketch",
 "definitions": ["DEFINITION_ID"],
 "new_definitions": [],
 "description": "reduce to helper twice"}
EOF
```

Use `lean-worker` with `leaf` for a proof without `sorry` premises and
`lean-worker-sketch` with `sketch` otherwise (`component_mode_mismatch`).
`expected_root` is the conclusion's name; `definitions` lists the definition IDs
the source uses (take them from the draft). `description` (up to 2000
characters) is optional and says what a decomposition does; the owner sees it
when reviewing. The worker submits to LeanGround and, if the certificate is
accepted, adds it to the board's `facts`; goal statuses update accordingly. If
LeanGround refuses the source, the job fails with LeanGround's reason. A sketch
is `proposed` until the owner activates it, unless the project waives review
([Review](../../concepts/collaboration/#review)), so its new subgoals cannot be
claimed before that. If the reply is lost, resend the same request with the
same key: you get the first result, and nothing is submitted twice.

## 5. Finish

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: *' -H 'Idempotency-Key: finish-ATTEMPT_ID' \
  -d '{"type":"finish","attempt":"ATTEMPT_ID","epoch":EPOCH,"spent":40,"outcome":"submitted"}'
```

`spent` may not exceed the allocation (`allocation_exceeded`); `outcome` is
`submitted`, `no_progress`, `execution_failed` or `released`
(`no_progress` and `execution_failed` count as failed attempts when goals are
ranked). Add `"note": "…"` (up to 500 characters) to keep what you learned on the
goal, or send `{"type": "note", "goal": GOAL_KEY, "text": "…"}` at any time. The
task becomes ready again, for the next attempt or for others, until its goal is
certified.

## New subgoals

A sketch introduces premises that LeanGround knows only by GoalKey. They appear
on the board as goals with a `prove` task each, and the `submit` job states each
new one from its placeholder in the sketch
([New subgoals](../proofs/#new-subgoals)): the goal gets an `identity` and a
`declaration`. The job's `result` (`GET $P/jobs/JOB_ID`) has the `certificate`,
its `state`, and every premise with its `goal`, placeholder `name`, whether it is
`new`, and its `statement` (`resolved`, `known`, `identity_mismatch`,
`placeholder_not_found` or a LeanGround reason). A `proposed` sketch's subgoals
cannot be claimed (`inactive_goal`) until the owner activates it, but their goal
drafts ([3. Get the draft](#3-get-the-draft)) work at once.

If a premise could not be stated (its `statement` in the job result is not
`resolved` or `known`), resolve it yourself:

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: *' -H 'Idempotency-Key: resolve-GOAL_KEY' \
  -d '{"type":"resolve","goal":"GOAL_KEY","proposition":"∀ (n : Nat), n + 0 = n","definitions":[]}'
```

The worker computes the GoalKey of the proposition in the project environment
(with the given definition IDs) and records the statement only if it equals
`GOAL_KEY` (otherwise the job fails with `identity_mismatch`).

## Refute a goal

If a goal is false, prove its negation:

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: *' -H 'Idempotency-Key: negate-GOAL_KEY' \
  -d '{"type":"negate","goal":"GOAL_KEY"}'
```

The job's `result` is `{goal, negation, name}`; if the negation was already
registered, the response's `result` is `{goal, negation}` and there is no job.
Claim and prove the goal `negation` like any other: a complete proof of it
refutes GOAL ([Refutation](../../concepts/collaboration/#refutation)).

## Other reads

| Request | Returns |
| --- | --- |
| `GET $P/jobs` | The latest 100 jobs: kind, status, tries, error, result. |
| `GET $P/jobs/JOB_ID` | One job, with its `result` when done. |
| `GET $P/goals[?status=S&branch=BRANCH]` | One entry per goal, the root first (`mdc goals`). |
| `GET $P/goals/GOAL_KEY` | One goal in full: certificates, notes, tasks and attempts (`mdc goal`). |
| `GET $P/decompositions[?state=S]` | Decompositions with their premises, description and review (`mdc review`). |
| `GET $P/history` | The latest 100 board events: actor, kind, revision. |
| `GET $P/facts/CERTIFICATE_ID` | A certificate of this project, with source. |
| `GET /api/me` | `{actor, principal, admin, scopes}` for your token; `scopes` is `null` for a person's own token and the list for a delegated one. |

Their fields are listed in [HTTP API](../http-api/#collaboration). Assembly,
reviewing decompositions and accepting results are owner commands; picks are the
owner's and the branch editors'. See [Proofs with LeanGround](../proofs/#commands).
