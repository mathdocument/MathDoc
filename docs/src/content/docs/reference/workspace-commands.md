---
title: CLI commands
---

## Help, selection and output

`mdc --help` lists three command groups in this order:

| Scope | Commands |
| --- | --- |
| Project and service management | `status`, `init`, `start`, `stop` |
| Entire branch | `search`, `graph`, `export`, `import`, `history`, `branch`, `project` |
| Single node | `new`, `dep`, `show`, `edit`, `rename`, `metric`, `lean` |

Use `mdc COMMAND -h` or `mdc COMMAND SUBCOMMAND --help`. There is no `help`
subcommand. The root accepts only `-h/--help`; command help places `-h/--help` and
`-m/--meas` first, followed by a blank line and the other applicable options.

Branch and node commands require `-p/--proj DATABASE/BRANCH` after the command
name. The selector is inherited by nested subcommands. Except for `branch del`,
these commands require the selected branch's local service to be running.

```sh
mdc graph check -p myproject/main -m
mdc graph -p myproject/main check
mdc status --meas
```

There is no default project, default client port, `--url` option or `MDC_URL`
setting. The root and `status/init/start/stop` do not accept `-p/--proj`.
`mdc -m`, `mdc -m graph check` and `mdc -p myproject/main graph check` are invalid.

Successful operations print JSON to stdout; lists such as search and history can
be JSON arrays. `-m/--meas` prints inclusive timing measurements to stderr.
Operational failures return exit code 1; invalid CLI arguments return 2.
Help returns 0. An uncertified Lean check also returns 1 while preserving its
JSON result. No command performs a source-workspace refresh.

## Projects and services

| Command | Behavior |
| --- | --- |
| `mdc status` | List all MathDoc database branches and local service ports. |
| `mdc init DATABASE` | Create a database with default `main` branch and Lean settings. |
| `mdc start DATABASE/BRANCH [--port PORT]` | Start a background service and return URL, port, PID and log path. |
| `mdc stop DATABASE/BRANCH` | Wait for service and Lean worker shutdown; retain graph data and caches. |

Start requires an existing branch. Missing branches and duplicate starts fail.
Without `--port`, the OS chooses an available port; explicit ports are 1–65535 and
must be free. Services bind to `127.0.0.1`. Stopping an inactive branch is an error.
All commands work from any directory; `start` encapsulates its own background launch.

Status is an object whose keys are `DATABASE/BRANCH`:

```json
{
  "etp/main": { "port": 7600 },
  "mdocs/main": { "port": null }
}
```

`port` is null if no service owns that branch's lease in the configured cache
root. Crashed services do not leave a stale running port. An empty inventory is
`{}`. Status reads TerminusDB metadata and needs no running mdc service.

## Branches and history

```sh
mdc branch new agent -p myproject/main
mdc start myproject/agent
mdc history -p myproject/agent
mdc stop myproject/agent
mdc branch del -p myproject/agent
```

`branch new NAME` forks the selected running branch's current head in the same
database. It neither starts the new branch nor switches the source service.
Each service stays attached to one branch; `status` identifies it explicitly.
`history` returns the latest 50 TerminusDB commits.

`branch del` deletes the branch selected by `-p` directly in TerminusDB. It must
already be stopped; a running or starting service blocks deletion. The command
cleans that branch's Lean artifacts, certificates, editor workspaces, libraries
and logs in the configured cache root. Only an empty service lock remains for
coordination. Other branches and caches are unaffected. Immutable database
commits remain after their branch reference is removed.

There is no CLI merge, rebase, checkout, or database-deletion command. These
administrative operations use TerminusDB directly. Deleting the last branch does
not delete its database. See [Storage](../../concepts/workspaces/).

## Nodes

References are exact names or complete UUIDs. Ambiguous names require a UUID;
paths and UUID prefixes are not accepted.

```sh
mdc new -p myproject/main -t 'Lemma'
mdc new -p myproject/main -t 'Another lemma' --parent 'Theorem' --revision PARENT_REV
mdc show -p myproject/main 'Lemma'
mdc rename -p myproject/main 'Lemma' 'Renamed lemma' --revision NODE_REV
printf 'Explanation.\n' | mdc edit -p myproject/main 'Renamed lemma' --type text --revision NODE_REV
mdc edit -p myproject/main 'Renamed lemma' --type text --delete --revision NODE_REV
```

`new` returns the created node. With `--parent`, node creation and adding the edge
from parent to new node are one transaction; `--revision` applies to that parent
and requires `--parent`. Names may be duplicated, but generated UUIDs and module
identities are distinct.

`show` returns the node's fields, revision and formalization status. `rename`
changes only its display title, preserving its UUID and Lean module identity.
`edit` replaces or creates a complete source block from stdin. Types are `text`,
`lean` (default), `rocq` and `latex`. `--delete` removes that block and reads no
stdin; an empty source without `--delete` remains an existing block.

Use the revision returned by `show` on edits, renames and dependency mutations.
If omitted, the CLI fetches the latest revision immediately before its write.
Explicit revisions protect work based on an earlier read; stale writes fail
without overwriting newer data. This is separate from compilation.
See [Dependency commands](../dependency-commands/), [Lean checks](../work-and-compilers/)
and [Concurrent edits](../../development/safe-mutations/).

## Lean project settings

`project show` returns `{revision, project}` for the selected branch. `project set`
reads a complete project object from stdin, with `toolchain`, `lakefile` and
optional `manifest` string (or null). An explicit `--revision` uses the branch
revision from `project show`; otherwise the CLI fetches the current branch revision.

```sh
mdc project show -p myproject/main
mdc project set -p myproject/main --revision BRANCH_REV < lean-project.json
```

This configures Lean and external libraries. It does not create or select a
database. See [Configuration](../configuration/) for the input shape and library
pinning rules. Whole-graph [export and import](../../concepts/import-export/)
include these project settings, without compilation caches or other branches.
