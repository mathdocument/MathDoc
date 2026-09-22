---
title: CLI commands
---

## Help, selection and output

`mdc --help` lists three command groups in this order:

| Scope | Commands |
| --- | --- |
| Project and service management | `status`, `init`, `remove`, `start`, `stop`, `cache` |
| Entire branch | `search`, `graph`, `export`, `import`, `history`, `branch`, `project` |
| Single node | `new`, `del`, `dep`, `show`, `edit`, `rename`, `metric`, `lean` |

Use `mdc COMMAND -h` or `mdc COMMAND SUBCOMMAND --help`. There is no `help`
subcommand. The root accepts only `-h/--help`; command help places `-h/--help` and
`-m/--meas` first, followed by a blank line and the other applicable options.

Branch and node commands require `-p/--proj DATABASE/BRANCH` after the command
name. The selector is inherited by nested subcommands. Except for `branch del`,
these commands require both the entry server and the selected branch's service.

```sh
mdc graph check -p myproject/main -m
mdc graph -p myproject/main check
mdc status --meas
```

There is no default project, `--url` option or `MDC_URL` setting. The entry server
defaults to port 17843; clients discover it through its local lease. The root and
`status/init/remove/start/stop/cache` do not accept `-p/--proj`.
`mdc -m`, `mdc -m graph check` and `mdc -p myproject/main graph check` are invalid.

Successful operations print JSON to stdout; lists such as search and history can
be JSON arrays. `-m/--meas` prints inclusive timing measurements to stderr.
Operational failures return exit code 1; invalid CLI arguments return 2.
Help returns 0. An uncertified Lean check also returns 1 while preserving its
JSON result. No command performs a source-workspace refresh.

## Projects and services

| Command | Behavior |
| --- | --- |
| `mdc status` | Return entry server status and all database branches, including stopped branches. |
| `mdc init DATABASE` | Create a database with default `main` branch and Lean settings. |
| `mdc remove DATABASE` | Stop all branches of this project and delete its database, all branch history and local branch caches. |
| `mdc start [DATABASE/BRANCH] [--port PORT]` | Start/reuse the entry server; optionally start one branch. Return URL, entry port, process PID and log path. |
| `mdc start --foreground [--port PORT]` | Run a supervised server in the current process and restore its previously started branches. |
| `mdc stop [DATABASE/BRANCH]` | Unload one branch and stop its Lean workers, or stop the entire server and all branches when no branch is given. Retain graph data and caches. |
| `mdc cache stats DATABASE` | Report local shared artifact, mapping and certificate counts and sizes; no database connection or compilation. |
| `mdc cache gc DATABASE [--dry-run] [--older-than-days DAYS] [--max-bytes BYTES] [--certificates]` | Reclaim shared entries after all this database's branches are stopped. Defaults to a seven-day publication grace period. |

Cache GC evicts the oldest eligible Lake mappings, then unreferenced objects.
With `--max-bytes`, eviction stops when shared artifact logical bytes fit the
budget; newer entries can keep `budget_met` false. Without a budget it removes
all age-eligible mappings and unreferenced objects. `--older-than-days 0` permits
immediate cleanup. Certificates are retained unless `--certificates` is supplied;
that option removes age-eligible proof records independently of the artifact
budget. Unknown or malformed native mappings abort the plan before deletion.
GC is explicit, never part of a check or branch deletion.

`stats` reports logical bytes and allocated file blocks, counting each inode
once for allocated bytes within the reported set. Private workspaces and
directory metadata are excluded. GC's `reclaimable_file_bytes` excludes objects
with additional hardlinks: a private producer output may still retain those
bytes after the pool entry is removed. Stopped branch workspaces are removed by
`branch del`; this also releases their links. Empty lock files remain.

An optional branch must already exist and be stopped; missing branches and
duplicate branch starts fail. `mdc start` without a branch is safe to repeat.
The entry server defaults to port **17843**, configurable with TOML `port` or
`--port`. Explicit ports are 1–65535 and must be free. If the entry server is
already running, it is reused; requesting a different port fails until you stop
it with `mdc stop`. There is one process and one HTTP listener, bound to
`127.0.0.1` by default; branches have no separate HTTP ports or backend processes. Stopping an inactive service is an error.
Commands work from any directory; ordinary `start` encapsulates background launch.

`remove DATABASE` executes without an interactive prompt. It accepts a database
name, not `DATABASE/BRANCH`, and works with or without a running entry server.
With a server, it stops only this project's branches and their editor sessions;
other projects remain running. Save any drafts you intend to keep first.
Removal deletes every branch, including `main`, and the database history. It
clears the project's local caches (retaining empty branch lock files), removes
its foreground restore entries, and leaves shared Elan toolchains installed.
If a branch cache is owned by another process, removal fails before deleting the
database. A database deletion failure leaves compiler caches intact; a later
cleanup failure explicitly reports that the database was already deleted.

`--foreground` is for Docker or another process supervisor and cannot take a
branch argument. It emits readiness JSON without private tokens, logs to the
process streams, and exits cleanly on SIGTERM, Ctrl-C or `mdc stop`. Start and stop
individual branches using other CLI invocations. Their desired running state is
saved atomically in `CACHE_ROOT/ENDPOINT_HASH/.server/active-projects.json` and
restored after a foreground restart, including after a crash. Stopping an
individual branch removes it from that set; stopping the whole server preserves
the set. Failed restores are reported on stderr without preventing other branches
from starting. Ordinary background starts retain their existing empty-server behavior.

Open `http://127.0.0.1:17843/` for the project list and
`http://127.0.0.1:17843/p/DATABASE/BRANCH/` for a running branch. Starting or
stopping another branch leaves existing branches and their Lean sessions alone.

Status separates the entry server from the branches:

```json
{
  "server": {
    "running": true,
    "port": 17843,
    "url": "http://127.0.0.1:17843"
  },
  "projects": {
    "etp/main": { "running": false, "url": null },
    "mdocs/main": {
      "running": true,
      "url": "http://127.0.0.1:17843/p/mdocs/main/"
    }
  }
}
```

`running` reflects the held service lease in the configured cache root. When the
server is stopped, its `port` and `url` are null and all branches are stopped.
A stopped branch has a null URL. Restart the server with `mdc start` and explicitly
load desired branches with `mdc start DATABASE/BRANCH`. Crashed services do not remain marked running.
An empty inventory has `projects: {}`. Status reads TerminusDB metadata without
requiring a running mdc service, loading graphs or starting Lean.

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
CLI and browser both check the database's existing branch names and reject a
duplicate before sending a branch-creation request to TerminusDB.
The browser project list also offers Init, Start/Stop, New branch and Delete
controls. Init creates a database with a stopped `main` branch. Browser forking
also works from stopped branches; deletion still requires stopping first.
The `main` name has an outlined badge and no branch-delete button; its empty
delete slot sits outside the New branch button's border and keeps the remaining
buttons aligned with other branches. It can still
be started, stopped, edited and forked. Each project header has a separate Delete
project button. Its confirmation covers **all** branches, including branches
hidden by the current search or status filter, plus data, history, caches and
unsaved editor sessions. Project deletion stops its running branches automatically.
Each loaded branch keeps its own graph, Lean environment and cache; `status` identifies it explicitly.
`history` returns the latest 50 TerminusDB commits.

`branch del` deletes the branch selected by `-p` directly in TerminusDB. It must
already be stopped; a running or starting service blocks deletion. TerminusDB
protects `main`, so both CLI and browser reject its deletion before touching
its caches, regardless of how many other branches exist. The command
cleans that branch's private build outputs, editor workspaces, libraries and
logs in the configured cache root. Only an empty service lock remains for
coordination. The database's shared objects and certificates survive; reclaim
them explicitly with `cache gc`. Other branches are unaffected. Immutable database
commits remain after their branch reference is removed.

There is no CLI merge, rebase, or checkout command. These operations use
TerminusDB directly. Branch deletion does not delete its database; use
`mdc remove DATABASE` to remove the entire project. See [Storage](../../concepts/workspaces/).

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
mdc del -p myproject/main 'Renamed lemma' --revision NODE_REV
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

`del SOURCE` deletes the node and removes all incoming dependency edges in one
commit. Referrer nodes and the deleted node’s dependencies remain; their source
blocks are not rewritten. Affected Lean certifications become stale. The JSON
result is `{fnode, deleted: true, removed_edges}`, counting incoming and outgoing
edges. The web toolbar offers the same deletion with confirmation.

Use the revision returned by `show` on edits, renames, deletions and dependency mutations.
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

## LaTeX project files

```sh
mdc project latex show -p myproject/main
mdc project latex set -p myproject/main --preamble macros.tex --bib references.bib
```

`--preamble` accepts a `.tex` preamble or `.cls` file; `--bib` accepts a `.bib`
file. Both files are stored as text in the selected branch. Local paths are used
only to read the upload, never as ongoing workspace dependencies. `--revision`
accepts the branch revision returned by `project latex show`. The settings are
included in full-graph export and inherited by new branches.
