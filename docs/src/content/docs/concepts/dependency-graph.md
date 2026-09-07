---
title: Dependency graph
---

Each node declares direct dependencies by UUID. TerminusDB stores these as typed links. Transactions reject missing targets, duplicate edges, self links and cycles before committing.

The service maintains an in-memory graph projection with topological depths, reverse edges and Lean input keys. It refreshes after its own writes, or reloads on detecting a commit made outside the service. The graph check reports the validated projection; it does not reparse source files.

For Lean certification, the direct managed `Lib.*` imports reported by Lean Server must equal the node's `dep` set. Every dependency must also have a Lean block certified for its current inputs. Unmanaged external library imports do not need node dependencies.
