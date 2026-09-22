---
title: Deploy with Docker Compose
---

The server needs Docker Engine and its Compose plugin. It does not need a Git
checkout, Rust, Node, Python or a host Lean installation. The deployment runs two
containers, `mathdoc-runtime` and `mathdoc-database`, and three named volumes.

The runtime uses Debian Trixie slim and includes the embedded web frontend,
Python/LaTeX dependencies, Elan and **Lean 4.33.1**. That Lean release works on
first start without a toolchain download. Project libraries may still need
network access and compilation. The release workflow currently targets Linux
AMD64; ARM64 support in the Dockerfile has not been verified end to end.

## Deployment files

```text
mathdoc/
├── compose.yaml
├── config.toml
├── mdc
└── secrets/
    └── database-password     # generated on first start; preserve it
```

`compose.yaml` selects the images, published port and storage. `config.toml` is a
commented template for `public_origin`; it can remain empty for local or SSH
access. `mdc` is an optional CLI wrapper. No `.env` or database password input is
required. The database port and account are internal deployment details.

## Download and start

The **Publish runtime** workflow publishes the tested image to GHCR and attaches
`mathdoc-deployment-linux-amd64.tar.gz` to a matching GitHub release. The Compose
file in that bundle pins the runtime by digest. Releases starting with **v0.6.0**
use this deployment format.

```sh
mkdir mathdoc
curl --fail --location \
  https://github.com/mathdocument/MathDoc/releases/latest/download/mathdoc-deployment-linux-amd64.tar.gz \
  -o mathdoc-deployment.tar.gz
tar -xzf mathdoc-deployment.tar.gz -C mathdoc
cd mathdoc
docker compose pull
docker compose up -d --wait --wait-timeout 180
./mdc status
```

The database generates a unique random password before its first initialization.
It is stored in `secrets/database-password`, readable by the deploying user and
the runtime's group, and mounted read-only in the runtime. Restarting or upgrading
does not regenerate it. If the database already exists but the password file is
missing or empty, startup fails and asks you to restore the original file.
Passwords are not embedded in either image or Compose's environment values.

Keep the password file with database backups. Anyone administering Docker can
access the deployment and its credentials. Do not put the deployment's generated
`secrets/` directory in source control.

## Create a project

```sh
./mdc init myproject
./mdc start myproject/main
./mdc status
```

`init` creates the graph in TerminusDB and is not needed again after a restart.
Lean 4.33.1 is already available. Projects with other pinned versions can install
them into the tools volume:

```sh
docker compose exec -T runtime elan toolchain install leanprover/lean4:VERSION
```

Replace `VERSION` with the project's release, such as `v4.33.1`. Project settings,
including its toolchain and dependency lockfile, remain versioned in the database.
The tools volume has a `lean/` directory and can accommodate future Rocq
installations; the deployment does not currently install a Rocq compiler.

## Access from another machine

The runtime listens on `0.0.0.0:17843` inside its container. Compose publishes it
only at **`127.0.0.1:17843` on the Docker host** by default. The database has no
published port and uses a private backend network. Runtime downloads use a
separate network with outbound access.

For SSH access, run this on your computer and keep it open:

```sh
ssh -N -o ExitOnForwardFailure=yes -L 17843:127.0.0.1:17843 user@SERVER
```

Then open `http://localhost:17843`. Leave `public_origin` unset.

For direct access on a trusted LAN, edit the runtime's port mapping in
`compose.yaml` to `"0.0.0.0:17843:17843"`, and set the address users will open:

```toml
public_origin = "http://192.0.2.10:17843"
```

Use the server's real address, then recreate the runtime:

```sh
docker compose up -d --force-recreate --wait runtime
```

For public access, put an authenticated HTTPS reverse proxy, optionally backed
by SSO, in front of MathDoc. With a proxy running on the host, keep the loopback
port mapping and forward to `http://127.0.0.1:17843`. Preserve the external Host,
paths and queries, and support WebSocket upgrades and long-lived connections.
Configure `public_origin` as the **browser-facing** address, for example
`https://mathdoc.example.com`. A containerized proxy can instead share a network
with the runtime, with no runtime host port published.

`public_origin` checks website addresses; it neither publishes ports nor logs in
users. Loopback requests remain allowed. MathDoc currently has no per-project user
permissions, and Lean programs execute as the runtime user. Access is intended
for trusted authors. See [Configuration](../../reference/configuration/).

To change the host port, edit only the middle number in the mapping, for example
`"127.0.0.1:17844:17843"`. The application still uses 17843 inside its container.
Update the tunnel or proxy accordingly. `MDC_PORT` is also accepted as an optional
host-port override; there is no need to create an `.env` file for it.

## CLI and agents

The `mdc` wrapper finds the Compose file beside itself and runs the container CLI
with stdin preserved. It works from any current directory. A shell function can
make it available as `mdc` everywhere:

```sh
mdc() { "$HOME/mathdoc/mdc" "$@"; }
mdc new -p myproject/main -t 'Example'
mdc show -p myproject/main 'Example'
# Use the revision returned by show to protect concurrent edits.
mdc edit -p myproject/main 'Example' --revision "$REV" < proof.lean
mdc lean check -p myproject/main 'Example'
```

The host reads `proof.lean` and streams it to the CLI. Agents can use the wrapper
over SSH. The CLI discovers the running service inside the container; no remote
`--url` client or host compiler is needed.

File arguments refer to container paths. Stream graph import/export instead:

```sh
mdc export -p myproject/main > graph.json
mdc init imported
mdc start imported/main
mdc import -p imported/main /dev/stdin < graph.json
```

For multiple input files, such as `project latex set`, use `docker compose cp` to
copy them into the runtime and remove them afterwards. Graph exports include
source and project settings, not full database history or compiler caches.

## Restart, upgrade and roll back

The foreground server remembers started branches in the cache volume and restores
them after container restarts. Explicitly stopping a branch removes it from that
selection. Use Compose to stop the whole deployment: bare `mdc stop` exits the
service process, which Docker's restart policy can start again.

```sh
./mdc stop myproject/main
./mdc start myproject/main
docker compose logs --tail 100 -f runtime
docker compose stop
docker compose up -d --wait
```

For an upgrade, save browser drafts and back up data. Download the new deployment
bundle into a **separate directory**. Keep a copy of the old Compose file, then
replace only `compose.yaml` and `mdc` in the existing deployment. Preserve
`config.toml` and `secrets/`, and reapply any local port/network changes to Compose.
Then run:

```sh
docker compose pull
docker compose up -d --no-build --wait --wait-timeout 180
```

To return to an older runtime, restore its image digest (or the previous Compose
file) and recreate the service. Check release notes for data-format compatibility;
changing an image cannot undo database changes. Do not use `down --volumes` during
an upgrade or rollback.

## Persistent storage and backups

| Default volume | Contents |
| --- | --- |
| `mathdoc-vol-data` | All graphs, source, branches and commit history. |
| `mathdoc-vol-cache` | Generated sources, dependencies, compiler caches and branch restart selection. |
| `mathdoc-vol-tools` | Elan state and additional Lean toolchains under `lean/`; room for future compiler tools. |

Lean 4.33.1 itself stays in the image; the tools volume contains a link to it.
The cache volume can be regenerated, but losing it also loses branch restart
selection. Do not reuse native compiler caches from a different OS or architecture.

The default Compose project name is `mathdoc`. Using `docker compose -p NAME`
changes container and volume prefixes consistently; keep that name stable across
upgrades. This deployment does not automatically attach to native development's
`mathdoc-terminus-data` or the former server deployment's `mathdoc-server_*` volumes.
Migrate an existing installation explicitly through graph import or a compatible
full database restore, including its original password. Never point a fresh
password at an existing database.

For a consistent full-history backup, stop the services and archive the database
volume. These commands use the already available runtime image as a helper:

```sh
docker compose stop
docker compose run --rm --no-deps --user 0 --entrypoint tar \
  -v mathdoc-vol-data:/backup-data:ro runtime -C /backup-data -czf - . \
  > terminus-backup.tar.gz
tar -czf mathdoc-config-backup.tar.gz compose.yaml config.toml secrets
docker compose up -d --wait
```

Use the actual data volume name if the project name was customized. Store the
configuration backup privately because it includes the database password. Restore
both data and the original password before starting the deployment. The database
container reapplies the password file's runtime-readable permissions on startup.

`docker compose down` retains the named volumes. **`docker compose down --volumes`
deletes all three, including the graphs and history.**

## Build locally

On a development/build machine with a checkout, Docker and Buildx:

```sh
docker build -t mathdoc-runtime:local .
python3 tests/docker-smoke.py
./scripts/package-deployment ./dist/mathdoc-deployment mathdoc-runtime:local
cd dist/mathdoc-deployment
docker compose up -d --pull never --wait --wait-timeout 180
./mdc status
```

The export directory must not already exist: the packaging script refuses to
overwrite configuration or credentials. The exported directory is independent of
the checkout. Build hosts need no native Rust or Node installations. Native
application development uses the root `compose.yaml` for its development database;
`compose.server.yaml` is the template exported for server deployment. Image builds
use the Dockerfile directly, without another Compose file or a development image.

The Docker test creates an isolated deployment from the exported files, chooses
unused ports and unique resource names, and removes only its own containers and
volumes. It checks the bundled toolchain offline, automatic credentials, HTTP
origins, CLI stdin, Lean compilation, LaTeX, restarts, recreation, down/up, cache
reuse and recovery from a missing or empty password file.

## Publish a release

1. Update the package version in `Cargo.toml` and `Cargo.lock`, the default runtime
   image in `compose.server.yaml`, and version examples in the documentation.
   Commit the changes and run `./scripts/check`.
2. Push the commit and wait for **Release check** to pass on that exact commit.
3. Create and push the matching annotated tag, for example `v0.6.0`, then publish
   a GitHub release for that tag with release notes. Pushing a tag alone does not
   publish the runtime.
4. Wait for **Publish runtime** to finish. The `runtime-release.yml` workflow
   builds and smoke-tests the Linux AMD64 image, pushes it to
   `ghcr.io/mathdocument/mathdoc-runtime:0.6.0`, then packages the exact pushed
   digest and uploads `mathdoc-deployment-linux-amd64.tar.gz` to the release.
5. Configure the GHCR package as public once so servers can pull without registry
   login. Download the release bundle into a fresh directory, pull the images
   without registry credentials, and verify startup before announcing the release.

Keep published tags and versioned images fixed. Ship corrections under a new
version instead of replacing an existing release's code or image.

A manual workflow run publishes a `sha-COMMIT` tag and uploads the deployment
bundle as a workflow artifact. Local builds and smoke tests never publish images.
