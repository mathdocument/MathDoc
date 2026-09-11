---
title: Databases and references
---

## Projects, branches and services

A MathDoc project is a TerminusDB database. `mdc init NAME` creates `NAME/main`;
branches within that database have independent graph heads and share database
history. Each running mdc service serves exactly one `DATABASE/BRANCH`. The CLI
and browser use that service for authoring. `mdc status` lists branch names and
ports, including stopped branches.

Several projects can use the same TerminusDB server. They remain separate
databases, with separate graphs and Lean settings. Several branches can run at
once on distinct ports. The operating system schedules their native Lean
processes; mdc does not pin workers to CPU cores or impose a machine-wide worker
budget. Every service owns its branch cache using an OS file lock.

## Where data lives

| Data | Location and lifetime |
| --- | --- |
| Nodes, source blocks, graph links, Lean project settings, history | TerminusDB storage; durable and independent of service processes. |
| Database credentials and host settings | Private user configuration or environment; outside graph exports. |
| Generated Lean sources, libraries, artifacts, certificates, logs, service record | Local cache, separated by endpoint/database/branch; disposable after stopping the service. |
| Unsaved browser drafts | Browser/editor session; save before closing or restarting. |

The supplied Compose deployment mounts Docker volume `mathdoc-terminus-data` at
`/app/terminusdb/storage`. On Docker Desktop this volume lives inside Docker's
Linux VM, not in a MathDoc project directory. `docker volume inspect
mathdoc-terminus-data` reports Docker's volume metadata. Preserve this volume for
complete database history; a graph export captures only one branch snapshot.

Caches default to `~/.cache/mdc/ENDPOINT_HASH/DATABASE/BRANCH`, with XDG and explicit
overrides described in [Configuration](../../reference/configuration/). CLI and
browser Lean sessions reuse Lake artifacts inside this branch cache. Different
branches have isolated artifacts; creating a branch does not copy its parent's
`.olean` files. Toolchains installed by Elan remain host-level installations.

Use one cache root consistently for all commands on a host. Service discovery and
the lock are scoped to that root; a different root cannot see or stop the old
service. `stop` retains caches; `branch del` requires a stopped branch and removes
its cache contents, leaving only the service lock for coordination.

No project folder or source mirror is required. Generated Lean files are owned
by the service: editing them does not edit nodes. Import/export are explicit
operations; ordinary commands never scan workspace files.

## Node references

A node reference is its exact display name or complete UUID. A duplicate display
name is ambiguous and requires a UUID. Paths, filenames and UUID prefixes are
not references. Each node also has a stable Lean module identity such as
`Lib.N_<uuid_without_hyphens>`; renaming the display title preserves that module.
