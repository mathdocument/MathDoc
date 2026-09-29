---
title: CLI commands
---

`mdc` is a thin client of the [HTTP API](../http-api/): each command is one or two
requests made with your token, and the output is the JSON response. It keeps no
local state and can run on any machine that reaches the server.

## Setup

| Environment | Meaning |
| --- | --- |
| `MDC_URL` | Server origin; default `http://127.0.0.1:17843`. |
| `MDC_TOKEN` | Your access token (required). |
| `MDC_PROJECT` | Default `DATABASE/BRANCH` for branch, node and proof commands. |

`-p DATABASE/BRANCH` selects the branch explicitly and overrides `MDC_PROJECT`.
Flags may appear anywhere after `mdc`. From a checkout, run
`node coordinator/dist/cli.js`; in the Docker image `mdc` is on the `PATH`, and
a deployment's `mdc` wrapper (`scripts/mdc-docker` in a checkout) runs it in the
`runtime` container from the host.

Output is pretty-printed JSON on stdout. Exit code 2 means a usage error (the
usage text is printed); 1 means the request failed, with `HTTP STATUS: message`
on stderr. `mdc -h` prints the command list.

## Server and workspaces

| Command | Behavior |
| --- | --- |
| `mdc status` | `{server, projects}`: every branch you can read, with URL and your role. |
| `mdc init DATABASE` | Create a database with a `main` branch; you become its owner. Admin only. |
| `mdc remove DATABASE` | Delete the database, all branches and history, and its workspace record. Admin only; no prompt. |
| `mdc grant ACTOR owner\|editor\|viewer\|none -p DB/BRANCH` | Set ACTOR's role on that branch (`none` removes it). Owner or admin. |

Roles and their rights are listed in [Workspaces](../../concepts/workspaces/#workspaces-and-roles).

## Branches

| Command | Behavior |
| --- | --- |
| `mdc branch new NAME -p DB/BRANCH` | Fork the selected branch's current head as `DB/NAME`; the new branch copies its role rules. Editor or better. |
| `mdc branch del -p DB/BRANCH` | Delete a branch other than `main`. Owner or admin. |
| `mdc history -p DB/BRANCH` | The latest 50 TerminusDB commits. |
| `mdc export -p DB/BRANCH` | Whole-branch bundle ([Export and restore](../../concepts/import-export/)). |
| `mdc import FILE -p DB/BRANCH` | Load a bundle into an empty branch. Owner or admin. |
| `mdc project latex show -p DB/BRANCH` | `{revision, project}` with the shared macro file and bibliography. |
| `mdc project latex set --preamble FILE --bib FILE -p DB/BRANCH` | Upload both files (`.tex`/`.cls` and `.bib`). |

There is no merge, rebase or checkout command; use TerminusDB directly for those.

## Graph

| Command | Behavior |
| --- | --- |
| `mdc graph check` | Node and edge counts and graph issues. |
| `mdc graph roots` | Unreferenced nodes with depth and component size. |
| `mdc graph full` | All node summaries and index-pair edges. |
| `mdc search QUERY [-n N]` | Case-insensitive title/UUID matches, at most N (default and cap 200). |

See [Graph and metrics](../graph-and-metrics/).

## Nodes

References are exact titles or complete UUIDs.

| Command | Behavior |
| --- | --- |
| `mdc show REF` | The node with `revision`, blocks, dependencies and status. |
| `mdc new -t TITLE [--parent REF]` | Create and return a node; with `--parent`, also add the edge parent → new node in the same commit and return the updated parent (the new UUID is the last entry of its `depens`). |
| `mdc del REF` | Delete the node and every edge to it. |
| `mdc rename REF TITLE` | Change the title; the UUID stays. |
| `mdc edit REF [--type text\|lean\|rocq\|latex]` | Replace or create that block with stdin (default type `lean`). |
| `mdc edit REF --type TYPE --delete` | Delete the block; reads no stdin. |
| `mdc dep …`, `mdc metric ior REF` | See [Dependency commands](../dependency-commands/) and [Graph and metrics](../graph-and-metrics/). |

Every write command accepts `--revision REV`, the revision printed by `show` (for
`new --parent` and `dep` the parent's or source's revision; for
`project latex set` the branch revision from `project latex show`). A stale
revision fails with `412` without writing. Without `--revision` the CLI reads the
current revision immediately before writing.

```sh
mdc show -p myproject/main Lemma            # note "revision"
printf 'Explanation.\n' | mdc edit -p myproject/main Lemma --type text --revision REV
```

## Proofs

| Command | Behavior |
| --- | --- |
| `mdc proof env` | The branch's proof environment (`404 environment_not_configured` if unset). |
| `mdc proof env set` | Set it from JSON on stdin; owner or admin. |
| `mdc proof submit REF [--budget N] [--members a,b]` | Create a proof request rooted at REF (budget default 1000). |
| `mdc proof list [REF]` | Your proof requests on the branch, or those containing REF. |
| `mdc proof status ID` | The whole project board. |
| `mdc proof command ID` | Send one command (JSON on stdin) at the board's current revision. |

```sh
echo '{"base_key": 1, "context": {"opens": ["Nat"]}}' | mdc proof env set -p myproject/main
mdc proof submit 'Main theorem' -p myproject/main --members bob,agent-1
echo '{"type": "sync"}' | mdc proof command PROJECT_ID
```

`proof command` reads the board first and sends its revision as `If-Match`, with
a fresh `Idempotency-Key`; rerunning the command is therefore a new request, not
a replay. Agents that need replay safety should call the
[HTTP API](../agents/) directly. Command types are listed in
[Proofs with LeanGround](../proofs/#commands).
