---
title: Service and node commands
---

`mdc init` initializes a TerminusDB database. `mdc serve --bind 127.0.0.1:7599` runs its local service. Global `--database` and `--branch` select the served database branch; clients select their service with `--url` or `MDC_URL`.

```sh
mdc new -t 'A theorem'
mdc show 'A theorem'
mdc rename 'A theorem' 'Renamed theorem'
mdc search theorem
mdc export > backup.json
mdc import backup.json
```

`mdc edit NAME --type TYPE [--revision REV]` reads the complete block source from stdin. `sync`, `back`, path references, terminal editing and source mirrors are removed. Use `mdc --help` for the installed command surface.
