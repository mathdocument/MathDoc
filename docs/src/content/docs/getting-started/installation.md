---
title: Installation
---

MathDoc runs on a Unix host. Install Rust to build `mdc`, Docker with Compose for
the supplied TerminusDB deployment, and Elan for native Lean. Node.js is needed
only to develop or rebuild the frontend; the repository includes browser assets.

## Install the executable

From the source checkout:

```sh
cargo install --path . --locked
elan toolchain install leanprover/lean4:v4.33.1
mdc --help
```

Alternatively, `cargo build --release --locked` produces `target/release/mdc`.
`cargo install` puts the binary in Cargo's executable directory, normally
`~/.cargo/bin`; include that directory in your PATH. No per-project binary or
launch script is needed.

## Start TerminusDB

If you already have a TerminusDB instance, reuse it and its existing credentials.
For a new instance, set `MDC_TERMINUS_PASSWORD` to a private password you choose,
then run this from the directory containing the supplied `compose.yaml`:

```sh
docker compose up -d
```

The compose file pins the server image, binds port 6363 to `127.0.0.1`, and stores
durable data in the Docker volume `mathdoc-terminus-data`. This is one shared
server: each MathDoc project is a separate database in it. Stopping containers
keeps the volume; deleting the volume deletes database data and history.

## Configure the client

Save the TerminusDB endpoint, user and the same database password in
`~/.config/mdc/config.toml` (or `$XDG_CONFIG_HOME/mdc/config.toml`). Use mode 600.
See [Configuration](../../reference/configuration/) for the complete example,
precedence and environment overrides. `mdc init` requires a reachable configured
TerminusDB server; it does not install or initialize Docker.

## Create and run a project

```sh
mdc init myproject
mdc start myproject/main
mdc status
```

`init` creates a new database with schema, Lean project settings and the `main`
branch. It starts no service and creates no file workspace. Use it once for each
new database; it does not reset an existing one.

`start` launches a background service for an existing branch and returns its URL,
port, PID and log path when ready. Open that URL in the browser. The OS chooses an
available port unless you pass `--port PORT`; occupied explicit ports fail.
The service remains running after the terminal closes. All subsequent commands
can run from any directory.

```sh
mdc stop myproject/main
mdc start myproject/main --port 7600
```

Stop waits for the HTTP service and Lean workers to shut down, retaining the data
and caches. Background execution is part of `start`, with no separate internal
CLI command. Services bind to loopback and are intended for trusted local authors;
Lean metaprograms execute as the service user.

## Upgrade

Reinstall the binary after pulling changes. If frontend sources changed, rebuild
`web/dist` first using the [development workflow](../../development/setup/).
Existing service processes keep their previous executable until stopped and
started again. Save browser drafts before restarting, then reopen or reload the
browser. Keep the TerminusDB volume and existing credentials throughout upgrades.
