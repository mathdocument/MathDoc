---
title: Service and node commands
---

`mdc status` lists all MathDoc project branches from the configured TerminusDB,
including projects without a running service. Its table has **Project**
(`DATABASE/BRANCH`) and **Port** columns. Headers are bold in terminals and plain
when redirected. Port is blank when no service is running for that branch in the
configured cache directory. The command needs database credentials but no mdc
HTTP service; `--url` is not applicable. Existing services must run a version with
status support to publish their port.

`mdc init DATABASE` creates a new project database on an existing TerminusDB instance. `mdc serve DATABASE [--branch main] [--bind 127.0.0.1:7599]` serves that branch. Both run from any directory. Ordinary clients select their service with `--url`, `MDC_URL`, or the user configuration; database flags are not accepted on client commands.

```sh
mdc new -t 'A theorem'
mdc show 'A theorem'
mdc rename 'A theorem' 'Renamed theorem'
mdc search theorem
mdc export > backup.json
mdc import backup.json
```

`mdc edit NAME --type TYPE [--revision REV]` reads the complete block source from stdin. `sync`, `back`, path references, terminal editing and source mirrors are removed. Use `mdc --help` for the installed command surface.
