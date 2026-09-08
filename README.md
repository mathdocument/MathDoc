# MathDoc

MathDoc is a locally hosted, versioned graph of mathematical knowledge. TerminusDB
stores nodes, dependencies and source blocks; the browser and `mdc` CLI use the
same service. Nodes have exact names and full UUIDs. Supported blocks are **text,
Lean, Rocq and LaTeX**. Lean has a native Monaco/Infoview editor backed by Lean Server.

## Start locally

Install Rust, Docker and Elan, then build the service (the committed browser assets
are embedded in the binary):

```sh
cargo build --release
elan toolchain install leanprover/lean4:v4.33.1
python3 scripts/start-db.py            # creates a private .env if needed
python3 scripts/run-local.py init       # once for each new database
python3 scripts/run-local.py serve
```

Open [MathDoc](http://127.0.0.1:7599). The supplied Compose configuration exposes
TerminusDB only on loopback and keeps its data in a persistent volume.
The Docker CLI helper works without a Compose plugin. Alternatively, start a new
installation with [compose.yaml](compose.yaml) and a private `.env`; use one
startup method consistently so two containers do not compete for the same port. The service and browser must run on the same host;
the HTTP service checks loopback Host and same Origin. Lean metaprograms execute
as the local service user: this deployment is for trusted local authors.

The helper loads `.env` without passing credentials in command arguments.
`MDC_TERMINUS_URL`, `MDC_TERMINUS_USER`, `MDC_CACHE_DIR` and `MDC_BIN` are optional.
Direct CLI clients only need `MDC_URL` (default `http://127.0.0.1:7599`).

## Authoring and agents

```sh
mdc new -t 'My theorem'
printf 'theorem exampleA : True := by trivial\n' | mdc edit 'My theorem' --type lean
mdc lean check 'My theorem'
mdc lean check 'My theorem' --build
mdc lean goals 'My theorem' --line 0 --column 31
mdc show 'My theorem'
mdc dep add 'My theorem' --target 'Earlier lemma'
mdc graph check
```

`edit` reads source from stdin. Agents can pass `--revision` from `show` to reject
stale edits/checks. Browser writes always carry revisions. Concurrent conflicting
writes fail rather than overwrite newer work. Dependency cycles are rejected
atomically. Duplicate display names require a UUID reference; paths and UUID
prefixes are not references.

Lean checks wait for diagnostics for the requested source version. `passed`
means no Lean errors; `certified` additionally requires every direct managed
`Lib.*` import to match `dep` and every dependency to be certified. Lean's normal
`sorry` warnings remain warnings. `--build` also creates native Lake artifacts,
including `.olean`; checks without it use incremental server elaboration.

Repeated checks reuse an input key covering Lean source, stable module identity,
transitive dependencies and the locked environment. Text/title/Rocq/LaTeX edits
do not invalidate Lean results. Source files under `.mdc-service` are disposable
build inputs. Never edit them. The service performs no workspace file scans.
Only startup or an external database commit reloads the graph projection.

The browser's left pane edits Lean; its right pane shows native Infoview goals,
messages and widgets. Save, Save & check and Save & build use the shared service.
Each browser tab has an isolated draft environment. Switching nodes keeps its
Lean connection and the two most recently used document workers alive. Selecting a
node refreshes its dependency snapshot; changed dependencies restart that worker.
Reload the environment after project configuration changes. CLI checks always
capture the current database dependency versions. Rocq and LaTeX remain editable;
Rocq compilation and language-server integration are not provided yet.

## Libraries and history

Use **Lean project** in the browser, or `mdc project show` / `mdc project set`
(JSON on stdin). Configuration contains `toolchain`, `lakefile` (TOML declaring
`lean_lib Lib`) and `manifest` (the complete `lake-manifest.json` as a string).
External libraries must use Git dependencies locked to full commits, including
transitive packages. Mutable local path dependencies are rejected. The native
Lake project supports libraries beyond Mathlib. Cold toolchain/library downloads
and first compilation still take time; later builds reuse `.lake` artifacts.
`MDC_LEAN_TIMEOUT_SECONDS` raises the default 300-second per-request limit for
large initial builds. Changed library resolutions must be saved as a new project
manifest before they can be certified.

Every mutation creates a TerminusDB commit. `mdc history` shows recent commits;
`mdc branch create agent-name` creates a database branch. Run a service for that
branch with `--branch agent-name serve --bind 127.0.0.1:7600`, and direct an agent
at that URL. Branch merge/rebase remains a TerminusDB operation; no merge UI is
included. Database history and application Git history are separate.

## Explicit migration and export

```sh
mdc export > mathdoc-backup.json
mdc export 'My theorem' > theorem.mdoc
mdc import mathdoc-backup.json
mdc import /absolute/path/to/legacy-workspace
```

Import is an explicit, atomic operation. It preserves valid UUIDs, dependencies,
source blocks and legacy Lean module identities, and rejects overwriting existing
UUIDs, unsupported block types or invalid graphs. Review the export and use an
empty database for a complete project import. Import never changes the source
workspace. Python blocks require explicit conversion/removal before import.
`sync`, `back`, file-path references and the MathDoc VS Code extension are retired.

## Development checks

```sh
npm --prefix web ci
npm --prefix web run check
npm --prefix web test
npm --prefix web run build
cargo test --locked
python3 scripts/test-local.py test_database test_service test_lean_service
npm --prefix web run test:e2e
# Optional: real database graph benchmark (47,435 nodes; 368,017 edges).
python3 scripts/test-local.py test_service_scale
```

Integration tests require the local database, `.env`, pinned native Lean and a
Playwright Chromium installation. They create uniquely named test databases.
The compiler currently serializes CLI checks per service branch and limits live
browser editors to eight; use separate branches/services for independent work.
One latest result per node is kept in memory; certified results are also loaded
on demand from the cache after a restart, using the full input key. The most recent
CLI file retains its live Lean worker. Lake artifacts survive restarts. Only one
service may own a branch cache at a time.

[Documentation](https://mathdocument.github.io/MathDoc/) · [MIT License](LICENSE)
