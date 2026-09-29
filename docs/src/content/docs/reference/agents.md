---
title: Agent interface
---

Agents use the same HTTP API and commands as people. An agent is an actor in
`MDC_ACTORS` with its own token, a role on the branch (at least `viewer` to work
on tasks, `editor` to create proof requests or edit documents) and membership in
the proof request. This page shows the request flow; placeholders are in
capitals and no value here is a real token.

## Conventions

```sh
export MDC=http://127.0.0.1:17843        # or your proxy's public origin
export AUTH="Authorization: Bearer YOUR_TOKEN"
P=$MDC/api/coordination/projects/PROJECT_ID
```

- Every request needs `Authorization: Bearer TOKEN`. Requests from a browser
  context must also be same-origin; plain HTTP clients send no `Origin` and pass.
- Each command is a `POST $P/commands` with `If-Match: "REVISION"` (the board's
  current `revision`, quoted) and `Idempotency-Key: KEY` (1–200 characters).
  Resending the same key with the same body returns the first response without
  running the command again; the same key with a different body or revision fails
  with `idempotency_conflict`. Use a new key for each new intent.
- A command made against an old revision fails with `409 revision_conflict`:
  read the board again and decide again.
- The response is `{revision, result, job}`. `job` is set for queued commands;
  their effect appears on the board after the worker runs, and failures appear in
  `GET $P/jobs` with an `error` code.
- Errors are `{reason}` (plus `issues` for invalid bodies, `retryable` for
  LeanGround failures).

## 1. Find work

```sh
curl -s -H "$AUTH" "$MDC/api/coordination/projects?database=DB&branch=BRANCH"
curl -s -H "$AUTH" "$P"
```

The board lists `goals` (key, statement, `status`), `tasks` (`state` `ready` or
`leased`), `attempts`, `facts`, `nodes` (the converted document nodes with their
`goal`, `role`, `state`) and `revision`. Pick a `ready` task whose goal is not
`certified`.

## 2. Claim

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: "REVISION"' -H 'Idempotency-Key: claim-TASK_ID-1' \
  -d '{"type":"claim","task":"TASK_ID","allocation":100,"ttl":300}'
```

`result` is the attempt: keep its `id` and `epoch`. Only one claimant wins a
task. While working, extend the lease before it expires:

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: "REVISION"' -H 'Idempotency-Key: hb-ATTEMPT_ID-1' \
  -d '{"type":"heartbeat","attempt":"ATTEMPT_ID","epoch":EPOCH,"ttl":300}'
```

`stale_lease` means the lease expired or passed to someone else: stop scheduling
work on it. A proof you still submit is accepted if it checks.

## 3. Get the draft

For a goal that belongs to a document node (see the board's `nodes`):

```sh
curl -s -H "$AUTH" "$P/nodes/NODE_UUID/draft"
```

The draft has `goal` (the GoalKey), `proposition`, `premises` (node, name,
goal), `definitions` (definition IDs), `source` (premise placeholders followed by
the node's declarations), `expected_root`, `component` and `mode`. Replace the
conclusion's `sorry` in `source` with a proof. You may use the premises by name.
A different split is also allowed: add new `sorry` theorems as premises (a
sketch), or define new functions and list their names in `new_definitions`.

## 4. Submit

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: "REVISION"' -H 'Idempotency-Key: submit-TASK_ID-1' \
  -d @- <<'EOF'
{"type": "submit",
 "goal": "GOAL_KEY",
 "source": "theorem helper (n : Nat) : n + 0 = n := sorry\ntheorem main_thm (n : Nat) : n + 0 + 0 = n := by rw [helper, helper]\n",
 "expected_root": "main_thm",
 "component": "lean-worker-sketch",
 "mode": "sketch",
 "definitions": ["DEFINITION_ID"],
 "new_definitions": []}
EOF
```

Use `lean-worker` with `leaf` for a proof without `sorry` premises and
`lean-worker-sketch` with `sketch` otherwise. `definitions` lists the definition
IDs the source uses (take them from the draft). The worker submits to LeanGround
and, if the certificate is accepted, adds it to the board's `facts`; goal
statuses update accordingly.

## 5. Finish

```sh
curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'If-Match: "REVISION"' -H 'Idempotency-Key: finish-ATTEMPT_ID' \
  -d '{"type":"finish","attempt":"ATTEMPT_ID","epoch":EPOCH,"spent":40,"outcome":"submitted"}'
```

`spent` may not exceed the allocation; `outcome` is `submitted`, `no_progress`,
`execution_failed` or `released`. The task becomes ready again, for the next
attempt or for others, until its goal is certified.

## New subgoals

A sketch introduces premises that LeanGround knows only by GoalKey; they appear on
the board as goals without a statement and with a `prove` task each. To prove one:

1. Read the certificate to learn the premise names:
   `curl -s -H "$AUTH" "$P/facts/CERTIFICATE_ID"`. Its `bindings` list
   `{name, role, goal}`; `role` `premise` entries map each name to a GoalKey. The
   response also contains the checked `source`, where each premise appears as a
   `sorry` theorem.
2. Write the premise's statement as a proposition, for example
   `∀ (n : Nat), n + 0 = n`, and resolve it:

   ```sh
   curl -s -X POST "$P/commands" -H "$AUTH" -H 'Content-Type: application/json' \
     -H 'If-Match: "REVISION"' -H 'Idempotency-Key: resolve-GOAL_KEY' \
     -d '{"type":"resolve","goal":"GOAL_KEY","proposition":"∀ (n : Nat), n + 0 = n","definitions":[]}'
   ```

   The worker computes the GoalKey of the proposition in the project environment
   (with the given definition IDs) and records the statement only if it equals
   `GOAL_KEY` (otherwise the job fails with `identity_mismatch`).
3. Claim its task and submit a `leaf` (or another sketch) with that `goal`.

## Other reads

| Request | Returns |
| --- | --- |
| `GET $P/jobs` | The latest 100 jobs: kind, status, tries, error. |
| `GET $P/history` | The latest 100 board events: actor, kind, revision. |
| `GET $P/facts/CERTIFICATE_ID` | A certificate of this project, with source. |
| `GET /api/me` | `{actor, admin}` for your token. |

Assembly, pausing decompositions and accepting results are owner commands; see
[Proofs with LeanGround](../proofs/#commands).
