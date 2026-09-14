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

`start` launches/reuses the shared entry server and starts the requested branch.
The default port is **17843**. Open `http://127.0.0.1:17843/` for the project list
or the printed `http://127.0.0.1:17843/p/myproject/main/` URL for the editor.
The JSON result also includes the entry port, branch PID and log path.
Services remain running after the terminal closes. Commands work from any directory.

```sh
mdc stop myproject/main
mdc start myproject/main
# Stop only the entry server and change its port; branches remain running.
mdc stop
mdc start --port 17844
```

`stop DATABASE/BRANCH` waits for that branch and its Lean workers to shut down,
retaining data and caches. Bare `stop` stops only the entry server; bare `start`
starts/reuses it without starting a branch. `--port` always selects the entry
port; occupied ports and attempts to change a running entry's port fail. Branches
use private OS-assigned ports. Background execution is part of `start`, with no
internal CLI command. See [Configuration](../../reference/configuration/) for
the default port setting and authenticated reverse-proxy deployment. Local use
requires no separate web server. Lean metaprograms execute as the service user.

## Upgrade

Reinstall the binary after pulling changes. If frontend sources changed, rebuild
`web/dist` first using the [development workflow](../../development/setup/).
Existing service processes keep their previous executable until stopped and
started again. Restart the entry server with `mdc stop` then `mdc start` to update
shared browser assets and routing. Restart affected branches separately for
backend changes. Save drafts before restarting and reload the browser afterwards.
The same entry port keeps project URLs stable. Keep the TerminusDB volume and
existing credentials throughout upgrades.
