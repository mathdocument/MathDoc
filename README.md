# MathDoc

`MathDoc` is a math knowledge management tool with a command-line interface exposed as `mdc`, which manages `.mdoc` files inside an initialized workspace with a `.mdc/` directory.

Each `.mdoc` file owns a unique `fnode` id and represents a small chunk of knowledge that can be supplemented by plain text and source blocks. `.mdoc` files can declare other files as dependencies; circular dependencies are forbidden.

## Build

```bash
cargo build --release
# binary: target/release/mdc
```

## References to `.mdoc` files

References accepted by `mdc` commands:
- a `.mdoc` path (absolute, relative to cwd, or relative to workspace root)
- a full `fnode` UUID
- a unique `fnode` prefix (first 8 chars are sufficient)

## Workflow example

```bash
mdc init
mdc new -t "Root Note" -f notes/root-note
mdc new -t "Background Lemma" -f notes/background-lemma
# use the path or fnode reported by `mdc new` as <ref>
mdc dep add notes/root-note.mdoc background
mdc dep show notes/root-note.mdoc -d -1
mdc dep leaf notes/root-note.mdoc
mdc eval notes/root-note.mdoc
mdc graph check
mdc graph tui
```

## Command Reference

### Workspace Commands

#### `mdc init`

Initialize the current directory as a MathDoc workspace:
```bash
mdc init
```
Creates a `.mdc/` folder. Nested MathDoc workspaces are allowed but managed separately.

#### `mdc new`

Create a new `.mdoc` file:
```bash
mdc new -t "Matrix Rank"
mdc new -t "Matrix Rank" -f notes/matrix-rank
```
- `-t, --title`: title of the new mdoc (default: `Untitled`)
- `-f, --file`: relative output path without `.mdoc` suffix; default creates `<fnode>.mdoc` at workspace root
- the `.mdoc` suffix is always appended automatically

#### `mdc sync`

Force-refresh the entire workspace index:
```bash
mdc sync
```

#### `mdc search`

Search mdocs by title or fnode:
```bash
mdc search <query>
mdc search <query> -n 20
```
- `-n, --max-results`: maximum results to display (default: `200`)

### Dependency Commands

#### `mdc dep add`

Interactively search for candidates and add them as direct dependencies:
```bash
mdc dep add <source> <query>
mdc dep add <source> <query> -n 50
```
- Interactive selection in the terminal (enter comma-separated numbers or ranges)

#### `mdc dep rm`

Interactively remove direct dependencies:
```bash
mdc dep rm <source>
```

#### `mdc dep show`

Show forward dependencies of a mdoc:
```bash
mdc dep show <source>
mdc dep show <source> -d -1
```
- `-d, --depth`: traversal depth (default: `1`; `-1` = unlimited)
- Runs incremental workspace discovery automatically

#### `mdc dep leaf`

Show all reachable leaf dependencies (nodes with no further deps):
```bash
mdc dep leaf <source>
```

#### `mdc dep refs`

Show reverse dependencies (who depends on this node):
```bash
mdc dep refs <target>
mdc dep refs <target> -d -1
```
- `-d, --depth`: traversal depth (default: `1`)

### Graph Commands

#### `mdc graph check`

Scan the workspace and report repository-wide issues:
```bash
mdc graph check
```
Reports: node/edge counts, missing targets, invalid files, dependency cycles.

#### `mdc graph roots`

Show all global root nodes (no incoming dependencies), sorted by topo height (deepest dependency tree first):
```bash
mdc graph roots
```

#### `mdc graph tui`

Open the interactive graph browser:
```bash
mdc graph tui
mdc graph tui <ref>
```
- Defaults to the highest root node (largest dependency tree)
- `<ref>`: start at a specific node (fnode prefix, path, or title)


### Evaluation Commands

#### `mdc eval`

Compile and run all source blocks in a mdoc and its dependency tree:
```bash
mdc eval <source>
mdc eval <source> -d -1
```
- `-d, --depth`: dependency traversal depth (default: `1`)
- Merges ancestor blocks of the same srctype in topological order before compilation (for srctypes with `depens = true`)
- Current supported srctypes: `natl`, `latex`, `lean`, `py`

#### Source block format

```
@src: natl
plain text content
@end

@src: py
print("hello")
@end
```

#### Compiler configuration

Edit `.mdc/config.toml` to configure compiler behavior. Effective defaults (built into the binary):

| Setting          | natl | latex    | lean     | py    |
| ---------------- | ---- | -------- | -------- | ----- |
| `depens`         | true | true     | true     | false |
| `reverse_depens` | true | true     | true     | true  |
| `timeout_sec`    | —    | required | required | 30s   |

For `latex`, `preamble` and `postamble` are **required** in config.toml (no built-in default). Example:

```toml
[src.latex]
timeout_sec = 60
preamble = """
\\documentclass{article}
\\begin{document}
"""
postamble = """
\\end{document}
"""
```
