---
title: JSON export and restore
---

`mdc export -p myproject/main` returns a JSON bundle with `nodes`, `project` and
`latex_project`. Nodes keep their UUID, title, `module`, direct dependencies and
source blocks, including block metadata such as certification IDs written back by
collaboration. `project` holds the branch's Lean project settings from the
earlier Lean integration; they are preserved unchanged but not used for
checking. `latex_project` holds the names and contents of the shared macro file
and bibliography. Bundles without `latex_project` import with an empty LaTeX
configuration.

Import and export always cover the whole branch; there is no single-node mode
and no export in the browser. Export needs read access to the branch.

```sh
mdc export -p myproject/main > backup.json
mdc init other
mdc import -p other/main backup.json
```

Import needs `owner` or `admin` on an empty destination branch (`409` otherwise;
a delegated token also needs the `edit` scope) and a bundle with both `nodes`
and `project`. It does not create databases, merge nodes or overwrite data. The
whole bundle is validated before one atomic commit: unknown fields, project
settings, UUIDs, titles, block types, module identities, LaTeX file names and
sizes, missing dependencies and cycles. A rejected import leaves the branch
unchanged. Node revisions depend only on node content, so imported nodes keep
the revisions they had in the exported branch.

An export is one branch snapshot. It contains no history, other branches,
workspace permissions, delegated tokens, proof environments or project boards;
back up the TerminusDB and PostgreSQL volumes for those. Certificates themselves
stay in LeanGround.

Size limit: branch API requests, imports included, accept bodies up to 256 MiB;
larger ones get `413`. Exports have no such limit.
