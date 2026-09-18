---
title: Deploy with Docker and SSH
---

Use this deployment for a trusted team sharing a Linux server through SSH. The
host needs Git and Docker with the Compose plugin; it does not need Rust, Node,
Python, Elan or root access. The current user must be allowed to start containers.
Linux AMD64 and ARM64 are supported by the runtime Dockerfile.

`compose.server.yaml` is a separate deployment from the native development
`compose.yaml`. It starts both MDC and TerminusDB with its own named volumes.
It does not attach to, import or modify an existing local installation.

## Clone and start

After logging into the server:

```sh
git clone https://github.com/mathdocument/MathDoc.git
cd MathDoc
docker compose version

# First installation only: create a private, random database password.
umask 077
test -e .env || printf 'MDC_TERMINUS_PASSWORD=%s\n' \
  "$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')" > .env

docker compose -f compose.server.yaml up -d --build --wait --wait-timeout 180
./scripts/mdc-docker status
```

The `.env` file is ignored by Git and excluded from the image build context. Keep
it: changing its password does not reset an existing TerminusDB password. Do not
overwrite it during upgrades. Anyone with the shared Unix account or Docker
access has access to this deployment and its credentials.

If `docker compose` is missing, install its [official user-level
plugin](https://docs.docker.com/compose/install/linux/#install-the-plugin-manually)
under `~/.docker/cli-plugins`; no root access is needed. Image builds also need the
Docker Buildx plugin on current Docker installations. Both plugins can be copied
from the official Docker CLI image when the host has only Docker and Git:

```sh
mkdir -p "$HOME/.docker/cli-plugins"
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$HOME/.docker/cli-plugins:/plugins" --entrypoint sh docker:cli -ec \
  'cp /usr/local/libexec/docker/cli-plugins/docker-compose /plugins/;
   cp /usr/local/libexec/docker/cli-plugins/docker-buildx /plugins/'
docker compose version
docker buildx version
```

The first build downloads the base images, npm/Rust dependencies and Python
packages. A Node build stage generates the frontend from source, then the Rust
stage embeds it into MDC. The runtime installs pinned Elan and LaTeX parser
dependencies and runs MDC as a non-root user. Node and Rust stay in build stages;
neither is installed in the runtime image. Logs go to Docker's bounded local log
driver.

## Prepare Lean and create a project

Install the default project's pinned Linux toolchain once, before opening its
editor. It is stored in a persistent volume rather than duplicated in image
layers. Other project versions can be installed with the same command; missing
versions are also downloaded by Elan when used.

```sh
docker compose -f compose.server.yaml exec -T mdc \
  elan toolchain install leanprover/lean4:v4.33.1
./scripts/mdc-docker init myproject
./scripts/mdc-docker start myproject/main
./scripts/mdc-docker status
```

`init` creates `myproject/main` in the database; it is not needed again after
restarts. Before importing an existing project, read its `toolchain` setting and
install that version. Toolchain/library downloads need outbound network access.
First use of a library may still need a Lake build; later checks reuse its cache.

## Open through SSH

Run this on your own computer, keeping the session open:

```sh
ssh -N -o ExitOnForwardFailure=yes -L 17843:127.0.0.1:17843 user@SERVER
```

Open `http://localhost:17843` for the project list. The server publishes only
`127.0.0.1:17843`; TerminusDB has no published host port. SSH authenticates access
to the tunnel. Shared SSH accounts do not provide per-person or per-project MDC
permissions. Lean code executes as the container's service user, so authors must
be trusted.

If the **server's** port is occupied, add `MDC_PORT=17844` to `.env` and rerun
Compose, then tunnel that port instead. If only your **computer's** port is
occupied, use `-L 17844:127.0.0.1:17843` and open `http://localhost:17844`.
Never change the host binding to `0.0.0.0` merely to avoid using a tunnel.
For an authenticated web proxy, see [Configuration](../../reference/configuration/).

## CLI and agents

`scripts/mdc-docker` resolves the deployment directory from its own location and
passes stdin and arguments to `mdc` inside the running container. It works from
any directory. For convenience, define a shell function with your actual clone
location:

```sh
mdc() { "$HOME/MathDoc/scripts/mdc-docker" "$@"; }

mdc new -p myproject/main -t 'Example'
mdc show -p myproject/main 'Example'
# Use the revision returned by show to protect concurrent edits.
mdc edit -p myproject/main 'Example' --revision "$REV" < proof.lean
mdc lean check -p myproject/main 'Example'
```

The shell reads `proof.lean` on the host and streams it to the container. Agents
can invoke this wrapper over SSH; they do not need their own Lean installation.
The CLI still discovers the service inside its container, not over a remote
`--url` connection.

File arguments are container paths. Stream whole-graph import/export through
stdin/stdout instead of mounting a source workspace:

```sh
mdc export -p myproject/main > graph.json
mdc init imported
mdc start imported/main
mdc import -p imported/main /dev/stdin < graph.json
```

For commands with multiple file arguments, such as `project latex set`, copy the
files into the container with `docker compose -f compose.server.yaml cp` and pass
their container paths. Remove the temporary copies afterwards. Graph exports
contain source and project settings, not full database history or compiler caches.

## Restarts and upgrades

The container runs `mdc start --foreground`. Starting a branch records it in
`active-projects.json` in the MDC volume. Restarting or recreating the container
restores those branches. Explicitly stopping a branch removes it from that list;
shutdown of the whole server preserves the list. A failed branch restore is
logged, and other branches can still start.

```sh
# Pause or resume one branch.
./scripts/mdc-docker stop myproject/main
./scripts/mdc-docker start myproject/main

# Upgrade after saving browser drafts.
git pull --ff-only
docker compose -f compose.server.yaml up -d --build --wait --wait-timeout 180

# Inspect or stop the deployment without deleting data.
docker compose -f compose.server.yaml ps
docker compose -f compose.server.yaml logs --tail 100 -f mdc
docker compose -f compose.server.yaml stop
```

Use Compose to stop the whole deployment. Bare `mdc stop` exits the container's
server process, which Docker's `unless-stopped` policy can restart. Normal Docker
stops send SIGTERM and allow 60 seconds for requests and Lean workers to close.

## Persistent storage and backups

| Default volume | Contents |
| --- | --- |
| `mathdoc-server_terminus-data` | All databases, branches, source and commit history. Back this up. |
| `mathdoc-server_mdc-cache` | Per-branch generated sources, dependencies, compiler caches and branch restart selection. |
| `mathdoc-server_lean-toolchains` | Downloaded Linux Lean toolchains, shared by this service. |

The Compose project name controls these volume prefixes. Keep it unchanged across
upgrades. `docker compose ... down` retains named volumes; **`down --volumes`
deletes them, including all graph data and history**. Do not use it for upgrades.
Keep `.env` separately with your private backups.

For a consistent full-history backup, stop the deployment and archive its
TerminusDB volume; restart afterwards. This example uses a temporary helper
container and works without host root privileges:

```sh
docker compose -f compose.server.yaml stop
docker run --rm -v mathdoc-server_terminus-data:/data:ro debian:bookworm-slim \
  tar -C /data -czf - . > terminus-backup.tar.gz
docker compose -f compose.server.yaml up -d --wait --wait-timeout 180
```

Cache volumes can be regenerated, but losing the MDC volume also loses the branch
restart selection. Keep it for fast incremental checks. Do not copy macOS native
compiler caches into the Linux volume. Moving existing graph data into this new
deployment is an explicit import or database restore, not part of `git clone`.

## Deployment verification

After building the image, developers can run:

```sh
python3 tests/docker-smoke.py
```

This creates a separate Compose project, a random password, an unused loopback
port and disposable volumes. It checks HTTP access, CLI mutations, native Lean
compilation, LaTeX rendering, restart/recreation and cache reuse, then removes only
its own containers and volumes. It downloads one Lean toolchain and does not
compile or modify existing projects. Local native development remains unchanged.
