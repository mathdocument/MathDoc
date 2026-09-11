---
title: JSON export and restore
---

`mdc export --proj myproject/main` produces a JSON bundle with `nodes` and `project`. Nodes retain their UUID, title, stable Lean module, direct dependencies and source blocks. `project` contains the toolchain, Lake configuration and lock manifest.

Import and export always operate on the entire selected branch graph. There is no
single-node mode. Bundles contain no compiler caches or `.olean` files.

```sh
mdc export --proj myproject/main > backup.json
mdc init other
mdc start other/main
mdc import --proj other/main backup.json
```

Import requires a running, empty destination branch and a bundle containing both
`nodes` and a non-null `project`. It does not create a database, merge nodes or
overwrite existing data. The whole bundle is validated before a single atomic
commit: project configuration, UUIDs, supported block types, module identities,
missing dependencies and cycles. Rejected imports leave the branch unchanged.
A graph with only one node is still a valid complete graph.

Export captures the current branch snapshot. Preserve the TerminusDB volume for
complete history and other branches.

The runtime accepts JSON bundles only. Legacy `.mdoc` workspaces must first be converted by the separate migration utility. Keep old files and any differing source mirrors until the imported database export has been checked against them.
