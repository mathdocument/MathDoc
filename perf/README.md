# Backend performance

Benchmarks of the TypeScript backend; how to run them and what they measure is in
[Performance measurements](../docs/src/content/docs/development/performance.md).

- [graph-service.py](graph-service.py): the document and graph API of a running server, over HTTP and the CLI.
- [board-load.ts](board-load.ts): K agents working on one project board (docs/mathdoc.md §2.7.5).
- [sync-cost.ts](sync-cost.ts): one sync job on a board with F certificates.

Reports of the removed Rust backend and the Mathlib import scripts were deleted;
they remain in Git history (`git show bcb161c:perf/reports/`).
