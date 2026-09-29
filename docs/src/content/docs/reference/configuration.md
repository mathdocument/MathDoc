---
title: Configuration
---

All configuration comes from environment variables; there is no configuration
file. `config.example.env` lists every variable with comments. When running from
a checkout, load it into the shell (`set -a; . ./.env; set +a`) or your process
manager; `compose.server.yaml` reads `.env` itself.

## Server (`serve` and `worker`)

| Variable | Default | Meaning |
| --- | --- | --- |
| `MDC_ACTORS` | required | JSON map `{"NAME": {"token": "…", "admin": false}}`. Names use letters, digits, `_`, `-`; tokens are at least 16 characters and distinct. |
| `MDC_DATABASE_URL` | required | PostgreSQL connection URL. Also the only variable `migrate` needs. |
| `MDC_TERMINUS_URL` | unset | TerminusDB URL, e.g. `http://127.0.0.1:6363`. Without it and the password, the document API and editor are disabled. |
| `MDC_TERMINUS_USER` | `admin` | TerminusDB user. |
| `MDC_TERMINUS_PASSWORD` | unset | TerminusDB password. |
| `LEANGROUND_SERVER_URL` | required | LeanGround base URL (`http` or `https`, no credentials in the URL). |
| `LEANGROUND_FACT_TOKEN` | required | The coordinator's LeanGround fact token. |
| `LEANGROUND_ACTOR` | required | The coordinator's actor name in LeanGround; added to every LeanGround project. |
| `MDC_HOST` | `127.0.0.1` | Listen address of the API (`0.0.0.0` inside the image). |
| `MDC_PORT` | `17843` | Listen port. |
| `MDC_PUBLIC_ORIGIN` | unset | Extra accepted origin behind a reverse proxy; see below. |
| `MDC_WEB_DIR` | `web/dist` | Built editor served at `/` and `/p/DB/BRANCH/`. |
| `MDC_RENDERER_DIR` | `renderer/` of the checkout | LaTeX renderer directory. |
| `MDC_LATEX_PYTHON` | unset | Python with the renderer's packages; unset, the API installs a pinned virtual environment on first use. |
| `MDC_CACHE_DIR` | `$XDG_CACHE_HOME/mathdoc` or `~/.cache/mathdoc` | Absolute path; holds the installed LaTeX runtime. |
| `MDC_APP_DIR` | unset | Serves the archived collaboration prototype (`app/dist`) at `/` instead of the editor. Not for normal use. |

The Compose files also use `MDC_POSTGRES_PASSWORD` (and `MDC_TERMINUS_PASSWORD`)
to initialize new database volumes; they do not change existing passwords.

Keep `.env` private (mode 600) and out of Git. Tokens and passwords never appear
in exports. Changing `MDC_ACTORS` takes effect when the processes restart.

## CLI

| Variable | Default | Meaning |
| --- | --- | --- |
| `MDC_URL` | `http://127.0.0.1:17843` | Server origin. |
| `MDC_TOKEN` | required | Your access token. |
| `MDC_PROJECT` | unset | Default `DATABASE/BRANCH`; `-p` overrides it. |

## Reverse proxy

The API accepts requests whose `Host` is loopback (`localhost`, `127.x.x.x`,
`::1`) or equals the host of `MDC_PUBLIC_ORIGIN`. If an `Origin` header is sent
it must match that same origin. This blocks cross-site browser requests; it is
not authentication.

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

The **proof environment** is per branch but lives in PostgreSQL, not in the
document; it is not exported and not copied to new branches. See
[Proofs with LeanGround](../proofs/#proof-environment).
