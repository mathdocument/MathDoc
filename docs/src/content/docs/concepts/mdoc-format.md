---
title: Portable .mdoc format
---

`.mdoc` is an explicit import/export format. It is not the active source of truth.

```text
@fnode: 00000000-0000-4000-8000-000000000001
@title: Example
@src: text
A short explanation.
@end
@src: lean
theorem exampleA : True := by trivial
@end
```

Supported source blocks are text, lean, rocq and latex. Export one node with `mdc export NAME`; export a complete database bundle, including stable module identities and project configuration, with `mdc export`. JSON bundles are the complete portable backup format.

`mdc import PATH` accepts a JSON bundle or a legacy workspace directory. It preserves valid UUIDs and dependencies, derives legacy module identities, and never changes input files. Imports reject existing UUIDs, unsupported types and invalid graphs. Project import requires an empty database.
