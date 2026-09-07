---
title: Browser frontend
---

The main Svelte interface retains search, graph/column navigation, dependency operations and non-Lean block editing. It tracks unsaved drafts and serializes node mutations with revision guards.

Lean uses a separate lazy-loaded page embedding `lean4monaco` and the upstream Infoview. Each page receives an isolated native Lean WebSocket session. The code editor and Infoview share a two-column area. Native browser providers are installed by LeanMonaco; its desktop extension entry is disabled in the browser manifest.

The browser integration test runs the real TerminusDB service, checks conflicting CLI/browser writes, navigation, graph invariants, native Lean goals/diagnostics and saved `.olean` builds. Frontend source types contain no Python or path-entry controls.
