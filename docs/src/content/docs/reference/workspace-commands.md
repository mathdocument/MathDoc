---
title: Service and node commands
---

| Command | Behavior |
| --- | --- |
| `mdc init DATABASE` | Create a database with its default `main` branch; no service is started. |
| `mdc start DATABASE/BRANCH [--port PORT]` | Start a background service and print its browser URL, port, PID and log path when ready. |
| `mdc stop DATABASE/BRANCH` | Wait for the service and Lean workers to shut down; keep database data and caches. |
| `mdc status` | List all MathDoc database branches and their local service ports. |

Without `--port`, the OS assigns an available port. Explicit ports must be in
1–65535; an occupied port is an error. Services bind to `127.0.0.1` and keep running
after the launching terminal closes. A missing database/branch or an already
running branch fails; start never creates a branch. Stopping an inactive branch
reports that it is not running. All commands work from any directory.
Background startup is encapsulated in `start`; there is no internal CLI subcommand.

Status returns a JSON object mapping `DATABASE/BRANCH` to a status object. The
`port` field is `null` if that branch has no service in the configured cache directory. For example:

```json
{
  "etp/main": { "port": 7600 },
  "mdocs/main": { "port": null }
}
```

An empty inventory returns `{}`. Status queries TerminusDB metadata and requires
no running mdc service. The output is the same in terminals and when redirected.

Help lists commands in one `Commands` section with three groups separated by blank
lines: project/service management, whole-branch operations, and single-node
operations. `metric ior` belongs to the single-node group. Every command and
subcommand includes a short description. The root lists only `-h/--help`.
Command options list `-h/--help` and `-m/--meas` first, then a blank line before
other options. `project` is last in the branch group; `dep` follows `new`.

Branch and node commands require `-p/--proj DATABASE/BRANCH`. Except for `branch del`,
they discover the running service port locally; no URL option or default project is used. The selector can appear
after a client command, such as `mdc graph --proj myproject/main check` or
`mdc graph check --proj myproject/main`. The CLI root and init/start/stop/status
do not accept `-p/--proj`. `-m/--meas` belongs to each command and prints timing measurements to stderr
while keeping JSON on stdout, for example `mdc graph check -p myproject/main -m`.
The root rejects `mdc -m` and `mdc -m graph check`.

```sh
mdc new --proj myproject/main -t 'A theorem'
mdc show --proj myproject/main 'A theorem'
mdc rename --proj myproject/main 'A theorem' 'Renamed theorem'
mdc search --proj myproject/main theorem
mdc export --proj myproject/main > backup.json
mdc import --proj other/main backup.json
mdc branch new --proj myproject/main agent
mdc start myproject/agent
mdc history --proj myproject/agent
mdc stop myproject/agent
mdc branch del --proj myproject/agent
```

Branch creation forks the selected service's current branch head without changing
that service's branch. History returns the latest 50 TerminusDB commits as JSON.

`mdc branch del --proj DATABASE/BRANCH` deletes the selected branch directly in
TerminusDB. Its service must already be stopped; a running or starting service
causes an error that asks you to run `mdc stop DATABASE/BRANCH` first. Deletion
removes all files in that branch's cache directory, including Lean artifacts,
checks, editor workspaces, libraries and logs. Only the empty service lock is kept
for coordination. Other branches and their caches are untouched. TerminusDB's
immutable commits remain in the database; deleting a branch removes its reference.

`mdc edit --proj myproject/main NAME --type TYPE [--revision REV]` reads the complete
block source from stdin. References accept exact names or full UUIDs. Use
`mdc --help` for the installed command surface.

`project show` reads the selected branch's Lean build configuration. `project set`
replaces it from stdin JSON: `toolchain`, `lakefile`, and optional `manifest`. This
configures Lean and external libraries; it does not create or select a database.
