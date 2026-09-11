---
title: JSON export and restore
---

`mdc export --proj myproject/main` produces a JSON bundle with `nodes` and `project`. Nodes retain their UUID, title, stable Lean module, direct dependencies and source blocks. `project` contains the toolchain, Lake configuration and lock manifest.

`mdc export --proj myproject/main NAME` produces a single-node JSON bundle with `project: null`. The node's dependencies must already exist in the destination database or be included in a combined bundle.

```sh
mdc export --proj myproject/main > backup.json
mdc import --proj other/main backup.json
```

Import validates UUIDs, supported block types, module identities and graph constraints. Existing UUIDs cannot be overwritten. Project settings require an empty destination database. This exports the current snapshot; preserve the TerminusDB volume for complete history and branches.

The runtime accepts JSON bundles only. Legacy `.mdoc` workspaces must first be converted by the separate migration utility. Keep old files and any differing source mirrors until the imported database export has been checked against them.
