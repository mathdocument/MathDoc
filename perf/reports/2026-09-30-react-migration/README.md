# React migration validation

Compared main (`5792f14`, Svelte) with the React shell (`3348003`) on the same
Linux workstation and Chromium build. Reports retain the machine, fixture sizes
and raw observations. Each run warms up once and takes two timed samples. These
are local smoke measurements, not a claim that React makes rendering faster.

| Measurement | Before | After |
| --- | ---: | ---: |
| Initial shell, gzip | 63.5 KiB | 183.5 KiB |
| All bundled assets, gzip where applicable | 18.17 MiB | 18.28 MiB |
| Editor ready, median | 991 ms | 1,033 ms |
| LaTeX preview, median | 5,547 ms | 6,021 ms |
| 10,000-node graph ready, median | 129 ms | 176 ms |
| Graph zoom frame, p95 | 19.3 ms | 21.2 ms |

All existing runtime tolerances pass. The React DOM/Base UI shell has an explicit
192 KiB initial cap; the relative 8% shell guard resumes for React-to-React
comparisons. Total assets retain their 8% guard and use a 20 MB absolute cap.
The old 9 MB cap was already below main's 19.05 MB bundle, which includes the
existing native editors, MathJax, TikZ runtime and fonts.

The current harness can measure the old shell with `--baseline`, then compare
head using the same fixtures and browser. UI assertions run on head: 8,311-item
virtual lists, exact scroll retention, continuous arrow/Home/End navigation,
dependency pagination and filtering, bounded Retina canvas size, accessible resize
separators, first-paint headers, cross-document transitions and reduced motion.
The harness also fixes pre-existing stale assumptions about the projects API,
qualified node names and preview boundary scrolling.

The migration keeps Canvas painting and pointer handling outside React. Monaco,
Lean's isolated iframe, LSP transport, Infoview and LaTeX rendering remain native
sessions. `useSessionLifetime` avoids disposing these sessions during a component
Fast Refresh effect reattachment.

Validation uses disposable databases and retains the development service on
17844. Vite on 5174 serves the React frontend for interactive review.

Validation: TypeScript and 32 unit tests pass; production frontend, Rust embedding
and documentation builds pass. The real-backend browser suite covers the migrated
flows, including draft conflicts, atomic saves, Lean session reuse/cancellation,
LaTeX rendering and completion, and native scrolling. Two asynchronous test races
were corrected: correlating a held Lean response with its actual request ID, and
waiting for native syntax colors instead of inspecting an intermediate paint.
