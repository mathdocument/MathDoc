---
title: JSON export and restore
---

`mdc export -p myproject/main` produces a JSON bundle with `nodes` and `project`. Nodes retain their UUID, title, stable Lean module, direct dependencies and source blocks. `project` contains the toolchain, Lake configuration and lock manifest.

Import and export always operate on the entire selected branch graph. There is no
single-node mode. Bundles contain no compiler caches or `.olean` files.

```sh
mdc export -p myproject/main > backup.json
mdc init other
mdc start other/main
mdc import -p other/main backup.json
```

Import requires a running, empty destination branch and a bundle containing both
`nodes` and a non-null `project`. It does not create a database, merge nodes or
overwrite existing data. The whole bundle is validated before a single atomic
commit: project configuration, UUIDs, supported block types, module identities,
missing dependencies and cycles. Rejected imports leave the branch unchanged.
A graph with only one node is still a valid complete graph.

Export captures the current branch snapshot. It includes neither history nor
other branches; preserve the TerminusDB volume for a complete database backup.
Both export and import currently hold the bundle in memory. The HTTP import body
limit is 256 MiB, so larger exports cannot be restored through `mdc import`.

The runtime accepts JSON bundles only, not `.mdoc` directories. Convert an old
workspace separately and verify the imported export before removing its files.
