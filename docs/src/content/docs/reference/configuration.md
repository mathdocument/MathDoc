---
title: Configuration & Profiling
description: Configure compiler deadlines and inspect command-level performance.
---

## Workspace configuration

`mdc init` writes `.mdc/config.toml` with every available setting commented out.
Uncomment only the values that should override built-in defaults.

```toml title=".mdc/config.toml"
[src.latex]
timeout_sec = 60

[src.lean]
timeout_sec = 300
setup_timeout_sec = 1800
```

Built-in defaults are:

| Setting | `text` | `latex` | `python` | `lean` | `rocq` |
| --- | --- | --- | --- | --- | --- |
| `timeout_sec` | - | 30 | 30 | 300 | 300 |
| `setup_timeout_sec` | - | - | - | 1800 | - |

All timeout values are positive integer seconds.

- `timeout_sec` applies to each language compiler subprocess.
- Lean's `setup_timeout_sec` gives `lake init` and `lake env lean --version` separate
  deadlines when either managed setup file is missing.
- If both Lean setup files already exist, no setup subprocess runs.

Configuration loading reports malformed values with their source section. Defaults and
overrides are merged into typed positive durations before a compiler request is built;
individual compiler implementations do not read TOML or apply defaults themselves.
Unknown `[src.<type>]` names are rejected, but unknown top-level and per-source setting
names are currently ignored. Check setting names carefully: a typo can leave the
built-in default active without an error.

## Inclusive profiling

Every command accepts the global `--prof` flag:

```bash
mdc graph check --prof
mdc metric ior notes/theorem.mdoc --prof
```

Profiling writes an inclusive elapsed-time tree to standard error while preserving the
command's normal standard output. This is particularly useful for full refresh, graph,
and synchronization costs.

The current scopes cover CLI dispatch, cache open/bootstrap, workspace scans, digest
calculation, issue construction, bulk row replacement, in-degree rebuild, derived graph
algorithms, selected SQLite commits, and parent-level parallel read phases. Parallel
scan workers do not currently emit separate profiling trees.

:::note
Scopes are inclusive: a parent's duration contains all reported child durations. Do not
sum an entire report as though every line represented disjoint work.
:::
