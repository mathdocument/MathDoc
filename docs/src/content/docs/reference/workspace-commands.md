---
title: Workspace Commands
description: Complete reference for init, new, edit, sync, search, and serve.
---

## `mdc init`

Initialize the current directory as a MathDoc workspace:

```bash
mdc init
```

The command creates a real `.mdc/` directory and writes `config.toml` with commented
compiler defaults. It does not create or open the SQLite index; the first command that
needs the index performs its bootstrap.

## `mdc new`

Create a new `.mdoc` file:

```bash
mdc new -t "Matrix Rank"
mdc new -t "Matrix Rank" -f notes/matrix-rank
```

| Option | Meaning |
| --- | --- |
| `-t, --title <title>` | Display title; defaults to `Untitled` |
| `-f, --file <path>` | Workspace-relative output path, with or without `.mdoc` |

Without `--file`, the output is `<fnode>.mdoc` at the workspace root. Absolute paths,
escaping paths, `.mdc/` targets, symlinked paths, nested-workspace targets, and existing
files are rejected.

Workspace path validation is broader than compiler-specific path validation. LaTeX
mirror paths must be UTF-8 and cannot contain `"`, `{`, `}`, `%`, carriage returns, or
line feeds. Lean path components must be UTF-8 and cannot contain `«`, `»`, carriage
returns, or line feeds.

Creation strongly refreshes the index under the workspace mutation lock, checks
identity uniqueness, creates missing parent directories safely, and indexes the new
node. If index update fails after file creation, MathDoc attempts both file rollback
and path-index repair; recovery failures are included in the reported error.

## `mdc edit`

Open a document in `$EDITOR`:

```bash
mdc edit <ref>
```

When `$EDITOR` is unset, MathDoc uses `vi`. The environment value is interpreted as one
executable, not as a shell command with arguments. The edited file is reindexed only
after the editor exits successfully.

## `mdc sync`

Strongly refresh the workspace and reconcile every parseable `.mdoc` with its five
language mirror trees:

```bash
mdc sync
```

A clean mirror is updated when its mdoc block changes. A dirty mirror is preserved for
`mdc back`. If both sides changed differently, both versions are preserved and a
conflict is reported. Outputs for deleted or renamed documents are removed only while
they still match their baseline.

Structurally parseable files with duplicate fnodes are exported because duplication is
an index/graph issue. Structurally unparseable documents retain previous mirrors. If any
unparseable document exists, orphan cleanup is deferred because an apparent orphan may
be the old path of that document.

Reconciliation is per source/type pair. Unrelated clean changes can commit even when
another pair conflicts. The command exits `1` while dirty mirrors or conflicts remain.

## `mdc search`

Search indexed titles and fnodes:

```bash
mdc search <query>
mdc search <query> -n 20
```

`-n, --max-results` sets the result limit and defaults to `200`. Results use the
canonical ranked index search. Invalid or duplicate nodes can appear when their
identity is recoverable; those summaries are marked broken.

Before searching, MathDoc discovers workspace additions, removals, renames, and
metadata-changed files. An external same-size edit that retains the same timestamp may
not be reparsed until a strong refresh such as `mdc sync` or `mdc graph check`.

## `mdc serve`

Start the local browser interface:

```bash
mdc serve
mdc serve notes/theorem.mdoc
mdc serve --bind 127.0.0.1:7878 --no-open
```

| Argument or option | Meaning |
| --- | --- |
| `source` | Optional start reference or unique case-insensitive exact title |
| `--bind <address>` | Numeric loopback address; defaults to `127.0.0.1:0` |
| `--no-open` | Do not launch the default browser |

Port `0` asks the operating system for a free port. Binding is restricted to numeric
loopback addresses. See the [web interface guide](../concepts/web-interface/) for UI
features and its local-only security model.
