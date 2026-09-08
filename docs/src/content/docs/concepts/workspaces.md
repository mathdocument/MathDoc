---
title: Databases and references
---

A MathDoc project is a TerminusDB database branch served by a local HTTP process. The graph and source blocks live in database documents. The CLI and browser use the same service.

A node reference is its exact display name or complete UUID. A duplicate display name is ambiguous and requires a UUID. Paths, filenames and UUID prefixes are not references. Each node also has a stable Lean module identity such as `Lib.N_<uuid_without_hyphens>`; renaming the display title does not rename this module.

The user cache (`~/.cache/mdc` by default) contains generated compiler inputs, certificates and caches, separated by endpoint/database/branch. The installed program and private user configuration live outside the cache. No project folder is required. Stop the service before removing its cache. Editing these files does not edit a node. Import/export are explicit operations; ordinary commands never scan workspace files.
