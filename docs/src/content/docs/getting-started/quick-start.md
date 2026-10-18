---
title: Quick start
---

This walk-through proves a small theorem end to end: the owner writes it as a
graph and creates the database's project, an agent splits the theorem and proves
the new lemma, and the owner reviews the split, assembles the proof and writes it
back. It assumes a running installation ([local](../installation/) or
[Docker](../server-deployment/)) with its worker, an administrator token and a
reachable LeanGround server with a registered base. With Docker, use the
deployment's [`mdc` wrapper](../server-deployment/#the-mdc-wrapper) (`./mdc`, or
its path from another directory) wherever this page says `mdc`.

## Create a database

```sh
export MDC_TOKEN=your-admin-token
export MDC_PROJECT=myproject/main   # default for -p
mdc init myproject
```

`init` creates the TerminusDB database `myproject` with a `main` branch; you
become its owner. Open `http://127.0.0.1:17843/p/myproject/main/` in the browser
(sign in with the same token) or keep using the CLI. To let another person from
`MDC_ACTORS` take part, give them a role on `main` (`mdc grant bob viewer` is
enough to work on goals; `editor` also lets them edit) and name them with
`--members` when you create the project; see
[Workspaces](../../concepts/workspaces/#workspaces-and-roles).

## Write nodes

```sh
mdc new -t double
printf 'def double (n : Nat) : Nat := n + n\n' | mdc edit double --type lean

mdc new -t double_add
printf 'theorem double_add (n : Nat) : double n + 0 = n + n := sorry\n' | mdc edit double_add --type lean
mdc dep add double_add -t double
mdc show double_add
```

Node references are exact titles or full UUIDs. An edge added with
`dep add A -t B` means A depends on B; `mdc new -t TITLE --parent A` creates a node
and the edge from A to it in one step.

`show` returns the node with its `revision` and status. Pass `--revision REV` on
later writes to fail instead of overwriting someone else's newer edit. Text and
LaTeX blocks work the same way with `--type text` or `--type latex`.

`double_add` will be the project's root theorem. Its proof is `sorry`, so it
becomes an open goal; a node with a finished proof is submitted as written.

## Set the proof environment

The database's owner fixes which LeanGround base and Lean context every goal in
the database uses; all branches share it. `base_key` is the key of a base
registered in your LeanGround server:

```sh
echo '{"base_key": 1}' | mdc proof env set
mdc proof env
```

`minimum_trust` defaults to `audited`; `context` and `options` are described in
[Proof environment](../../reference/proofs/#proof-environment). Changing the
environment later supersedes the project, and the next submission starts a new
one. In the browser this is **Project settings → Proof environment**.

## Create the project

```sh
mdc proof submit double_add --budget 1000
mdc proof status PROJECT_ID              # PROJECT_ID: the "id" printed by submit
```

The first `proof submit` in a database creates its project with `double_add` as
the root theorem and attaches the branch; `--members bob,carol` adds members. The
worker converts the branch: it registers `double` as a LeanGround definition,
resolves the statement of `double_add` and gives the open goal a `prove` task.
Wait until `proof status` shows `"ready": true`. `proof submit` on another
branch attaches that branch to the same project. In the browser, the node's
**Submit to LeanGround** button does the same, and the toolbar's
**Collaboration** button opens the project's collaboration page.

## Split the goal as an agent

Give each agent process its own
[delegated token](../../concepts/workspaces/#delegated-tokens). It acts as
`YOU/NAME` with your roles and membership, but by default only with the `work`
scope: it can claim and submit, not review or accept.

```sh
mdc token new agent-1      # prints {"token": "mdct_…", "actor": "you/agent-1", …} once
```

In the agent's shell, in a directory of its own:

```sh
export MDC_TOKEN=mdct_…               # the agent's token
export MDC_PROJECT=myproject/main
mkdir agent && cd agent
mdc pull double_add                   # claims the goal; writes 3f9a1c2e-double_add.lean
```

The file name starts with the first 8 hex digits of the goal's GoalKey. Above
the line `-- mdc: edit below this line` is a read-only header: the base's
`import`, the project's context and the registered definition `double`. Below it
is the goal's theorem with proof `sorry`. The agent splits the goal: it adds a
premise (a `sorry` theorem, which becomes a new goal) and proves the root from
it, so that the part below the line reads:

```lean
theorem double_eq (n : Nat) : double n = n + n := sorry

-- Goal. Do not change its statement.
theorem double_add (n : Nat) : double n + 0 = n + n := by
  rw [Nat.add_zero, double_eq]
```

```sh
mdc split 3f9a1c2e-double_add.lean -m "unfold double" --wait
mdc done 3f9a1c2e-double_add.lean
```

`split --wait` reports the decomposition's `certificate`, its
`certificate_state` (`proposed`) and the new goal in `premise_goals`. LeanGround
checked that the premise implies the root, not that the premise is true. `done`
ends the lease with the outcome `submitted`. A complete proof goes through
`mdc prove FILE` instead, and a goal the agent believes false through
`mdc refute GOAL` ([Refutation](../../concepts/collaboration/#refutation)).

## Review the decomposition

The owner, with their own token:

```sh
mdc review                                      # decompositions awaiting review
mdc review 4be1c09a --accept -m "the right lemma"
```

`mdc review` lists each decomposition's `id` with its goal, premises,
description and submitter; `4be1c09a` stands for that ID or a unique prefix of
at least 8 hex digits. A proposed decomposition schedules nothing: its new goal
cannot be claimed (`inactive_goal`) until the owner activates it
([Review](../../concepts/collaboration/#review)). In the browser:
**Collaboration → Certificates → Pending review**, **Activate**.

## Prove the new goal

Back in the agent's directory:

```sh
mdc next --claim                      # pulls the goal most worth working on: double_eq
# below the line, replace the proof `sorry` of double_eq with `rfl`
mdc prove 9d02b7c1-double_eq.lean --wait
mdc done 9d02b7c1-double_eq.lean
```

`prove --wait` reports `"status": "certified"`. Every goal on `main`'s route is
now proved: `mdc goals` shows `double_eq` as `certified` and the root as
`derivable`.

## Assemble and accept

The owner has LeanGround combine `main`'s route into one proof without premises,
checked from scratch, and then writes it back:

```sh
echo '{"type": "assemble", "component": "lean-worker", "branch": "main"}' | mdc proof command PROJECT_ID
mdc proof status PROJECT_ID              # wait until runs.RUN_ID.status is "certified"
echo '{"type": "accept", "run": "RUN_ID"}' | mdc proof command PROJECT_ID
mdc show double_add
```

RUN_ID is the `result.id` of the `assemble` reply. `accept` writes the certified
sources into `main` in one commit: `double_add` gets the agent's proof and
certification metadata, and `double_eq`, which had no node, becomes a new node.
Once the worker has written them, `show` reports
`formalization.lean_certification.status` as `certified` and the browser shows
**Certified** on the node. In the browser, **Collaboration → Review** has
**Assemble a route** and **Accept and write back**. See
[Assembly](../../concepts/collaboration/#assembly) and
[Writeback](../../concepts/collaboration/#writeback).

## Back up

```sh
mdc export > backup.json
```

[Export and restore](../../concepts/import-export/) describes the bundle.
