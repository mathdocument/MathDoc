---
title: Service and project configuration
---

## Host settings and credentials

Settings come from `$XDG_CONFIG_HOME/mdc/config.toml`, defaulting to
`~/.config/mdc/config.toml`. `MDC_CONFIG` selects an explicit file; a missing
explicit file is an error. Environment values override corresponding file values,
then built-in defaults apply. Unknown TOML keys are rejected.

```toml
terminus_url = "http://127.0.0.1:6363"
terminus_user = "admin"
terminus_password = "the-existing-database-password"
# port = 17843
# public_origin = "https://mdc.example.test"
# cache_dir = "/absolute/path/to/cache"
# lean_timeout_seconds = 300
```

The password is plaintext in this private file or the process environment. Protect
the file with mode 600. It is the password of the existing TerminusDB instance;
`mdc init` does not generate or change it. With the supplied Docker deployment,
`MDC_TERMINUS_PASSWORD` also configures `TERMINUSDB_ADMIN_PASS` in the container.
Do not commit credentials or include them in graph exports.

| Environment override | Default without file setting |
| --- | --- |
| `MDC_TERMINUS_URL` | `http://127.0.0.1:6363` |
| `MDC_TERMINUS_USER` | `admin` |
| `MDC_TERMINUS_PASSWORD` | Required for direct database operations. |
| `MDC_CACHE_DIR` | `$XDG_CACHE_HOME/mdc` or `~/.cache/mdc` |
| `MDC_LATEX_PYTHON` | Optional Python executable with the pinned LaTeX runtime installed. |
| `MDC_LEAN_TIMEOUT_SECONDS` | 300; must be a positive integer. |
| `MDC_LISTEN_ADDRESS` | `127.0.0.1`; use `0.0.0.0` inside a container. Only these two addresses are accepted. |

`init`, `start`, `status` and `branch del` need TerminusDB credentials. Other
clients and `stop` need the same endpoint setting and cache root as the service;
they discover it locally and do not query TerminusDB themselves. Use the same
endpoint spelling: it is part of the cache identity. The timeout applies to
native Lean operations, not to CLI argument parsing or service startup.

`port` selects the entry server's listening port (default 17843); `mdc start
--port PORT` overrides it. All branches share this one listener; the setting has no
environment override. A running entry server keeps its port until stopped.

Cache overrides must be absolute paths. Cache paths append an endpoint hash,
database name and branch name. The branch record and generated Lean data
live there; the server record and shared runtime log live in `ENDPOINT_HASH/.server/`.
See [Storage](../../concepts/workspaces/). Changing a cache root does
not migrate records or stop services in the old root.

There is no `MDC_URL`, TOML `url`, or CLI `--url`. Select a running branch with
`-p/--proj DATABASE/BRANCH` after a branch or node command. Management commands use
positional names instead. No current directory, default branch or default client
port selects a project implicitly.

## Access through a reverse proxy

The entry server listens on loopback by default. Container deployments can set
`MDC_LISTEN_ADDRESS=0.0.0.0`, while publishing the port only on the host's loopback
interface. This does not enable authentication or change the Host/Origin checks.
For browser access from another machine,
place an authenticated HTTPS reverse proxy on the same host and forward the
whole site to `127.0.0.1:17843`, preserving paths, queries and the public Host.
Enable WebSocket upgrades and long-lived connections for Lean. All projects use
the same public origin and `/p/DATABASE/BRANCH/` routes; no per-branch proxy rules
are needed.

Set `public_origin` to that exact HTTP(S) origin, then restart the entry server.
The setting permits its Host/Origin pair and controls returned browser URLs;
paths, queries, fragments and credentials are rejected. Leave it unset for local
use. The proxy must supply authentication and access control: this setting is
an origin allowlist, not a login system. MathDoc currently trusts its authors,
has no per-project user permissions and runs Lean metaprograms as the service
user. Only deploy shared access to trusted authors. Keep TerminusDB private.

## Versioned Lean project

Lean environment settings live in the database branch, separate from host
settings. Read them with `mdc project show -p myproject/main`; replace them through
the browser's Project settings → Lean tab or `mdc project set -p myproject/main` on stdin.

A minimal complete input object is:

```json
{
  "toolchain": "leanprover/lean4:v4.33.1",
  "lakefile": "name = \"MathDoc\"\nversion = \"0.1.0\"\n\n[[lean_lib]]\nname = \"Lib\"\n",
  "manifest": null
}
```

`lakefile` contains the original configuration text. `lakefile_name` defaults to
`lakefile.toml`; set it to `lakefile.lean` for a native Lean configuration. Both
formats are materialized unchanged. `module_root` defaults to `Lib` and selects
the namespace for newly created nodes, for example `Mathlib.N_<uuid>`.
Optional `files` maps relative paths to supporting UTF-8 source/configuration
text. These files are versioned and exported but are not graph nodes. Hidden,
absolute, traversal and reserved configuration paths are rejected, as are
collisions with graph modules. Compiler artifacts do not belong in `files`.
`manifest` is either null/omitted or the JSON text of a complete
`lake-manifest.json`, encoded as a string. `project show` wraps this object in
`{revision, project}`; pass only `project` back to `set`. To reject concurrent
branch changes, supply `--revision` from that response.

Pin a Lean release and declare the library selected by `module_root`. External libraries are not
limited to mathlib: declare them in Lake and supply a complete manifest with all
Git dependencies locked to full 40-character commits. Mutable path dependencies,
custom top-level `srcDir`, `buildDir`, `leanLibDir`, and a nonstandard
`packagesDir` are unsupported. First preparation may download missing libraries
and build imports. Changing the environment invalidates matching Lean input keys;
reload an open editor environment after changing it. Native Lean configurations
require a manifest even without dependencies. They execute as the trusted local
author; their library declarations and paths are evaluated by Lake, not a TOML
validator. Keep the standard source and artifact directories for mdc checks.

## Versioned LaTeX project

The **Project settings → LaTeX** tab and `mdc project latex set` store one macro
file (`.cls` or `.tex`) and one bibliography (`.bib`) in the branch. They are
inherited by new branches and included in graph exports. See
[LaTeX previews](../../concepts/latex/) for supported macros, reference scope,
completion and runtime installation. Python dependencies are shared under
`CACHE_ROOT/runtime/`; parsed document caches remain private to each branch's
worker. There are no generated LaTeX files in a project workspace.

## Timing measurements

`-m/--meas` prints inclusive elapsed timings to stderr and preserves JSON on
stdout. It is available on each command and inherited by its subcommands, but
not accepted before the top-level command. Inclusive timings can overlap and
should not be added together. Measurements describe the current CLI invocation;
a remote HTTP duration includes service work without exposing every server span.

```sh
mdc status -m
mdc graph check -p myproject/main --meas
```
