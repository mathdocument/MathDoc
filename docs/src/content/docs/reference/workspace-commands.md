---
title: Service and node commands
---

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
