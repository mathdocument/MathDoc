---
title: Installation
---

This page runs MathDoc from a source checkout, for local use and development.
For a server with only Docker, use the [Docker Compose deployment](../server-deployment/).

## Requirements

| Needed | Why |
| --- | --- |
| Node.js 22 or newer (CI and the image use 26) and npm | Builds and runs the backend, the CLI and the web editor. |
| Docker with Compose | Runs TerminusDB and PostgreSQL from the supplied `compose.yaml`. You can use existing servers instead. |
| Python 3.9+ with `venv` | LaTeX previews. On first use the backend installs its pinned renderer packages; see [LaTeX runtime](../../concepts/latex/#runtime-and-caching). |
| A LeanGround server | Checks every Lean proof. The `LEANGROUND_*` variables must be set for the API and worker to start. While LeanGround is unreachable, documents, LaTeX and the graph still work; setting a proof environment fails, and jobs that need LeanGround (conversion, submissions, assembly) are retried with backoff for about 40 minutes. |

No Lean toolchain, Elan or Lake is needed on the MathDoc host.

LeanGround must be recent enough for definition registration: `main` at or after
commit `3965273`, with its definitions schema applied
(`leanground db schema --only 10-definitions`) and its workers reinstalled with the
matching version (LeanGround's `scripts/install_tools.sh`). An older worker rejects
every request that carries definitions. The sketch component `lean-worker-sketch`
must not be disabled (`fact_sketch_enabled=false`). MathDoc calls LeanGround with
one fact token (`LEANGROUND_FACT_TOKEN`) that must belong to `LEANGROUND_ACTOR`.
See the LeanGround documentation for its own installation.

## Build

```sh
npm ci
npm run build -w @mathdoc/coordinator
npm run build -w mdc-web
```

The first build puts the backend and CLI into `coordinator/dist`; the second
puts the editor into `web/dist` (`npm run build` runs both). Both directories are
generated and ignored by Git.

## Start the databases

Copy `config.example.env` to `.env`, replace every `replace-me` value and choose
real tokens: one `MDC_ACTORS` entry per person or agent, with distinct tokens of
at least 16 characters, and `"admin": true` for whoever creates databases. The
PostgreSQL password appears twice, in `MDC_POSTGRES_PASSWORD` and in
`MDC_DATABASE_URL`. Set `LEANGROUND_SERVER_URL`, `LEANGROUND_FACT_TOKEN` and
`LEANGROUND_ACTOR` for your LeanGround. The file documents every variable; see
also [Configuration](../../reference/configuration/). Then:

```sh
set -a; . ./.env; set +a
docker compose up -d
```

`compose.yaml` starts TerminusDB on `127.0.0.1:6363` and PostgreSQL on
`127.0.0.1:5432` (`MDC_TERMINUS_PORT` and `MDC_POSTGRES_PORT` change the host
ports), with data in the Docker volumes `mathdoc-terminus-data` and
`mathdoc-postgres-data`. Stopping containers keeps the volumes; deleting a volume
deletes its data. Existing volumes keep their passwords: the compose variables
only set the passwords of new instances.

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

Ctrl-C or `SIGTERM` stops either process. The API finishes the requests in
progress and stops its LaTeX renderers, then exits; the worker finishes its
current job.

## The `mdc` CLI

The CLI is `coordinator/dist/cli.js`, an HTTP client of the API. It needs the
server URL and your token; `MDC_PROJECT` (or `-p`) names the branch most commands
work on:

```sh
alias mdc="node $PWD/coordinator/dist/cli.js"
export MDC_URL=http://127.0.0.1:17843   # the default
export MDC_TOKEN=your-access-token
mdc status
```

It keeps no token on disk. The proof work commands (`pull`, `prove`, `split`,
`done`, …) keep records in a `.mdc/` directory next to their goal files, and
`review`, `pick`, `branch new` and `proof command` keep replay records in a
state directory (`MDC_STATE_DIR`, default `~/.local/state/mdc`).

See [CLI commands](../../reference/workspace-commands/). Continue with the
[Quick start](../quick-start/).

## Upgrade

Pull, run `npm ci` and both builds again, run `npm run migrate`, then restart the
API and worker processes. Reload open browser tabs after saving drafts. Keep the
TerminusDB and PostgreSQL volumes and the existing passwords.
