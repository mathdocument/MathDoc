---
title: Safe Mutations
description: Path validation, snapshot checks, quarantine replacement, batching, and rollback limits.
---

MathDoc treats workspace writes as generation-sensitive mutations. The goal is to avoid
silently replacing bytes when a path, ancestor, or file generation changed after it was
observed.

## Store-owned mutations

`WorkspaceStore::create_node()` and `WorkspaceStore::replace_node()` route through
`MutationSession`, the sole owner of the durable dirty marker and commit/abort index
recovery. Creation validates the output against the store root and workspace mutation
lock, renders the document, creates missing parents, revalidates the final path, and
writes against `FileSnapshot::Missing`.

On index failure it attempts file rollback and path-index repair. Either recovery can
fail, and those failures remain part of the returned error. `MdocNode::render()` only
validates and serializes bytes; production filesystem creation stays store-owned.

The application batch APIs prevalidate and render all output documents before writes.
A single `MutationSession` persists its dirty marker, applies guarded replacements, and
calls one batched index upsert. On failure, every completed replacement is offered rollback
in reverse order; the enclosing session repairs the index from current files. A conflicting
external edit is preserved even when that prevents complete rollback. If files and index
commit successfully but the final mutation-lock check fails, the committed files remain in
place and the dirty marker is available for recovery on the next open.

## Descriptor-relative replacement

Safe writes bind the validated parent-directory inode, create temporary files relative
to that descriptor, and use same-directory no-clobber quarantine renames before
replacement or rollback.

Ancestor identities and the quarantined generation are checked around persistence. If
generation or directory identity becomes uncertain, MathDoc returns `FileConflict` and
restores the quarantined name when safe. Otherwise it leaves the quarantine file in
place rather than deleting or overwriting another writer's bytes.

Replacement snapshots cover content, inode identity, permissions, ownership, and
supported extended attributes. Writes reject symlink or nonregular targets and, where
supported, read-only, hard-linked, ACL-bearing, or unsupported-flag files.

Replacing a file quarantines its old generation before installing the new one, so the
target pathname may be briefly absent.

## Multi-file batches

`sync` uses operation-scoped `FileSnapshotBatch` instances. Each maintains a bounded
cache of no-follow parent-directory descriptors, records every traversed directory
identity, and verifies those generations before writes apply.

During `sync`, mdocs are processed in batches of 2,048 sources. Mdoc and mirror reads use
at most twelve scoped workers with deterministic result ordering. Read-only mirror
snapshots omit replacement metadata; a selected write target is recaptured as a
complete snapshot and its observed content is checked again. `back` uses the same
parallel lightweight scan and hydrates complete snapshots only for write targets.

A clean observation-cache hit uses descriptor-relative `fstatat` batches before returning.
Generation checks include inode, size, mtime, ctime, permissions, and ownership. A cache
miss retains exact byte-level input validation. The manifest and workspace lock generation
are revalidated before a fast return; operations that write validate unchanged inputs
before and after applying changes and revalidate the manifest generation.

Completed operations are retained until the source-block manifest commits. If a later
step fails, MathDoc attempts rollback in reverse order. Rollback is best-effort and can
itself fail; multi-file batches are not crash-atomic. Reconciliation revalidates the
workspace lock generation before each write, removal, rename, and manifest update.
Sparse-placeholder removals retain rollback receipts like other transaction changes.

Directory-tree cleanup first renames the selected generation to a same-parent
quarantine. Recursive removal stays descriptor-relative to that quarantined directory.
A replacement that appears at the original pathname is preserved and reported as a
conflict rather than traversed or removed. An initially absent target is also accepted
only after its bound parent and missing generation are revalidated.

## Concurrency boundary

`WorkspaceWorkLock` and `WorkspaceMutationLock` are distinct interprocess capabilities.
The work lock serializes mirror reconciliation and compiler workspace use but does not
exclude unrelated node mutations; the mutation lock serializes graph and node writes.
`WorkspaceWorkLock::root()` validates the held lock generation, and `validate_root()`
binds callers to the requested workspace. Workdraft `sync`/`back` and compiler dispatch
therefore receive the work lock explicitly.

These cooperative locks coordinate MathDoc processes but cannot constrain arbitrary
external editors or other non-cooperating processes. A held lock is accepted only while
its opened descriptor, pathname generation, and `.mdc` directory identity still match.
The work-to-mutation handoff acquires the mutation lock before revalidating the work lock,
so reconciliation state cannot silently cross a replaced lock generation.

Unix does not provide a portable atomic content-compare-and-unlink operation. On macOS,
for example, a process can continue writing through a descriptor opened before a file
was quarantined. MathDoc verifies the quarantined inode immediately before unlink and
preserves or restores it if such a write is observed.

A write in the final interval between that verification and `unlinkat` cannot be
detected atomically; the open descriptor retains the inode until closed. This remaining
race is why the system reports best-effort safety rather than claiming arbitrary-writer
or crash atomicity.
