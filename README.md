# MathDoc

MathDoc manages mathematical knowledge as a versioned dependency graph in TerminusDB, where nodes carry text, LaTeX, Lean and Rocq. People and agents prove the graph together: a Lean node is submitted to [LeanGround](https://github.com/YanbiaoLab/LeanGround), which checks every proof, and certified results are written back to the document in reviewed batches. The browser editor, the `mdc` CLI and agents use the same HTTP API with access tokens.

- [Documentation](https://mathdocument.github.io/MathDoc/)
- [Installation](docs/src/content/docs/getting-started/installation.md)
- [Server deployment](docs/src/content/docs/getting-started/server-deployment.md)
- [Quick start](docs/src/content/docs/getting-started/quick-start.md)
- [Collaboration](docs/src/content/docs/concepts/collaboration.md)
- [Agent interface](docs/src/content/docs/reference/agents.md)
- [CLI reference](docs/src/content/docs/reference/workspace-commands.md)
- [HTTP API](docs/src/content/docs/reference/http-api.md)
- [Development](docs/src/content/docs/development/setup.md)

Repository layout: `coordinator/` the TypeScript backend (API, worker, `mdc` CLI), `web/` the knowledge editor, `renderer/` the LaTeX renderer, `app/` an archived collaboration prototype, `docs/` the documentation site and migration records.
