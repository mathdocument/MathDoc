---
title: Dependency Commands
description: Complete reference for adding, removing, and traversing direct dependencies.
---

## `mdc dep add`

Search interactively and add a direct dependency:

```bash
mdc dep add <source> <query>
mdc dep add <source> <query> -n 50
```

Add one uniquely resolved target without prompting:

```bash
mdc dep add <source> --target <target-ref>
mdc dep add <source> -t <target-ref>
```

`<target-ref>` may be an exact fnode, unique fnode prefix, or `.mdoc` path. The full
target fnode is always stored. Adding an existing direct dependency is an idempotent
success; self-dependencies, ambiguous references, missing targets, duplicate identities,
and cycle-creating edges are rejected.

If interactive search has no match, the command can create a new note and link it in
one recoverable operation. Creation is not offered when matches exist but are excluded
because they are the source, an existing dependency, or otherwise invalid.

Before the final write, MathDoc strongly refreshes the complete graph under the
workspace lock and repeats duplicate, snapshot, and cycle checks.

## `mdc dep rm`

Select one or more direct dependencies interactively:

```bash
mdc dep rm <source>
```

Remove one dependency without prompting:

```bash
mdc dep rm <source> --target <target-ref>
mdc dep rm <source> -t <target-ref>
```

Removal first resolves exact values and unique prefixes among the source's direct
dependency values. This allows a dangling dependency to be removed after its target
file has disappeared. Paths are accepted for targets that still exist. Ambiguous and
non-direct targets are rejected.

## `mdc dep show`

Traverse forward from a node into its prerequisites:

```bash
mdc dep show <source>
mdc dep show <source> -d -1
```

`-d, --depth` defaults to `1`. Zero traverses no edges, positive values set a hop limit,
and `-1` enables unlimited traversal. Values below `-1` are rejected. The command
refreshes the reachable subgraph from the source and exits `1` if that subgraph contains
a cycle.

## `mdc dep leaf`

List all reachable nodes with no further dependency:

```bash
mdc dep leaf <source>
```

The command refreshes the source's reachable subgraph and exits `1` if it encounters a
cycle.

## `mdc dep refs`

Traverse reverse dependencies: nodes that depend on the target.

```bash
mdc dep refs <target>
mdc dep refs <target> -d -1
```

Depth follows the same rules as `dep show` and defaults to one hop. The target path is
refreshed before reverse edges are queried.
