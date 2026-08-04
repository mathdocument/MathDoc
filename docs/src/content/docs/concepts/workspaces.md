---
title: Workspaces & References
description: How MathDoc discovers workspaces, organizes generated state, and resolves node references.
---

A MathDoc workspace is a directory tree containing a real, non-symlink `.mdc/` control
directory. `mdc init` creates it; every other command discovers it by walking upward
from the current directory.

## Discovery boundaries

Workspace scans skip:

- the `.mdc/` control directory itself
- nested MathDoc workspaces
- symlinked directories
- symlinked `.mdoc` entries

The workspace root and `.mdc/` must not be reached through unsafe symlink layouts.
Mutation commands additionally validate that target paths stay inside the workspace
and do not enter `.mdc/` or a nested workspace.

## Control directory layout

```text
.mdc/
  config.toml                 # compiler settings
  index.db                    # managed SQLite index
  mutation.lock               # cooperative workspace mutation lock
  source-blocks.json          # mirror synchronization baseline
  text/
    Lib/notes/theorem.txt
  latex/
    Lib/notes/theorem.tex
    Lib.tex                   # generated selected input
    Main.tex                  # user-editable template
    Main.pdf                  # latexmk output
  lean/
    Lib/notes/theorem.lean
    Lib.lean                  # generated selected import
    lakefile.toml
    lean-toolchain
    .lake/build/
  python/
    Lib/notes/theorem.py
  rocq/
    Lib/notes/theorem.v
    build/notes/theorem.vo
    _CoqProject
```

This is a representative layout. SQLite sidecars, compiler state, temporary files, and
other managed or ephemeral artifacts may also appear.

The `Lib/` directories are editable working trees. They preserve the relative path of
each source `.mdoc` and keep editable content separate from compiler artifacts.

:::note
The index, manifest, generated drivers, and compiler outputs are managed state. Keep
your `.mdoc` files in version control; decide separately whether language project files
such as `lakefile.toml`, `lean-toolchain`, or a customized `Main.tex` belong in your
workspace repository.
:::

## References

Most commands accept a `<ref>` that identifies a node in one of three ways:

| Form | Examples |
| --- | --- |
| Path | `notes/theorem`, `notes/theorem.mdoc`, `./theorem.mdoc`, an absolute path |
| Exact fnode | `550e8400-e29b-41d4-a716-446655440000` |
| Unique fnode prefix | `550e8400` or any other unambiguous case-insensitive prefix |

Paths may be absolute, relative to the current directory, or relative to the workspace
root. If a supplied path has no extension, MathDoc appends `.mdoc`; dotted basenames
such as `foo.bar.mdoc` must therefore retain the suffix. Extensionless bare values are
also tried as paths before fnode resolution. There is no minimum length for a unique
fnode prefix.

`mdc serve [source]` and `mdc graph tui [source]` additionally accept a unique,
case-insensitive exact title for their initial node. Other commands deliberately retain
the stricter path/fnode/prefix contract.

## Managed writes

MathDoc uses snapshot-checked, descriptor-relative file operations and a cooperative
workspace lock. Replacing an existing file temporarily quarantines the old generation
before installing the new one, so the pathname may be briefly absent.

Multi-file operations attempt reverse-order rollback if a later step fails. Rollback
is best-effort rather than crash-atomic, and the workspace lock cannot stop an arbitrary
external editor from racing a MathDoc process. Conflicts are reported instead of
blindly replacing an uncertain file generation.
