# MathDoc

MathDoc manages mathematical knowledge as a versioned dependency graph in
TerminusDB, with a locally hosted browser interface and the `mdc` CLI.

Nodes support text, Lean, Rocq and LaTeX. Lean editing includes Monaco, native
Infoview, incremental Lean Server checks and reusable Lake artifacts. Browser
and CLI edits share database transactions and revision guards.

- [Documentation](https://mathdocument.github.io/MathDoc/)
- [Installation](docs/src/content/docs/getting-started/installation.md)
- [Quick start](docs/src/content/docs/getting-started/quick-start.md)
- [CLI reference](docs/src/content/docs/reference/workspace-commands.md)
- [HTTP API](docs/src/content/docs/reference/http-api.md)
- [Development](docs/src/content/docs/development/setup.md)

MathDoc is self-hosted on a Unix machine and accessed through a local browser.
Only Lean compilation is integrated; Rocq and LaTeX blocks are stored and edited.
