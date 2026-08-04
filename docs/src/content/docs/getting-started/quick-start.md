---
title: Quick Start
description: Create a small connected workspace and run the complete source workflow.
---

This walkthrough creates two mathematical nodes, links them, and opens the result in
MathDoc's local web interface.

## 1. Initialize a workspace

```bash
mkdir my-mathematics
cd my-mathematics
mdc init
```

Commands locate this workspace by walking upward until they find its `.mdc/` control
directory, so they can be run from nested folders.

## 2. Create two nodes

```bash
mdc new -t "Background Lemma" -f notes/background
mdc new -t "Main Theorem" -f notes/theorem
```

Each command creates a plain-text `.mdoc` file with a workspace-unique UUID. The path
organizes the file; its `@fnode` identity remains stable if the file is moved.

## 3. Connect the theorem to its prerequisite

```bash
mdc dep add notes/theorem --target notes/background
mdc dep show notes/theorem
```

Dependencies point from a node to the direct knowledge it requires. MathDoc resolves
the path and stores the prerequisite's full `@fnode`, rather than its current filename.

## 4. Add source material

Open `notes/theorem.mdoc` and add a source block:

```text title="notes/theorem.mdoc"
@src: text
The main theorem follows from the background lemma.
@end
```

You can edit `.mdoc` directly, or export stable language-specific files first:

```bash
mdc sync
```

For example, the theorem's Lean mirror is
`.mdc/lean/Lib/notes/theorem.lean`. Missing source blocks intentionally produce empty
mirror files, so each node has a predictable five-language layout.

## 5. Compile and bring changes back

```bash
mdc work notes/theorem
mdc back
```

`work` reconciles the workspace and invokes each native compiler represented by the
selected node. `back` writes changed mirrors into the matching `.mdoc` source blocks.
Both directions compare against a stored baseline and preserve independently edited
versions instead of silently overwriting them.

## 6. Inspect the graph

```bash
mdc graph check
mdc graph tui notes/theorem
```

Or start the local visual interface:

```bash
mdc serve notes/theorem
```

The browser interface can navigate and edit nodes, manage dependencies, create new
nodes, and display the full graph in a deterministic depth-layered layout.

:::tip
Run `mdc graph check` in continuous integration to catch malformed files, duplicate
identities, missing targets, and dependency cycles introduced by direct file edits.
:::
