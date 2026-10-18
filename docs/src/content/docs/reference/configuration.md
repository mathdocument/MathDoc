---
title: Configuration
---

All configuration comes from environment variables. `config.example.env` lists the
common ones with comments; this page lists all of them. When running from a
checkout, copy it to `.env`, edit it and load it into the shell
(`set -a; . ./.env; set +a`) or your process manager. A Compose deployment reads
settings from `mathdoc.env` and secrets from files in `secrets/`
([Server deployment](../../getting-started/server-deployment/)).

Each secret can instead be read from a file: `MDC_ACTORS_FILE`,
`MDC_TERMINUS_PASSWORD_FILE`, `MDC_DATABASE_PASSWORD_FILE` and
`LEANGROUND_FACT_TOKEN_FILE`. Setting both a variable and its `_FILE` form is an
error, as is an empty or unreadable file; a file's content is trimmed of
surrounding whitespace.

## Server (`serve` and `worker`)

| Variable | Default | Meaning |
| --- | --- | --- |
| `MDC_ACTORS` | required | JSON map `{"NAME": {"token": "…", "admin": false}}` with at least one actor. Names use letters, digits, `_`, `-`; tokens are at least 16 characters and distinct; `admin` defaults to `false`; other fields are rejected. |
| `MDC_DATABASE_URL` | required | PostgreSQL connection URL. `migrate` needs only this (and `MDC_DATABASE_PASSWORD` when the password is kept apart). |
| `MDC_DATABASE_PASSWORD` | unset | Password added to `MDC_DATABASE_URL` (usually as `MDC_DATABASE_PASSWORD_FILE`). |
| `MDC_TERMINUS_URL` | unset | TerminusDB URL, e.g. `http://127.0.0.1:6363`. Unless both it and `MDC_TERMINUS_PASSWORD` are set, the document API and editor are disabled. |
| `MDC_TERMINUS_USER` | `admin` | TerminusDB user. |
| `MDC_TERMINUS_PASSWORD` | unset | TerminusDB password. |
| `LEANGROUND_SERVER_URL` | required | LeanGround base URL (`http` or `https`, no credentials in the URL). |
| `LEANGROUND_FACT_TOKEN` | required | The coordinator's LeanGround fact token. |
| `LEANGROUND_ACTOR` | required | The coordinator's actor name in LeanGround; added to every LeanGround project. |
| `LEANGROUND_TIMEOUT_S` | `960` | Whole seconds, 30 to 86400, that a LeanGround call running Lean (`resolve`, `submit`, `define`, `assemble`) may take, just above LeanGround's own 900-second default; other calls wait 30 seconds. A call that times out, or a LeanGround that cannot be reached, fails the job's try; the job is retried with backoff (5 s, 15 s, 45 s, …, at most 15 minutes apart, 8 tries in all). |
| `MDC_HOST` | `127.0.0.1` | Listen address of the API (`0.0.0.0` inside the image). |
| `MDC_PORT` | `17843` | Listen port (1–65535). |
| `MDC_PUBLIC_ORIGIN` | unset | Extra accepted origin behind a reverse proxy, also the base of the URLs in `/api/status`: an `http` or `https` origin without credentials, path, query or fragment (otherwise the process does not start); see below. |
| `MDC_WEB_DIR` | `web/dist` | Built editor served at `/` and `/p/DB/BRANCH/`, relative to the working directory. |
| `MDC_RENDERER_DIR` | `renderer/` of the checkout | LaTeX renderer directory. |
| `MDC_LATEX_PYTHON` | unset | Python with the renderer's packages; unset, the API installs a pinned virtual environment (plasTeX 3.1, pybtex 0.26.1) under `MDC_CACHE_DIR` on first use, which needs `python3` with `venv`. |
| `MDC_CACHE_DIR` | `$XDG_CACHE_HOME/mathdoc` or `~/.cache/mathdoc` | Absolute path; holds the installed LaTeX runtime. Unused when `MDC_LATEX_PYTHON` is set. |

The Docker image sets `MDC_HOST=0.0.0.0`, `MDC_PORT=17843`,
`MDC_WEB_DIR=/app/web/dist`, `MDC_RENDERER_DIR=/app/renderer`,
`MDC_LATEX_PYTHON=/opt/latex/bin/python` and `MDC_CACHE_DIR=/var/lib/mdc/cache`.
The `worker` reads and validates the same variables, but serves nothing: the
listen address, public origin, editor and LaTeX settings have no effect on it.

The development `compose.yaml` uses `MDC_TERMINUS_PASSWORD` and `MDC_POSTGRES_PASSWORD`
(and optional `MDC_TERMINUS_PORT`, `MDC_POSTGRES_PORT`, default `6363` and `5432`)
to initialize its database volumes; they do not change existing passwords. The
deployment's `compose.yaml` (`compose.server.yaml` in a checkout) sets
`MDC_TERMINUS_URL`, `MDC_DATABASE_URL` and the `_FILE` forms of the four secrets
itself (files in `secrets/`); it reads `MATHDOC_IMAGE` (default
`ghcr.io/mathdocument/mathdoc-runtime:0.7.0`), `MDC_PORT` (the host port only;
the container listens on 17843) and `COMPOSE_PROJECT_NAME` (default `mathdoc`,
the prefix of container and volume names) from the shell or a `.env` file next
to it.

Keep `.env` and a deployment's `secrets/` private and out of Git. Tokens and passwords never appear
in exports. Changing `MDC_ACTORS` takes effect when the processes restart.

## CLI

| Variable | Default | Meaning |
| --- | --- | --- |
| `MDC_URL` | `http://127.0.0.1:17843` | Server origin. Unset, the proof work commands use the server of their `.mdc/` workspace. |
| `MDC_TOKEN` | required | Your access token. |
| `MDC_PROJECT` | unset | Default `DATABASE/BRANCH`; `-p` overrides it. |
| `MDC_STATE_DIR` | `$XDG_STATE_HOME/mdc`, else `~/.local/state/mdc` | Replay records of `review`, `pick`, `branch new` and `proof command`. |

See [CLI commands](../workspace-commands/#setup).

## Reverse proxy

The document API (`/api/status`, `/api/projects` and `/p/DB/BRANCH/api/…`)
accepts requests whose `Host` is loopback (`localhost`, `127.x.x.x`, `::1`) or
equals the host of `MDC_PUBLIC_ORIGIN`. If an `Origin` header is sent it must
match that same origin (`http://HOST` for a loopback host); otherwise the reply
is `403`. This blocks cross-site browser requests; it is not authentication. The
collaboration API, `/api/me` and `/api/tokens` rely on the token alone.

For browser access from other machines, put an HTTPS reverse proxy on the server,
forward the whole site to `127.0.0.1:17843` preserving path, query and the public
`Host`, and set `MDC_PUBLIC_ORIGIN` to the exact public origin, for example
`https://mathdoc.example.org` (no path, query or credentials). Tokens then
authenticate each person or agent; the proxy may add its own access control.
Keep TerminusDB, PostgreSQL and LeanGround private.

## Versioned project settings

Settings stored in the branch rather than the environment:

- **LaTeX project**: one macro file (`.cls` or `.tex`) and one bibliography,
  set in **Project settings → LaTeX** or with `mdc project latex set`
  ([LaTeX](../../concepts/latex/)). Inherited by new branches and exported.
- **Lean project** (`GET`/`PUT /p/DB/BRANCH/api/project/lean`): the toolchain and
  Lake settings from the earlier Lean integration. Preserved and exported, but
  not used: proofs are checked in the proof environment.

The **proof environment** is per database (shared by its branches) but lives in
PostgreSQL, not in the document; it is not exported. See
[Proofs with LeanGround](../proofs/#proof-environment).
