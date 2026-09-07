---
title: Databases and references
---

A MathDoc project is a TerminusDB database branch served by a local HTTP process. The graph and source blocks live in database documents. The CLI and browser use the same service.

A node reference is its exact display name or complete UUID. A duplicate display name is ambiguous and requires a UUID. Paths, filenames and UUID prefixes are not references. Each node also has a stable Lean module identity such as `Lib.N_<uuid_without_hyphens>`; renaming the display title does not rename this module.

`.mdc-service` contains disposable generated compiler inputs and caches. Editing these files does not edit a node. Import/export are explicit operations; ordinary commands never scan workspace files.
