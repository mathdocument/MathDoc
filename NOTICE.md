# Engineering notices

## `mdc sync` retains one directory file descriptor per pending rollback

Observed while syncing the ETP workspace with 47,433 mdocs and roughly
150,000 mirror writes.

- `workdraft::transaction::apply_changes` retains every `AppliedWrite` until
  the source-block manifest is committed so the whole operation can roll back.
- Each `AppliedWrite` currently owns a `DirectoryBinding`, which owns an open
  parent-directory file descriptor.
- The number of live descriptors therefore grows linearly with the number of
  mirror writes. On the observed macOS host, opening descriptors failed around
  61,437 even though `RLIMIT_NOFILE` reported a higher value.
- The user-visible error only showed the outer context:
  `could not verify the generation of ...: inspecting ...`; the underlying
  `EMFILE` was not included by the CLI's one-line error rendering.
- The transaction did successfully roll back the attempted mirror writes.

Suggested fix: make `AppliedWrite` and `AppliedRename` retain a lightweight
snapshot of the bound directory chain (paths plus inode identities), not the
open descriptors. Reopen the directory only during rollback, verify the saved
identity chain, and then run the existing generation checks and rollback.
Add a regression test that holds hundreds of applied-write receipts and
asserts that the process file-descriptor count remains bounded. The existing
ancestor-replacement, external-edit, rollback, and full test suites must still
pass.
