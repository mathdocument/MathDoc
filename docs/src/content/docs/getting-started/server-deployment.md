---
title: Deploy with Docker Compose
---

The server needs Docker Engine and its Compose plugin (2.24 or later). It does not
need a Git checkout, Node, Python or Lean. LeanGround runs elsewhere (or in its own
deployment on the same host) and is reached over HTTP.

The deployment runs five containers from the TerminusDB image (pinned by digest),
the PostgreSQL 17 image and the `mathdoc-runtime` image:

| Service | What it runs |
| --- | --- |
| `database` | TerminusDB: documents and their history. |
| `postgres` | PostgreSQL: project boards, jobs and events, workspace permissions, proof environments, delegated tokens. |
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
├── state/                 # created by mdc: replay records of review, pick, branch new, proof command
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
| `LEANGROUND_SERVER_URL` | LeanGround's URL as seen from inside the containers. A LeanGround on the Docker host itself is `http://host.docker.internal:PORT` with Docker Desktop and colima. |
| `LEANGROUND_ACTOR` | The coordinator's actor name in LeanGround. |
| `MDC_PUBLIC_ORIGIN` | Optional: the browser-facing origin behind a proxy or LAN address. |
| `LEANGROUND_TIMEOUT_S` | Optional: seconds a LeanGround call that runs Lean may take (default 960). |

`mathdoc.env` is passed to the containers; do not set `MDC_PORT` in it (see
[Access from another machine](#access-from-another-machine)).

Create the two secrets you own (`od -An -N32 -tx1 /dev/urandom | tr -d ' \n'` makes a
random token):

```sh
cat > secrets/actors.json <<'EOF'
{"alice": {"token": "A-LONG-RANDOM-TOKEN", "admin": true},
 "bob":   {"token": "ANOTHER-LONG-RANDOM-TOKEN", "admin": false}}
EOF
printf '%s\n' 'THE-COORDINATOR-FACT-TOKEN' > secrets/leanground-token
```

One `actors.json` entry per person or agent; names use letters, digits, `_` and
`-`, and tokens must be at least 16 characters and distinct. Agents can instead
use [delegated tokens](../../concepts/workspaces/#delegated-tokens) of a person.
The LeanGround token must belong to `LEANGROUND_ACTOR` in LeanGround's
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
--only 10-definitions`), its worker rebuilt and reinstalled (an older worker rejects
every request that uses definitions) and its sketch component enabled. See
[Requirements](../installation/#requirements) and
[Proofs with LeanGround](../../reference/proofs/).

LeanGround may restart or be away for a while: calls that run Lean wait up to
`LEANGROUND_TIMEOUT_S` (default 960 seconds, enough for a freshly started LeanGround
to import Mathlib), and a job whose try fails because LeanGround cannot be reached
or times out is retried 5 s, 15 s, 45 s, … later, at most 15 minutes apart and 8 tries
in all (about 40 minutes); see [Configuration](../../reference/configuration/).

## Create a database

An administrator creates databases. The creator owns the new database and can
grant roles to others (see [Workspaces](../../concepts/workspaces/)):

```sh
export MDC_TOKEN=your-admin-token
./mdc init myproject
./mdc grant bob editor -p myproject/main
```

The [Quick start](../quick-start/) continues from here: nodes, the proof
environment, the project and an agent's proof work, all with `./mdc`.

### The `mdc` wrapper

`mdc` runs the CLI in a one-off container from the runtime's image, sharing the
runtime's network, as your user and group, with the current directory mounted, and
passes arguments and stdin. The `runtime` container must be running. It requires
`MDC_TOKEN` in your environment and forwards `MDC_PROJECT` if set:

```sh
./mdc new -p myproject/main -t 'Example'
./mdc edit -p myproject/main 'Example' --type lean < proof.lean
```

File arguments (`import FILE`, `project latex set --preamble FILE --bib FILE`) are
paths under the current directory. The proof work commands write their goal files
and `.mdc/` into it, owned by you; the replay records of `review`, `pick`,
`branch new` and `proof command` are kept in `state/` next to the wrapper:

```sh
mkdir work && cd work
MDC_TOKEN=AGENT-TOKEN ../mdc pull 'Main theorem' -p myproject/main
# edit the goal file, then
MDC_TOKEN=AGENT-TOKEN ../mdc prove *-main_theorem.lean --wait
MDC_TOKEN=AGENT-TOKEN ../mdc done
```

With colima or another VM, run it from a directory the VM shares (by default your
home directory); an unshared directory is mounted empty.

Agents on other machines do not need the wrapper: they run `mdc` from a checkout with
`MDC_URL` set, or call the HTTP API, through the tunnel or proxy with their own token.
See [Agent interface](../../reference/agents/).

## Access from another machine

The runtime listens on `0.0.0.0:17843` inside its container. Compose publishes it
only at **`127.0.0.1:17843` on the Docker host**. `MDC_PORT`, set in the shell that
runs `docker compose` or in a `.env` file beside `compose.yaml`, changes the host
port; the container port stays 17843, which the `mdc` wrapper and the health check
use. The databases have no published port and use a private backend network; the
runtime and worker also join an outbound network to reach LeanGround.

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
docker compose logs --tail 100 -f runtime worker
docker compose stop
docker compose up -d --wait --wait-timeout 180
```

A release's `compose.yaml` pins the runtime image by digest, so `docker compose
pull` alone keeps the same version. To upgrade, extract the new release's bundle
into an empty directory and copy its `compose.yaml` and `mdc` over the old ones,
keeping `mathdoc.env`, `secrets/` and `state/` (and redoing any edit to the port
mapping). Then:

```sh
docker compose pull
docker compose up -d --wait --wait-timeout 180
```

`up` reruns `migrate` before starting the API and worker. Save browser drafts
before upgrading and reload open tabs afterwards. Databases created by an earlier MathDoc version have no
workspace record and are visible to administrators only until an administrator
assigns an owner (`set_owner`, see [HTTP API](../../reference/http-api/#directory)).

## Volumes and backups

| Volume | Contents |
| --- | --- |
| `mathdoc-vol-data` | All document databases, branches, sources and commit history. |
| `mathdoc-vol-postgres` | Project boards (goals, certificate IDs, tasks, leases, routes), events, jobs, workspace permissions, proof environments, delegated tokens. |
| `mathdoc-vol-cache` | Cache directory; regenerable. |

The Compose project name (default `mathdoc`) sets the volume and container prefix;
keep it unchanged. Back up both database volumes together with `secrets/`:
permissions and project boards in PostgreSQL refer to databases, branches and nodes
in TerminusDB. Certificates and their sources are kept by LeanGround, not in these
volumes; boards keep their IDs, not their sources. `down` keeps named volumes;
**`down --volumes` deletes all data and history**.

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
CLI mutations through the `mdc` wrapper, a proof work command writing into a host
directory as the calling user, a LaTeX preview, the initial `not_submitted`
certification status, restart, recreation and down/up, that no password or token
appears in logs or `docker inspect`, and recovery from a lost or empty password
file. It points the worker at an unreachable LeanGround and removes only its own
containers and volumes. With `MDC_SMOKE_LEANGROUND_URL` (as seen from the
containers), `MDC_SMOKE_LEANGROUND_TOKEN` and `MDC_SMOKE_LEANGROUND_ACTOR` set, it
uses that LeanGround instead and also proves a theorem through the deployment: an
agent pulls, proves and finishes it from a host directory, and the owner assembles
and accepts it (`MDC_SMOKE_BASE_KEY`, default 1, names a base that has `Nat`). On
macOS with colima, set `TMPDIR` under your home directory so the VM can see the
bind-mounted `secrets/`.
