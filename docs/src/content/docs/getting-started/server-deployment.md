---
title: Deploy with Docker Compose
---

The server needs Docker Engine and its Compose plugin (2.24 or later). It does not
need a Git checkout, Node, Python or Lean. LeanGround runs elsewhere (or in its own
deployment on the same host) and is reached over HTTP.

The deployment runs five containers from two pinned database images and the
`mathdoc-runtime` image:

| Service | What it runs |
| --- | --- |
| `database` | TerminusDB: documents and their history. |
| `postgres` | PostgreSQL: collaboration data, workspace permissions, proof environments. |
| `migrate` | One-shot schema migration; `runtime` and `worker` wait for it to succeed. |
| `runtime` | The API and web editor, published only on `127.0.0.1:${MDC_PORT:-17843}`. |
| `worker` | Background jobs: node conversion, LeanGround calls, sync, assembly, writeback. |

The runtime image contains the compiled backend, the built editor, the LaTeX
renderer with its pinned Python packages and `mdc` on the `PATH`. It contains no
Lean. It runs as uid/gid 10001; logs use Docker's bounded `local` driver.

## Deployment files

```text
mathdoc/
├── compose.yaml
├── mathdoc.env            # settings: LeanGround address, public origin
├── mdc                    # optional CLI wrapper
└── secrets/
    ├── actors.json        # you create: access tokens
    ├── leanground-token   # you create: the coordinator's LeanGround fact token
    ├── database-password  # generated on first start; preserve it
    └── postgres-password  # generated on first start; preserve it
```

The **Publish runtime** workflow publishes the tested image to GHCR and attaches
`mathdoc-deployment-linux-amd64.tar.gz` to a matching GitHub release; its
`compose.yaml` pins the runtime by digest. From a checkout, `scripts/package-deployment
DIRECTORY [IMAGE]` writes the same files. The release workflow targets Linux AMD64.

## Configure and start

```sh
mkdir mathdoc
curl --fail --location \
  https://github.com/mathdocument/MathDoc/releases/latest/download/mathdoc-deployment-linux-amd64.tar.gz \
  -o mathdoc-deployment.tar.gz
tar -xzf mathdoc-deployment.tar.gz -C mathdoc
cd mathdoc
```

Edit `mathdoc.env`:

| Variable | Value |
| --- | --- |
| `LEANGROUND_SERVER_URL` | LeanGround's URL as seen from inside the containers. |
| `LEANGROUND_ACTOR` | The coordinator's actor name in LeanGround. |
| `MDC_PUBLIC_ORIGIN` | Optional: the browser-facing origin behind a proxy or LAN address. |

Create the two secrets you own (`od -An -N32 -tx1 /dev/urandom | tr -d ' \n'` makes a
random token):

```sh
cat > secrets/actors.json <<'EOF'
{"alice": {"token": "A-LONG-RANDOM-TOKEN", "admin": true},
 "bob":   {"token": "ANOTHER-LONG-RANDOM-TOKEN", "admin": false}}
EOF
printf '%s\n' 'THE-COORDINATOR-FACT-TOKEN' > secrets/leanground-token
```

One `actors.json` entry per person or agent; tokens must be at least 16 characters
and distinct. The LeanGround token must belong to `LEANGROUND_ACTOR` in LeanGround's
`fact_tokens`. Then:

```sh
docker compose pull
docker compose up -d --wait --wait-timeout 180
MDC_TOKEN=A-LONG-RANDOM-TOKEN ./mdc status
```

The database containers generate a unique random password for TerminusDB and for
PostgreSQL before their first initialization and store them in `secrets/`. On
every start they set every file in `secrets/` to mode 0440, owned by the directory's
owner and group 10001, so the deploying user and the runtime can read them. A
restart or upgrade does not regenerate a password. If a database already exists but
its password file is missing or empty, startup fails and asks you to restore the
original file. Passwords and tokens are not embedded in images or Compose
environment values; the backend reads them from the files (`*_FILE` variables).

Keep `secrets/` with the database backups and out of source control. Anyone
administering Docker can access the deployment and its credentials. Changing a
generated password file does not change the password stored in an existing volume.

LeanGround must be current: its definitions migration applied (`leanground db schema
--only 10-definitions`) and its worker rebuilt and reinstalled (an older worker rejects
every request that uses definitions). See [Proofs with LeanGround](../../reference/proofs/).

## Create a project

An administrator creates databases. The creator owns the new database and can
grant roles to others (see [Workspaces](../../concepts/workspaces/)):

```sh
export MDC_TOKEN=your-admin-token
./mdc init myproject
./mdc grant bob editor -p myproject/main
```

`mdc` runs the CLI inside the `runtime` container, passing arguments and stdin. It
requires `MDC_TOKEN` in your environment and forwards `MDC_PROJECT` if set:

```sh
./mdc new -p myproject/main -t 'Example'
./mdc edit -p myproject/main 'Example' --type lean < proof.lean
```

The shell reads `proof.lean` on the host and streams it into the container. File
arguments (`import FILE`, `project latex set --preamble FILE --bib FILE`) are
container paths: stream through stdin (`./mdc -p x/main import /dev/stdin < graph.json`)
or copy files in with `docker compose cp`.

Agents on other machines do not need the wrapper: they call the HTTP API through
the tunnel or proxy with their own token. See [Agent interface](../../reference/agents/).

## Access from another machine

The runtime listens on `0.0.0.0:17843` inside its container. Compose publishes it
only at **`127.0.0.1:17843` on the Docker host**; `MDC_PORT` changes the host port.
The databases have no published port and use a private backend network; the runtime
and worker also join an outbound network to reach LeanGround.

For SSH access, run this on your computer and keep it open:

```sh
ssh -N -o ExitOnForwardFailure=yes -L 17843:127.0.0.1:17843 user@SERVER
```

Open `http://localhost:17843` and sign in with your token. Leave
`MDC_PUBLIC_ORIGIN` unset.

For direct access on a trusted LAN, edit the runtime's port mapping in
`compose.yaml` to `"0.0.0.0:17843:17843"`, set `MDC_PUBLIC_ORIGIN` in `mathdoc.env`
to the address users open (for example `http://192.0.2.10:17843`) and recreate:

```sh
docker compose up -d --force-recreate --wait runtime
```

For public access, put an HTTPS reverse proxy in front of MathDoc and set
`MDC_PUBLIC_ORIGIN` to the browser-facing address; see
[Configuration](../../reference/configuration/#reverse-proxy). The origin check
rejects other hosts; tokens authenticate users.

## Restarts and upgrades

Branches load on demand; there is nothing to start or stop per branch. All
long-running services use `restart: unless-stopped`.

```sh
docker compose pull
docker compose up -d --wait --wait-timeout 180
docker compose logs --tail 100 -f runtime worker
docker compose stop
```

`up` reruns `migrate` before starting the API and worker. Save browser drafts
before upgrading. Databases created by an earlier MathDoc version have no
workspace record and are visible to administrators only until an administrator
assigns an owner (`set_owner`, see [HTTP API](../../reference/http-api/#directory)).

## Volumes and backups

| Volume | Contents |
| --- | --- |
| `mathdoc-vol-data` | All document databases, branches, sources and commit history. |
| `mathdoc-vol-postgres` | Collaboration projects, events, jobs, workspace permissions, proof environments. |
| `mathdoc-vol-cache` | Cache directory; regenerable. |

The Compose project name (default `mathdoc`) sets the volume and container prefix;
keep it unchanged. Back up both database volumes together with `secrets/`:
permissions and proof requests in PostgreSQL refer to databases, branches and nodes
in TerminusDB. `down` keeps named volumes; **`down --volumes` deletes all data and
history**.

```sh
docker compose stop
for v in data postgres; do
  docker run --rm -v "mathdoc-vol-$v:/data:ro" debian:bookworm-slim \
    tar -C /data -czf - . > "mathdoc-$v.tar.gz"
done
docker compose up -d --wait --wait-timeout 180
```

A graph export (`mdc export`) captures one branch snapshot, without history,
permissions or collaboration data.

## Verification

Developers can check a built image with `./scripts/check docker`, which builds
`mathdoc-runtime:local` and runs `tests/docker-smoke.py`. The test exports a
deployment into a temporary directory with a free loopback port, isolated names and
disposable volumes, then checks: no Lean in the image, generated passwords and their
permissions, uid 10001, origin checks, token authentication, the secret-file rules,
CLI mutations through the `mdc` wrapper, a LaTeX preview, the initial
`not_submitted` certification status, restart, recreation and down/up, that no
password or token appears in logs or `docker inspect`, and recovery from a lost or
empty password file. It points the worker at an unreachable LeanGround and removes
only its own containers and volumes. On macOS with colima, set `TMPDIR` under your
home directory so the VM can see the bind-mounted `secrets/`.
