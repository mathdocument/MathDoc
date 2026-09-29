---
title: Quick start
---

This walk-through creates a small document, sets its proof environment and
submits a theorem to LeanGround. It assumes a running installation
([local](../installation/) or [Docker](../server-deployment/)), an administrator
token and a reachable LeanGround server with a registered base.

## Create a project

```sh
export MDC_TOKEN=your-admin-token
export MDC_PROJECT=myproject/main   # default for -p
mdc init myproject
mdc grant bob editor                # optional: let actor bob edit this branch
```

`init` creates the TerminusDB database `myproject` with a `main` branch; you
become its owner. Open `http://127.0.0.1:17843/p/myproject/main/` in the browser
(sign in with the same token) or keep using the CLI.

## Write nodes

```sh
mdc new -t double
printf 'def double (n : Nat) : Nat := n + n\n' | mdc edit double --type lean

mdc new -t double_eq
printf 'theorem double_eq (n : Nat) : double n = n + n := rfl\n' | mdc edit double_eq --type lean
mdc dep add double_eq -t double
mdc show double_eq
```

Node references are exact titles or full UUIDs. An edge added with
`dep add A -t B` means A depends on B; `mdc new -t TITLE --parent A` creates a node
and the edge from A to it in one step.

`show` returns the node with its `revision` and status. Pass `--revision REV` on
later writes to fail instead of overwriting someone else's newer edit. Text and
LaTeX blocks work the same way with `--type text` or `--type latex`.

## Set the proof environment

The branch owner fixes which LeanGround base and Lean context every proof
request on this branch uses. `base_key` is the key of a base registered in your
LeanGround server:

```sh
echo '{"base_key": 1, "minimum_trust": "audited"}' | mdc proof env set
mdc proof env
```

In the browser this is **Project settings → Proof environment**.

## Submit a theorem

```sh
mdc proof submit double_eq --budget 1000
mdc proof list
mdc proof status PROJECT_ID
```

`proof submit` creates a proof request rooted at `double_eq`. The worker registers
`double` as a LeanGround definition, resolves the statement of `double_eq` and
submits its proof. `proof status` returns the whole project board; once LeanGround
has answered, the root goal's `status` becomes `certified` (or stays
`insufficient` with a reason on the node). In the browser, the node shows
**Certified** and the toolbar's **Collaboration** dialog shows the request.

A theorem whose proof is `sorry` becomes an open goal with a task instead; others
can claim it, as described in [Collaboration](../../concepts/collaboration/).
When the root is proved, the owner assembles a route and accepts it to write the
certified sources back; see [Proofs with LeanGround](../../reference/proofs/).

## Back up

```sh
mdc export > backup.json
```

[Export and restore](../../concepts/import-export/) describes the bundle.
