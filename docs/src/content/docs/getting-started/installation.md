---
title: Installation
---

For a server with only Docker and Git, use [Docker and SSH deployment](../server-deployment/).
The instructions below describe native installation and local development.

MathDoc runs on a Unix host. Install Rust to build `mdc`, Docker with Compose for
the supplied TerminusDB deployment, and Elan for native Lean. Node.js 26 and npm
are required to build the frontend from source; generated browser assets are not
stored in Git. The installed executable does not need Node.js.
LaTeX previews require Python 3.9+ with venv support. On first use, mdc installs
its pinned parser dependencies in a shared virtual environment. See
[LaTeX runtime setup](../../concepts/latex/#runtime-and-caching) for offline use.

## Install the executable

From the source checkout:

```sh
npm --prefix web ci
npm --prefix web run build
cargo install --path . --locked
elan toolchain install leanprover/lean4:v4.33.1
mdc --help
```

After building the frontend, `cargo build --release --locked` is an alternative
that produces `target/release/mdc`.
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
The JSON result also includes the shared port, server PID and runtime log path.
Services remain running after the terminal closes. Commands work from any directory.

```sh
mdc stop myproject/main
mdc start myproject/main
# Stop all branches and change the shared port, then reload the desired branch.
mdc stop
mdc start myproject/main --port 17844
```

`stop DATABASE/BRANCH` unloads that branch and shuts down its Lean workers,
retaining data and caches. Bare `stop` shuts down the server and every loaded
branch; bare `start` starts/reuses the server without loading a branch. `--port` always selects the entry
port; occupied ports and attempts to change a running entry's port fail. All branches
run in the same mdc process through one HTTP listener. Background execution is part of `start`, with no
internal CLI command. See [Configuration](../../reference/configuration/) for
the default port setting and authenticated reverse-proxy deployment. Local use
requires no separate web server. Lean metaprograms execute as the service user.

## Upgrade

Reinstall the binary after pulling changes. If frontend sources changed, rebuild
`web/dist` first using the [development workflow](../../development/setup/).
The running server keeps its previous executable until restarted. Save drafts,
run `mdc stop`, then `mdc start DATABASE/BRANCH` for each branch you want to load.
This updates the backend and frontend together. Reload the browser afterwards.
The same entry port keeps project URLs stable. Keep the TerminusDB volume and
existing credentials throughout upgrades.
