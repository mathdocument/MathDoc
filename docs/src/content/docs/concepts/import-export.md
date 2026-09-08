---
title: JSON export and restore
---

`mdc export` produces a JSON bundle with `nodes` and `project`. Nodes retain their UUID, title, stable Lean module, direct dependencies and source blocks. `project` contains the toolchain, Lake configuration and lock manifest.

`mdc export NAME` produces a single-node JSON bundle with `project: null`. The node's dependencies must already exist in the destination database or be included in a combined bundle.

```sh
mdc export > backup.json
mdc --url http://127.0.0.1:7600 import backup.json
```

Import validates UUIDs, supported block types, module identities and graph constraints. Existing UUIDs cannot be overwritten. Project settings require an empty destination database. This exports the current snapshot; preserve the TerminusDB volume for complete history and branches.

The runtime accepts JSON bundles only. Legacy `.mdoc` workspaces must first be converted by the separate migration utility. Keep old files and any differing source mirrors until the imported database export has been checked against them.
