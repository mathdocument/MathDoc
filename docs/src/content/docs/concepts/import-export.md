---
title: JSON export and restore
---

`mdc export -p myproject/main` returns a JSON bundle with `nodes`, `project` and
`latex_project`. Nodes keep their UUID, title, `module`, direct dependencies and
source blocks, including block metadata such as certification IDs written back by
collaboration. `project` holds the branch's Lean project settings from the
earlier Lean integration; they are preserved unchanged but no longer used for
checking. `latex_project` holds the shared macro file and bibliography. Bundles
without `latex_project` import with an empty LaTeX configuration.

Import and export always cover the whole branch; there is no single-node mode.

```sh
mdc export -p myproject/main > backup.json
mdc init other
mdc import -p other/main backup.json
```

Import needs `owner` or `admin` on an empty destination branch and a bundle with
both `nodes` and `project`. It does not create databases, merge nodes or
overwrite data. The whole bundle is validated before one atomic commit: project
settings, UUIDs, block types, module identities, missing dependencies and cycles.
A rejected import leaves the branch unchanged.

An export is one branch snapshot. It contains no history, other branches,
workspace permissions, proof environments or collaboration projects; back up the
TerminusDB and PostgreSQL volumes for those. Certificates themselves stay in
LeanGround.

Size limit: the API currently accepts request bodies up to 2,200,000 bytes, so
`mdc import` cannot restore larger bundles. Exports have no such limit.
