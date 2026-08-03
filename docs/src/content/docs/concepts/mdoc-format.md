---
title: The .mdoc Format
description: The identity, dependency, source block, and serialization rules of MathDoc files.
---

An `.mdoc` file is a UTF-8 plain-text document with a stable identity, a title, direct
dependencies, and typed source blocks.

```text title="notes/theorem.mdoc"
@fnode: 550e8400-e29b-41d4-a716-446655440000
@title: My Lemma

@dep:
a1b2c3d4-e5f6-7890-abcd-ef1234567890
@end

@src: latex
\begin{lemma}
  $1 + 1 = 2$.
\end{lemma}
@end

@src: text
Informal explanation here.
@end
```

## Identity and title

`@fnode` is the stable, workspace-unique identity of the node. `mdc new` generates a
lowercase UUID v4. The parser accepts lowercase ASCII letters and digits separated by
internal hyphens; the familiar eight-character value shown in parts of the UI is only
a short display convention.

`@title` is human-readable display text. Search uses both the title and fnode, but links
between documents never depend on the title.

## Dependencies

The optional `@dep:` block lists exact full fnodes of direct prerequisites, one per
line. It is omitted when the list is empty.

```text
@dep:
550e8400-e29b-41d4-a716-446655440000
ccfbb1e7-3a8f-4991-8463-c4cd6593a427
@end
```

Dependencies retain their original order. CLI mutation commands prevent self-links,
duplicate links, and cycle-creating edges, but direct text edits can still introduce
invalid graph state. Use `mdc graph check` as the authoritative validation pass.

## Source blocks

A source block begins with `@src: <srctype>` and ends at the next trimmed `@end` line.
The built-in types are:

- `text` for informal exposition
- `latex` for typeset mathematical source
- `python` for computation
- `lean` for Lean formalization
- `rocq` for Rocq formalization

A file may contain at most one block of each source type. Type names are parsed
case-insensitively and serialized in their canonical lowercase form.

Source headers may include shell-like quoted `key=value` metadata:

```text
@src: lean module="Algebra.Main"
```

Metadata is parsed and preserved, but current compilers do not act on it.

:::caution[Block terminator]
Because a trimmed `@end` terminates the current block, source content cannot contain a
line whose trimmed value is exactly `@end`.
:::

## Encoding and normalization

Files must be valid UTF-8. MathDoc validates the complete structure during parsing,
including source headers and block termination. Source mutation APIs normalize
nonempty content to end with a newline, matching parser output.

Broken files are retained on disk and reported by graph checks. When possible, the
index recovers their fnode and title so diagnostics remain useful.
