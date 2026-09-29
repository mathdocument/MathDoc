---
title: Installation
---

This page runs MathDoc from a source checkout, for local use and development.
For a server with only Docker and Git, use [Docker and SSH deployment](../server-deployment/).

## Requirements

| Needed | Why |
| --- | --- |
| Node.js 22 or newer (CI and the image use 26) and npm | Builds and runs the backend, the CLI and the web editor. |
| Docker with Compose | Runs TerminusDB and PostgreSQL from the supplied `compose.yaml`. You can use existing servers instead. |
| Python 3.9+ with `venv` | LaTeX previews. On first use the backend installs its pinned renderer packages; see [LaTeX runtime](../../concepts/latex/#runtime-and-caching). |
| A LeanGround server | Checks Lean proofs. Without it, documents, LaTeX and the graph still work, but submissions fail. |

No Lean toolchain, Elan or Lake is needed on the MathDoc host.

LeanGround must be recent enough for definition registration: `main` at or after
commit `3965273`, with its definitions schema applied
(`leanground db schema --only 10-definitions`) and its workers reinstalled with the
matching version (LeanGround's `scripts/install_tools.sh`). An older worker rejects
every request that carries definitions. The sketch component `lean-worker-sketch`
must not be disabled (`fact_sketch_enabled=false`). See the LeanGround
documentation for its own installation.

## Build

```sh
npm ci
npm run build -w @mathdoc/coordinator
npm run build -w mdc-web
```

The first command builds the backend and CLI into `coordinator/dist`; the second
builds the editor into `web/dist`. Both directories are generated and ignored by Git.

## Start the databases

Copy `config.example.env` to `.env`, replace every `replace-me` value and choose
real tokens. The file documents every variable; see also
[Configuration](../../reference/configuration/). Then:

```sh
set -a; . ./.env; set +a
docker compose up -d
```

`compose.yaml` starts TerminusDB on `127.0.0.1:6363` and PostgreSQL on
`127.0.0.1:5432`, with data in the Docker volumes `mathdoc-terminus-data` and
`mathdoc-postgres-data`. Stopping containers keeps the volumes; deleting a volume
deletes its data. An existing TerminusDB keeps its existing password: the
compose variable only sets the password of a new instance.

## Run the API and the worker

In the shell where `.env` is loaded:

```sh
npm run migrate      # create or update the PostgreSQL tables; safe to repeat
npm start            # API and web editor on MDC_HOST:MDC_PORT (default 127.0.0.1:17843)
npm run worker       # in a second shell: the LeanGround worker
```

These run `node coordinator/dist/main.js migrate|serve|worker`. The API serves
the editor at `http://127.0.0.1:17843/`. The browser asks for an access token:
enter one of the tokens from `MDC_ACTORS`. The worker is needed only for proof
work; documents are served by the API alone.

## The `mdc` CLI

The CLI is `coordinator/dist/cli.js`, a thin HTTP client. It needs the server URL
and your token:

```sh
alias mdc="node $PWD/coordinator/dist/cli.js"
export MDC_URL=http://127.0.0.1:17843   # the default
export MDC_TOKEN=your-access-token
mdc status
```

See [CLI commands](../../reference/workspace-commands/). Continue with the
[Quick start](../quick-start/).

## Upgrade

Pull, run `npm ci` and both builds again, run `npm run migrate`, then restart the
API and worker processes. Reload open browser tabs after saving drafts. Keep the
TerminusDB and PostgreSQL volumes and the existing passwords.
