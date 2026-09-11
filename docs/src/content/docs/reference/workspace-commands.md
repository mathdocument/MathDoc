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

Status has **Project** (`DATABASE/BRANCH`) and **Port** columns. Headers are bold
in terminals and plain when redirected. Port is blank when no service is running
for that branch in the configured cache directory. Status queries TerminusDB
metadata and requires no running mdc service.

All client commands require `--proj DATABASE/BRANCH`. They discover the service
port locally; no URL option or default project is used. The selector can appear
before or after the client subcommand. Do not pass `--proj` to init/start/stop/status.

```sh
mdc --proj myproject/main new -t 'A theorem'
mdc --proj myproject/main show 'A theorem'
mdc --proj myproject/main rename 'A theorem' 'Renamed theorem'
mdc --proj myproject/main search theorem
mdc --proj myproject/main export > backup.json
mdc --proj other/main import backup.json
mdc --proj myproject/main branch create agent
mdc start myproject/agent
mdc --proj myproject/agent history
```

Branch creation forks the selected service's current branch head without changing
that service's branch. History returns the latest 50 TerminusDB commits as JSON.

`mdc --proj myproject/main edit NAME --type TYPE [--revision REV]` reads the complete
block source from stdin. References accept exact names or full UUIDs. Use
`mdc --help` for the installed command surface.
