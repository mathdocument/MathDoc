---
title: Dependency graph
---

Each node declares direct dependencies by UUID. TerminusDB stores these as typed
links. Graph validation rejects missing targets, duplicate edges, self links and
cycles before committing. Adding an already-present dependency through the API
is an idempotent operation and does not create another edge.

The service maintains an in-memory graph projection with topological depths, reverse edges and Lean input keys. It refreshes after its own writes, or reloads on detecting a commit made outside the service. The graph check reports the validated projection; it does not reparse source files.

For Lean certification, imports reported by Lean Server are matched against the actual module identities in the graph, including original names such as `Mathlib.Data.Nat.Basic`. These direct managed imports must equal the node's `dep` set. Every dependency must also have a Lean block certified for its current inputs. External library and supporting project-file imports do not need node dependencies.
