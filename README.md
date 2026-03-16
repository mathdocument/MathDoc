# MathDoc

`MathDoc` is a math knowledge management tool with a command-line interface exposed as `mdc`, which manages `.mdoc` files inside an initialized workspace with a `.mdc/` directory.

Each `.mdoc` file owns a unique `fnode` id and represents a small chunk of knowledge that can be supplemented by plain texts and code blocks. `.mdoc` files can refer other as dependencies, circular dependencies are forbidden.

References to `.mdoc` files accepted by `mdc` commands are
- a `.mdoc` path
- a full `fnode`
- a unique `fnode` prefix



## Workflow example

```bash
mdc init
mdc new -t "Root Note" -f notes/root-note
mdc edit notes/root-note.mdoc
mdc new -t "Background Lemma" -f notes/background-lemma
# use the created path or fnode reported by `mdc new` as <root>
mdc dep add <root> background
mdc dep show <root> -d -1
mdc dep leaf <root>
mdc eval <root>
mdc graph check
```

## Command Reference

### Workspace Commands

#### `mdc init`

Initialize the current directory as a MathDoc workspace:
```bash
mdc init
```
This will create a `.mdc` folder in your current directory, nested MathDoc workspaces are allowed but what they manage will be separated.

#### `mdc new`

Create a new `.mdoc` file:
```bash
mdc new -t "Matrix Rank"
mdc new -t "Matrix Rank" -f notes/matrix-rank
```
- `-t, --title`: title of the new mdoc, default is `Untitled`
- `-f, --file`: relative output file path without the forced `.mdoc` suffix
- default behavior creates `<fnode>.mdoc` at the mdoc root
- the `.mdoc` suffix is always appended; for example `-f data/ok.mdoc` creates `data/ok.mdoc.mdoc`


#### `mdc edit`

Open a mdoc in `$EDITOR`, then refresh its index in search cache:
```bash
mdc edit <source>
```

#### `mdc sync`

Force-refresh the whole search cache:
```bash
mdc sync
```

#### `mdc search`

Search mdocs:
```bash
mdc search <query>
mdc search <query> --max-results 20
```
- `-n, --max-results`: maximum number of results to display, default is `200`

Examples:
```bash
mdc search matrix
mdc search a1b2c3d4
```

### Dependency Commands

#### `mdc dep add`

Interactively search for candidate nodes and add them as direct dependencies of a source mdoc:

```bash
mdc dep add <source> <query>
mdc dep add <source> <query> --max-results 50
```
- interactive selection in the terminal
- `-n, --max-results`: maximum number of candidates to display, default is `200`
- if there are no matches and the command is run in a TTY, `mdc dep add` prompts whether to create a new mdoc and add it immediately as a dependency
- cycle-producing dependency additions are rejected before the file is written


#### `mdc dep rm`
Interactively remove direct dependencies from a mdoc:

```bash
mdc dep rm <source>
```

- interactive selection in the terminal
- broken dependency rows can also be removed

#### `mdc dep show`

Show forward dependencies of a mdoc:
```bash
mdc dep show <source>
mdc dep show <source> --depth -1
mdc dep show <source> --refresh
```
- `-d, --depth`: traversal depth, default is `1`, use `-1` for unlimited traversal
- default mode reads the cached dependency graph
- `--refresh` first runs workspace discovery for new/deleted/renamed `.mdoc` paths, then refreshes the reachable cached subgraph
- use `mdc sync` when you want a full repository refresh

#### `mdc dep leaf`

Show all reachable leaf dependencies of a mdoc:
```bash
mdc dep leaf <source>
mdc dep leaf <source> --refresh
```
- traverses the full reachable dependency graph
- only prints nodes that have no downstream dependencies
- shared leaves are deduplicated
- default mode reads the cached dependency graph
- `--refresh` first runs workspace discovery for new/deleted/renamed `.mdoc` paths, then refreshes the reachable cached subgraph
- use `mdc sync` when you want a full repository refresh

#### `mdc dep refs`

Show referrers of a target mdoc:
```bash
mdc dep refs <target>
mdc dep refs <target> --depth -1
mdc dep refs <target> --refresh
```
- `-d, --depth`: traversal depth, default is `1`, use `-1` for unlimited traversal
- by default reads the cached graph index
- use `--refresh` or `mdc sync` after external file edits


#### `mdc graph check`

Inspect the global dependency graph for repository-wide issues:
```bash
mdc graph check
mdc graph check --full
```
This scan reports:
- total number of `.mdoc` files
- total number of dependency relations
- missing dependency targets
- invalid `.mdoc` files
- dependency cycles
- default mode reads the cached graph index
- use `--full` to refresh the workspace before checking, or `mdc sync` beforehand

#### `mdc graph roots`

Show all global root nodes that are not depended on by any other node:
```bash
mdc graph roots
mdc graph roots --refresh
```
This scan reports:
- includes valid nodes and invalid files that currently have no incoming dependencies
- excludes missing dependency placeholders
- shows each root's weakly connected component size, excluding missing placeholders
- sorts roots by component size descending
- default mode reads the cached graph index
- use `--refresh` or `mdc sync` after external file edits


### Evaluation Commands

#### `mdc eval`

One can write codeblocks in `.mdoc` files of the form
```mdoc
@src: [type]
@end
```
where [type] for now could be natl/latex/lean/python.

`mdc eval` will first merge all the dependent-enabled code blocks of the dependencies of a given source `.mdoc` file in topological order, and then compile them according to compiling configurations:
```bash
mdc eval <source>
mdc eval <source> --depth -1
```
- `-d, --depth`: dependency traversal depth, default is `1`, use `-1` for unlimited depth
- dependency preflight starts with workspace discovery, then uses the cached reachable graph with targeted refresh of currently reachable dependency rows
- actual execution still loads the real reachable `.mdoc` files and block content from disk

To modify the default compiling behavior of `mdc eval`, edit `.mdc/config.toml`. The default compiling configuration is
```toml
[src.natl]
depens = true

[src.latex]
depens = true
preamble = '''
\documentclass{article}
\begin{document}
'''
postamble = '''
\end{document}
'''

[src.py]
depens = false
```
