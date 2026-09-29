---
title: Deploy with Docker and SSH
---

This deployment serves a team from one Linux server reached through SSH. The host
needs Git and Docker with the Compose plugin; it does not need Node, Python, Lean
or root access. LeanGround runs elsewhere (or in its own deployment on the same
host) and is reached over HTTP.

`compose.server.yaml` is separate from the development `compose.yaml` and uses
its own named volumes. It does not attach to or modify an existing local
installation.

## Services

| Service | What it runs |
| --- | --- |
| `terminusdb` | Documents and their history. No published port. |
| `postgres` | Collaboration data, workspace permissions, proof environments. No published port. |
| `migrate` | One-shot `node coordinator/dist/main.js migrate`; `mdc` and `worker` wait for it to succeed. |
| `mdc` | The API and web editor (`serve`), published only on `127.0.0.1:${MDC_PORT:-17843}`. |
| `worker` | The LeanGround worker (`worker`). |

The image (`Dockerfile`) is built from `node:26-bookworm-slim`. It contains the
compiled backend, the built editor, the LaTeX renderer with its pinned Python
packages in `/opt/latex`, and `mdc` on the `PATH`. It contains no Lean. It runs as
the non-root `node` user; logs use Docker's bounded `local` driver.

## Configure and start

```sh
git clone https://github.com/mathdocument/MathDoc.git
cd MathDoc
cp config.example.env .env
chmod 600 .env
```

Edit `.env`. Compose requires these values and refuses to start without them:

| Variable | Value |
| --- | --- |
| `MDC_TERMINUS_PASSWORD` | A new random password; it initializes the TerminusDB volume. |
| `MDC_POSTGRES_PASSWORD` | A new random password; it initializes the PostgreSQL volume. |
| `MDC_ACTORS` | JSON map from actor name to `{"token": …, "admin": true|false}`. One entry per person or agent; tokens at least 16 characters and distinct. |
| `LEANGROUND_SERVER_URL` | LeanGround's URL as seen from inside the containers. |
| `LEANGROUND_FACT_TOKEN`, `LEANGROUND_ACTOR` | The coordinator's service identity in LeanGround. |

For example, a random token or password: `od -An -N32 -tx1 /dev/urandom | tr -d ' \n'`.
Compose sets the database URLs itself; the other variables in `config.example.env`
are for running from a checkout. `.env` is ignored by Git and excluded from the
image. Changing a password in `.env` later does not change the password stored
in an existing volume.

```sh
docker compose -f compose.server.yaml up -d --build --wait --wait-timeout 180
docker compose -f compose.server.yaml ps
```

Anyone with Docker access on the host can read `.env` and every volume.

## Open through SSH

Run this on your own computer and keep it open:

```sh
ssh -N -o ExitOnForwardFailure=yes -L 17843:127.0.0.1:17843 user@SERVER
```

Open `http://localhost:17843` and sign in with your token from `MDC_ACTORS`.
If the server's port is taken, set `MDC_PORT=17844` in `.env`, rerun Compose and
tunnel that port. If only your local port is taken, use
`-L 17844:127.0.0.1:17843` and open `http://localhost:17844`. Do not publish the
port on `0.0.0.0` to avoid the tunnel; for a reverse proxy see
[Configuration](../../reference/configuration/#reverse-proxy).

## Create a project

An administrator creates databases. The creator owns the new database and can
grant roles to others (see [Workspaces](../../concepts/workspaces/)):

```sh
export MDC_TOKEN=your-admin-token
./scripts/mdc-docker init myproject
./scripts/mdc-docker grant bob editor -p myproject/main
```

`scripts/mdc-docker` runs `mdc` inside the running `mdc` container from any
directory, passing arguments and stdin. It requires `MDC_TOKEN` in your
environment and forwards `MDC_PROJECT` if set. A shell function saves typing:

```sh
mdc() { "$HOME/MathDoc/scripts/mdc-docker" "$@"; }
mdc new -p myproject/main -t 'Example'
mdc edit -p myproject/main 'Example' --type lean < proof.lean
```

The shell reads `proof.lean` on the host and streams it into the container. File
arguments (`import FILE`, `project latex set --preamble FILE --bib FILE`) are
container paths: stream through stdin (`mdc import -p x/main /dev/stdin < graph.json`)
or copy files in with `docker compose -f compose.server.yaml cp`.

Agents on other machines do not need this wrapper: they call the HTTP API through
the tunnel or proxy with their own token. See [Agent interface](../../reference/agents/).

## Restarts and upgrades

Branches load on demand; there is nothing to start or stop per branch. All
services use `restart: unless-stopped`.

```sh
git pull --ff-only
docker compose -f compose.server.yaml up -d --build --wait --wait-timeout 180

docker compose -f compose.server.yaml logs --tail 100 -f mdc worker
docker compose -f compose.server.yaml stop
```

`up` reruns `migrate` before starting the API and worker. Save browser drafts
before upgrading. Databases created by an earlier MathDoc version have no
workspace record and are visible to administrators only until an administrator
assigns an owner (`set_owner`, see [HTTP API](../../reference/http-api/#directory)).

## Volumes and backups

| Default volume | Contents |
| --- | --- |
| `mathdoc-server_terminus-data` | All databases, branches, sources and commit history. |
| `mathdoc-server_postgres-data` | Collaboration projects, events, jobs, workspace permissions, proof environments. |
| `mathdoc-server_mdc-cache` | Cache directory (`MDC_CACHE_DIR`); regenerable. |

Back up both database volumes together: permissions and proof requests in
PostgreSQL refer to databases, branches and nodes in TerminusDB. The Compose
project name sets the volume prefix; keep it unchanged. `down` keeps named volumes;
**`down --volumes` deletes all data and history**.

A consistent offline backup:

```sh
docker compose -f compose.server.yaml stop
for v in terminus-data postgres-data; do
  docker run --rm -v "mathdoc-server_$v:/data:ro" debian:bookworm-slim \
    tar -C /data -czf - . > "mathdoc-$v.tar.gz"
done
docker compose -f compose.server.yaml up -d --wait --wait-timeout 180
```

A graph export (`mdc export`) captures one branch snapshot, without history,
permissions or collaboration data.

## Verification

Developers can check a built image with:

```sh
docker compose -f compose.server.yaml build
python3 tests/docker-smoke.py
```

The test starts a separate Compose project with random passwords, a free loopback
port and disposable volumes. It checks that the service runs as non-root, that
the image has no Lean toolchain, token authentication, CLI mutations, branch
creation, a LaTeX preview, the initial `not_submitted` certification status and
that data and a running worker survive a restart and a container recreation. It points the worker at an unreachable
LeanGround unless `LEANGROUND_SERVER_URL` and `LEANGROUND_FACT_TOKEN` are set, and
removes only its own containers and volumes.
