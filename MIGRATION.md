# Web service migration

Approved scope: TerminusDB is the source of truth; the local HTTP service owns all
mutations and compilation. Clients resolve exact names or complete UUIDs. Keep
text, Lean, Rocq and LaTeX blocks, with Lean as the first language-server backend.
Keep strict direct managed import / dependency matching. Remove sync/back and the
MathDoc VS Code extension. Commit each tested stage using Conventional Commits.

1. TerminusDB documents, atomic version-guarded writes, graph snapshots and tests.
2. Web/API and CLI cutover; explicit import/export; remove filesystem workflows.
3. Persistent Lean sessions, version-bound checks, Lake artifacts and strict deps.
4. Monaco/Infoview split editor, remove Python/path UI, end-to-end verification.

The generated Lean project is disposable build state, never an editable archive.
Toolchain and Lake project configuration are versioned database documents. Lean
imports are obtained from Lean, not inferred by regex or generated from deps.
Each editor has an isolated draft session. CLI checks reuse persistent sessions.
Compilation success and strict dependency certification remain distinguishable.
Rocq and LaTeX remain editable; their language servers are outside this migration.

Validation: real TerminusDB CRUD, restart persistence and stale-write rejection;
real Lean edit/error/fix, goals, dependency mismatch and repeated warm checks;
frontend type checks/build/tests and browser interaction. Existing filesystem-only
tests are retired with their implementation, not used to certify the new service.
