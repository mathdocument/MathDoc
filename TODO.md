# Security and Correctness TODO

Repository-wide audit of `HEAD` (`5615631`) on 2026-07-10. This file records
confirmed, actionable defects separately from architectural hardening and test
coverage. Severity reflects data-loss/security impact, not implementation size.

## P1 - High-Value Correctness and Hardening

### [ ] QUERY-001 Escape SQL LIKE metacharacters and support documented paths

**References:** `src/indcache/queries.rs:972-1042`,
`src/indcache/mod.rs:352-385`.

`%` and `_` are interpreted as wildcards in search/fnode resolution. Documented
suffixless paths such as `notes/theorem` are not expanded to `.mdoc`.

**Acceptance:** Escape LIKE metacharacters with an explicit escape character and
generate suffixless `.mdoc` path candidates. Test literal `%`, `_`, escape
characters, and root/nested suffixless paths.

### [ ] FILE-001 Define metadata preservation for atomic replacement

**References:** `src/safe_file.rs:6-13`, `src/safe_file.rs:145-204`.

Replacement copies mode bits only and installs a new inode. ACLs, xattrs,
ownership details, hard-link relationships, macOS metadata, and Windows streams
can be silently lost.

**Acceptance:** Preserve explicitly supported security metadata and reject files
whose unsupported metadata or hard links cannot be preserved. Add platform tests
for ACL/xattr and hard-link behavior.

### [ ] WORK-001 Make work/back marker rules symmetric

**References:** `src/depgraph/workback.rs:120-191`,
`src/depgraph/workback.rs:272-370`, `src/cli/cmd_work.rs:521-527`.

Unsafe marker-like content is checked for node blocks during merge, but not for
ambles; back can also import content that the next work generation rejects.

**Acceptance:** Use one escaping/validation rule for node, preamble, and
postamble content in both directions. Work must reject unsafe ambles before any
write; back must reject content that cannot be generated again.

### [ ] WORK-002 Make `mdc back` command-wide transactional

**References:** `src/cli/cmd_work.rs:328-399`,
`src/cli/cmd_work.rs:606-691`.

Validation and rollback are per source type. One type can commit before another
type fails, so command exit 1 can mean partial success, with order depending on
directory iteration and fatal errors.

**Acceptance:** Prepare and validate every active source type first, then apply
all changes through one rollback set. Any failure must leave all nodes, ambles,
and sidecars unchanged unless an explicit best-effort mode is selected.

### [ ] COMP-001 Contain compiler descendants on Windows

**References:** `src/compiler/mod.rs:249-271`, `src/compiler/mod.rs:368-383`,
`src/compiler/mod.rs:581-587`.

Non-Unix cleanup kills only the leader. Descendants can survive timeout and hold
pipes, causing leaked processes and detached drain threads.

**Acceptance:** Use a Windows Job Object with kill-on-close and cancellable pipe
I/O. Timeout/cancellation tests must leave no descendants and join every drain.

### [ ] COMP-002 Correct timeout completion ordering

**References:** `src/compiler/mod.rs:676-765`.

Timeout is checked before `try_wait`, and elapsed time is checked again after a
successful leader exit. A process that exited near the deadline can be reported
as timed out because the parent was descheduled or output draining finished late.

**Acceptance:** Record process completion independently of drain completion and
never replace an observed successful status with timeout. Add near-deadline and
slow-drain tests with bounded wall-clock cleanup.

### [ ] COMP-003 Make compiler paths deterministic and lossless

**References:** `src/compiler/python.rs:23-25`, `src/compiler/mod.rs:120-123`,
`src/compiler/mod.rs:635-637`.

Python receives lossy UTF-8 paths and inherits the caller's working directory,
so imports/output vary by invocation directory and non-Unicode paths can break.

**Acceptance:** Pass `Path`/`OsStr` arguments without lossy conversion and use a
documented deterministic working directory. Compiling from any workspace
subdirectory must behave identically.

### [ ] COMP-004 Reject `_CoqProject` symlinks

**References:** `src/compiler/rocq.rs:52-57`.

A dangling `.mdc/rocq/_CoqProject` symlink is followed by `std::fs::write`, which
can create an external target.

**Acceptance:** Require missing or regular non-symlink managed files and create
them through safe no-clobber persistence. External targets must stay unchanged.

### [ ] CLI-001 Propagate editor and cache-update failures

**References:** `src/cli/cmd_core.rs:11-20`, `src/cli/cmd_tui.rs:558-576`.

`mdc edit` ignores nonzero editor status. TUI edit flow ignores spawn/status and
cache failures while reporting success.

**Acceptance:** Nonzero editor status must make the command fail and suppress
success notification. Surface cache refresh errors and add CLI/TUI tests using a
failing editor.

### [ ] CLI-002 Make TUI terminal restoration RAII-safe

**References:** `src/cli/cmd_tui.rs:47-59`, `src/cli/cmd_tui.rs:549-566`.

Initialization, event-loop, panic, or cleanup failures can leave raw mode,
alternate screen, or hidden cursor active.

**Acceptance:** Introduce an RAII terminal guard whose `Drop` independently
best-effort restores every terminal state. Fault-injection tests must cover each
initialization and event-loop failure point.

### [ ] FRONT-001 Freeze overlay mutation targets

**References:** `web/src/App.svelte:138-156`,
`web/src/components/RmDepOverlay.svelte:21-79`.

Navigation keeps the old editor visible while loading. An open remove-dependency
overlay can reactively switch `fnode`, accept an out-of-order children response,
and submit old selections against a new node.

**Acceptance:** Capture an immutable target when opening an overlay, close/block
it on committed navigation, and generation-token or abort every load. Delayed A
responses must never render or mutate after B is active.

### [ ] FRONT-002 Track every pending mutation and form draft

**References:** `web/src/lib/unsaved.ts:19-46`,
`web/src/components/AddBlockControl.svelte:28-40`,
`web/src/components/AddDepOverlay.svelte:77-114`,
`web/src/components/RmDepOverlay.svelte:61-79`,
`web/src/components/NewNodeOverlay.svelte:61-81`.

Only block save/delete and title save use the pending-mutation barrier. Add block,
dependency changes, and node creation can finish after dismissal, navigation, or
unload. New-node form text is not draft-protected.

**Acceptance:** Register every mutation and nonempty creation form, block or
confirm dismissal/unload while pending, and gate callbacks by component lifetime
and immutable target. Beforeunload must include pending operations.

### [ ] FRONT-003 Surface navigation and refresh failures

**References:** `web/src/lib/state.svelte.ts:136-145`,
`web/src/App.svelte:121-129`, `web/src/components/EditorPane.svelte:122-136`.

Failed navigation/refresh requests silently keep stale content, allowing users
to believe an explicit refresh succeeded.

**Acceptance:** Preserve drafts but display a persistent error and retry action.
Explicit refresh must report every failed subrequest and must not label old data
as refreshed.

### [ ] FRONT-004 Stop force-graph work when hidden or idle

**References:** `web/src/App.svelte:287-290`,
`web/src/components/ForceGraph.svelte:212-225`,
`web/src/components/ForceGraph.svelte:376-453`.

The hidden graph remains mounted with a permanent RAF loop. Losing mouseup
outside the canvas can leave a pinned node and nonzero simulation alpha target.

**Acceptance:** Use pointer capture and cancel/blur cleanup; stop RAF and
simulation when hidden or settled. Tests must observe no recurring frame after
idle and correct drag cleanup outside the canvas.

### [ ] FRONT-005 Make keyboard safety equal to mouse safety

**References:** `web/src/components/SearchOverlay.svelte:54-58`,
`web/src/components/AddDepOverlay.svelte:77-85`.

Enter submits a selected broken node even though its row button is disabled.

**Acceptance:** Submission must reject broken entries and keyboard navigation
must skip disabled rows. Enter and click must have identical outcomes.

### [ ] FRONT-006 Make overlays truly modal

**References:** `web/src/components/SearchOverlay.svelte:83-97`,
`web/src/components/AddDepOverlay.svelte:150-162`,
`web/src/components/NewNodeOverlay.svelte:85-100`.

Focus can leave dialogs and activate background controls, enabling conflicting
overlays or mutations under an open dialog.

**Acceptance:** Mark dialogs modal, make the background inert, trap and restore
focus, and enforce one top-level overlay. Tab/Shift-Tab must remain in the active
dialog.

### [ ] RELEASE-001 Enforce frontend source/dist synchronization

**References:** `Cargo.toml:39-41`, `src/web/assets.rs:5-12`,
`web/package.json:6-10`, `GUIDE.md:321-325`.

Release builds can embed stale committed assets when `web/src` changes without
`npm run build`.

**Acceptance:** Release CI must run `npm ci`, check, build, fail on a `web/dist`
diff, and smoke-test that every URL in `index.html` is embedded and served with
the expected MIME/cache policy.

## P2 - Consistency and Recovery

### [ ] CACHE-003 Distinguish duplicate targets from missing targets

**References:** `src/indcache/refresh.rs:906-944`.

Target counts of zero and greater than one both produce a missing issue, so a
duplicate target can be reported as duplicate and missing.

**Acceptance:** Generate missing only for count zero; count greater than one is
duplicate/ambiguous only. Assert both dependency and graph-check reports.

### [ ] SCHEMA-001 Reject future database schema versions before mutation

**References:** `src/indcache/schema.rs:129-135`,
`src/indcache/schema.rs:213-228`.

An older binary opens a larger `user_version`; `CREATE_SQL` executes before the
version check and may mutate a future database.

**Acceptance:** Read `user_version` before schema-changing statements on an
existing DB and reject versions above the supported version without changing it.

### [ ] PARSE-001 Reject malformed duplicate directives

**References:** `src/mdocnode/node.rs:300-305`,
`src/mdocnode/node.rs:401-437`.

Two empty `@dep:` blocks are accepted, `@dep: trailing text` is accepted, and
duplicate metadata keys silently overwrite one another.

**Acceptance:** Track an explicit dep-block seen flag, require exact directives,
and reject duplicate metadata keys with parse tests.

### [ ] INIT-001 Make initialization idempotent and crash-recoverable

**References:** `src/cli/cmd_core.rs:55-65`, `src/config.rs:215-233`.

A crash after creating `.mdc` but before managed files leaves a partial workspace;
later init returns early instead of repairing missing files.

**Acceptance:** Re-running init must atomically create every missing managed file
without overwriting user files and must reject a symlinked/non-directory `.mdc`.

### [ ] WEB-006 Unify parent-linked and standalone new-node paths

**References:** `src/web/api.rs:539-608`, `src/depgraph/mod.rs:874-892`.

Parent-linked creation accepts absolute paths, doubles an existing `.mdoc`
suffix, and treats empty input differently from standalone creation.

**Acceptance:** Route both branches through one parser; consistently default
empty/`.` paths, reject absolute paths, and reject or normalize `.mdoc` suffixes.

### [ ] COMP-005 Make interrupted Lean setup recoverable

**References:** `src/compiler/lean.rs:76-101`.

A killed `lake init` can leave a lakefile that is treated as proof of complete
setup, permanently skipping repair.

**Acceptance:** Stage initialization or write a completion marker only after
successful validation. Interrupted setup must retry or repair automatically.

### [ ] CLI-003 Preserve meaningful compiler exit semantics

**References:** `src/compiler/mod.rs:1043-1055`,
`src/cli/cmd_work.rs:297-312`.

Timeout 124 and tool-not-found 127 are printed but collapsed to command exit 1;
only interruption is propagated.

**Acceptance:** Define deterministic aggregation and preserve meaningful status
for single failures. Add end-to-end timeout/tool-missing shell-status tests.

### [ ] WORK-003 Use a stable sidecar digest

**References:** `src/cli/cmd_work.rs:18-35`.

`DefaultHasher` is unspecified across Rust versions and only 64-bit, making it a
poor persistent data-loss guard.

**Acceptance:** Use a named stable digest such as BLAKE3 or SHA-256 and record the
algorithm/version in the sidecar with migration tests.

### [ ] FRONT-007 Synchronize focused node with browser history

**References:** `web/src/App.svelte:65-75`,
`web/src/lib/state.svelte.ts:118-126`.

Navigation does not update URL history; reload can reopen the original hash and
browser Back/Forward does not match application history.

**Acceptance:** Update history only after committed navigation and process
popstate through the same draft/pending guards. Bookmark, reload, Back, and
Forward must preserve focus.

## Architectural Hardening

### [ ] ARCH-001 Eliminate pathname compare/rename TOCTOU

**References:** `src/safe_file.rs:145-204`,
`src/mdocnode/node.rs:68-121`.

The final unchanged check and rename are separate pathname operations, and parent
directories are not pinned. A hostile concurrent filesystem can replace the
target or parent between checks.

**Acceptance:** Serialize writers and use pinned directory handles with no-follow
`openat`/`renameat`-style operations or a platform compare-exchange primitive.

### [ ] ARCH-002 Add crash recovery for multi-file mutations

**References:** `src/depgraph/mod.rs:432-468`,
`src/cli/cmd_work.rs:606-691`.

Node creation/linking and work/back use compensating rollback but no durable
transaction journal. Process death or power loss can leave orphan nodes,
mismatched sidecars, or partially applied source types.

**Acceptance:** Introduce a fsynced mutation journal/generation manifest with
startup roll-forward or rollback. Successful return must also surface required
directory-sync failures.

### [ ] ARCH-003 Define compiler containment guarantees

**References:** `src/compiler/mod.rs:412-427`,
`src/compiler/mod.rs:561-579`, `src/compiler/mod.rs:914-941`.

Unix process groups cannot contain a compiler that deliberately calls `setsid`;
the current test manually kills the escaped process.

**Acceptance:** Document the supported cleanup boundary and use stronger
containment where available, such as Linux cgroups. Tests must distinguish
ordinary descendants from intentionally escaped processes.

### [ ] ARCH-004 Move cancellation ownership out of compiler core

**References:** `src/compiler/mod.rs:17-22`,
`src/compiler/mod.rs:440-559`.

Per-call Unix signal masking is not a reliable cancellation model in a generic
multithreaded host, and concurrent calls compete for process-directed signals.

**Acceptance:** Add an explicit per-request cancellation token. CLI signal
handling should live at the application boundary; concurrent compile tests must
cancel only the intended request.

## Required Test Infrastructure

### [ ] TEST-001 Add deterministic mutation failpoints

Cover file changes between parse and save, failures after persistence, rollback
failure, directory sync failure, concurrent replacement, and process-level graph
mutation races. Existing Web concurrency tests share one in-process mutex and do
not exercise these defects.

### [ ] TEST-002 Add frontend component/browser tests

`web/package.json` has no test command. Add delayed-response tests for mutation
barriers, stale overlays, navigation/refresh failure, modal focus, disabled
keyboard submission, force-graph idle/drag cleanup, browser history, and unload
guards.

### [ ] TEST-003 Add cross-platform compiler CI

Exercise Windows Job Object/process-tree cleanup, near-deadline timeout ordering,
slow drains, exit-code propagation, non-Unicode paths, and external compilers.
Current real compiler tests silently skip when tools are absent.

### [ ] TEST-004 Add real migration and filesystem-failure fixtures

Build databases from historical DDL rather than lowering `user_version` on a
current schema. Add exact mtime/size collision, unreadable subtree, future schema,
symlinked control path, malicious cached path, ACL/xattr, and hard-link fixtures.

### [ ] TEST-005 Add release reproducibility checks

Build the frontend in a clean temporary directory, compare it byte-for-byte with
`web/dist`, package the crate, and smoke-test every embedded asset reference,
MIME type, Host policy, and cache header in both release and `dev-web` modes.
