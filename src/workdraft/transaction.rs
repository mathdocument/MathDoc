use anyhow::{bail, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::workspace::{AppliedRename, AppliedWrite, FileSnapshot};

use super::manifest::SourceBlockManifest;
use super::mirror::remove_empty_parents;

pub(super) struct PreparedWrite {
    pub(super) path: PathBuf,
    pub(super) snapshot: FileSnapshot,
    pub(super) content: Vec<u8>,
}

pub(super) struct PreparedRemoval {
    pub(super) path: PathBuf,
    pub(super) type_root: PathBuf,
    pub(super) snapshot: FileSnapshot,
}

pub(super) struct PreparedRename {
    pub(super) from: PathBuf,
    pub(super) to: PathBuf,
    pub(super) snapshot: FileSnapshot,
}

pub(super) struct PreparedManifest<'a> {
    pub(super) path: &'a Path,
    pub(super) snapshot: &'a FileSnapshot,
    pub(super) content: &'a SourceBlockManifest,
}

pub(super) fn apply_changes(
    allowed_root: &Path,
    manifest: PreparedManifest<'_>,
    inputs: &[(PathBuf, FileSnapshot)],
    writes: Vec<PreparedWrite>,
    removals: Vec<PreparedRemoval>,
    renames: Vec<PreparedRename>,
) -> Result<()> {
    let _profile = crate::profile::scope("workdraft::transaction::apply_changes");
    let mut manifest_content = serde_json::to_vec_pretty(manifest.content)?;
    manifest_content.push(b'\n');
    let manifest_changed = manifest.snapshot.content() != Some(manifest_content.as_slice());
    if writes.is_empty() && removals.is_empty() && renames.is_empty() && !manifest_changed {
        let _phase = crate::profile::scope("workdraft::validate_noop_inputs");
        for chunk in inputs.chunks(2048) {
            let paths: Vec<_> = chunk.iter().map(|(path, _)| path.clone()).collect();
            let current = super::capture_files_parallel(allowed_root, &paths)?;
            for ((path, snapshot), current) in chunk.iter().zip(&current) {
                if !snapshot.matches(current) {
                    bail!("{} changed during source block operation", path.display());
                }
            }
        }
        ensure_unchanged(manifest.path, manifest.snapshot)?;
        return Ok(());
    }

    let validate_profile = crate::profile::scope("workdraft::validate_inputs_before");
    for (path, snapshot) in inputs {
        ensure_unchanged(path, snapshot)?;
    }
    for write in &writes {
        ensure_unchanged(&write.path, &write.snapshot)?;
    }
    for removal in &removals {
        ensure_unchanged(&removal.path, &removal.snapshot)?;
    }
    for rename in &renames {
        ensure_unchanged(&rename.from, &rename.snapshot)?;
    }
    ensure_unchanged(manifest.path, manifest.snapshot)?;
    drop(validate_profile);

    let write_paths: HashSet<&Path> = writes.iter().map(|write| write.path.as_path()).collect();
    let mut applied = Vec::new();
    let result = (|| -> Result<()> {
        for rename in &renames {
            applied.push(AppliedChange::Rename(rename.snapshot.case_rename_beneath(
                allowed_root,
                &rename.from,
                &rename.to,
            )?));
        }
        for write in &writes {
            applied.push(AppliedChange::Write(write.snapshot.replace_beneath(
                allowed_root,
                &write.path,
                &write.content,
            )?));
        }
        for removal in &removals {
            if let Some(write) = removal
                .snapshot
                .remove_beneath(allowed_root, &removal.path)?
            {
                applied.push(AppliedChange::Write(write));
            }
        }
        let validate_profile = crate::profile::scope("workdraft::validate_inputs_after");
        for (path, snapshot) in inputs {
            if write_paths.contains(path.as_path()) {
                continue;
            }
            if !snapshot.unchanged(path)? {
                bail!(
                    "{} changed while source block writes were applied",
                    path.display()
                );
            }
        }
        drop(validate_profile);
        if manifest_changed {
            applied.push(AppliedChange::Write(manifest.snapshot.replace_beneath(
                allowed_root,
                manifest.path,
                &manifest_content,
            )?));
        }
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(rollback_error) = rollback_changes(applied) {
            return Err(anyhow::anyhow!(
                "{error}; additionally failed to roll back source block operation: {rollback_error}"
            ));
        }
        return Err(error);
    }
    for removal in removals {
        remove_empty_parents(allowed_root, &removal.path, &removal.type_root);
    }
    Ok(())
}

fn ensure_unchanged(path: &Path, snapshot: &FileSnapshot) -> Result<()> {
    if !snapshot.unchanged(path)? {
        bail!("{} changed during source block operation", path.display());
    }
    Ok(())
}

enum AppliedChange {
    Write(AppliedWrite),
    Rename(AppliedRename),
}

fn rollback_changes(changes: Vec<AppliedChange>) -> Result<()> {
    let mut errors = Vec::new();
    for change in changes.into_iter().rev() {
        let result = match change {
            AppliedChange::Write(write) => write.rollback(),
            AppliedChange::Rename(rename) => rename.rollback(),
        };
        if let Err(error) = result {
            errors.push(error.to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("{}", errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_write_failure_rolls_back_applied_files_and_preserves_manifest() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let manifest_path = root.join("source-blocks.json");
        let original_manifest = b"{\"version\":2,\"sources\":{}}\n";
        std::fs::write(&manifest_path, original_manifest).unwrap();
        let manifest_snapshot = FileSnapshot::capture(&manifest_path).unwrap();
        let manifest = super::super::manifest::parse_manifest(&manifest_snapshot, &manifest_path)
            .unwrap()
            .manifest;

        let first_path = root.join("first.lean");
        std::fs::write(&first_path, b"first before\n").unwrap();
        let first_snapshot = FileSnapshot::capture(&first_path).unwrap();

        let failing_path = root.join("readonly.lean");
        std::fs::write(&failing_path, b"readonly before\n").unwrap();
        std::fs::set_permissions(&failing_path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let failing_snapshot = FileSnapshot::capture(&failing_path).unwrap();

        let error = apply_changes(
            &root,
            PreparedManifest {
                path: &manifest_path,
                snapshot: &manifest_snapshot,
                content: &manifest,
            },
            &[],
            vec![
                PreparedWrite {
                    path: first_path.clone(),
                    snapshot: first_snapshot,
                    content: b"first after\n".to_vec(),
                },
                PreparedWrite {
                    path: failing_path.clone(),
                    snapshot: failing_snapshot,
                    content: b"must fail\n".to_vec(),
                },
            ],
            Vec::new(),
            Vec::new(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("read-only file"));
        assert_eq!(std::fs::read(&first_path).unwrap(), b"first before\n");
        assert_eq!(std::fs::read(&failing_path).unwrap(), b"readonly before\n");
        assert_eq!(std::fs::read(&manifest_path).unwrap(), original_manifest);
    }

    #[test]
    fn noop_reconciliation_rejects_a_changed_input() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let manifest_path = root.join("source-blocks.json");
        std::fs::write(&manifest_path, b"{\"version\":2,\"sources\":{}}\n").unwrap();
        let initial_snapshot = FileSnapshot::capture(&manifest_path).unwrap();
        let manifest = super::super::manifest::parse_manifest(&initial_snapshot, &manifest_path)
            .unwrap()
            .manifest;
        let mut manifest_content = serde_json::to_vec_pretty(&manifest).unwrap();
        manifest_content.push(b'\n');
        std::fs::write(&manifest_path, manifest_content).unwrap();
        let manifest_snapshot = FileSnapshot::capture(&manifest_path).unwrap();

        let source_path = root.join("source.mdoc");
        std::fs::write(&source_path, b"before\n").unwrap();
        let source_snapshot = FileSnapshot::capture(&source_path).unwrap();
        std::fs::write(&source_path, b"after\n").unwrap();

        let error = apply_changes(
            &root,
            PreparedManifest {
                path: &manifest_path,
                snapshot: &manifest_snapshot,
                content: &manifest,
            },
            &[(source_path, source_snapshot)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("changed during source block operation"));
    }

    #[test]
    fn ancestor_swap_cannot_redirect_prepared_write() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let manifest_path = root.join("source-blocks.json");
        std::fs::write(&manifest_path, b"{\"version\":2,\"sources\":{}}\n").unwrap();
        let manifest_snapshot = FileSnapshot::capture(&manifest_path).unwrap();
        let manifest = super::super::manifest::parse_manifest(&manifest_snapshot, &manifest_path)
            .unwrap()
            .manifest;
        let ancestor = root.join("generated");
        std::fs::create_dir(&ancestor).unwrap();
        let path = ancestor.join("node.lean");
        let displaced = root.join("displaced");
        let hook_ancestor = ancestor.clone();
        let hook_outside = outside.path().to_path_buf();
        let hook_displaced = displaced.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::WriteBeforeDirectoryBinding,
            move || {
                std::fs::rename(hook_ancestor, hook_displaced).unwrap();
                symlink(hook_outside, ancestor).unwrap();
            },
        );

        let error = apply_changes(
            &root,
            PreparedManifest {
                path: &manifest_path,
                snapshot: &manifest_snapshot,
                content: &manifest,
            },
            &[],
            vec![PreparedWrite {
                path,
                snapshot: FileSnapshot::Missing,
                content: b"generated\n".to_vec(),
            }],
            Vec::new(),
            Vec::new(),
        )
        .unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert!(!outside.path().join("node.lean").exists());
        assert!(!displaced.join("node.lean").exists());
    }

    #[test]
    fn ancestor_swap_cannot_redirect_prepared_removal() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let manifest_path = root.join("source-blocks.json");
        std::fs::write(&manifest_path, b"{\"version\":2,\"sources\":{}}\n").unwrap();
        let manifest_snapshot = FileSnapshot::capture(&manifest_path).unwrap();
        let manifest = super::super::manifest::parse_manifest(&manifest_snapshot, &manifest_path)
            .unwrap()
            .manifest;
        let ancestor = root.join("generated");
        std::fs::create_dir(&ancestor).unwrap();
        let path = ancestor.join("node.lean");
        std::fs::write(&path, b"inside\n").unwrap();
        std::fs::write(outside.path().join("node.lean"), b"outside\n").unwrap();
        let snapshot = FileSnapshot::capture(&path).unwrap();
        let displaced = root.join("displaced");
        let hook_ancestor = ancestor.clone();
        let hook_outside = outside.path().to_path_buf();
        let hook_displaced = displaced.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::RemoveBeforeDirectoryBinding,
            move || {
                std::fs::rename(hook_ancestor, hook_displaced).unwrap();
                symlink(hook_outside, ancestor).unwrap();
            },
        );

        let error = apply_changes(
            &root,
            PreparedManifest {
                path: &manifest_path,
                snapshot: &manifest_snapshot,
                content: &manifest,
            },
            &[],
            Vec::new(),
            vec![PreparedRemoval {
                path,
                type_root: root.join("generated"),
                snapshot,
            }],
            Vec::new(),
        )
        .unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert_eq!(
            std::fs::read(outside.path().join("node.lean")).unwrap(),
            b"outside\n"
        );
        assert_eq!(
            std::fs::read(displaced.join("node.lean")).unwrap(),
            b"inside\n"
        );
    }

    #[test]
    fn ancestor_swap_cannot_redirect_prepared_case_rename() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let manifest_path = root.join("source-blocks.json");
        std::fs::write(&manifest_path, b"{\"version\":2,\"sources\":{}}\n").unwrap();
        let manifest_snapshot = FileSnapshot::capture(&manifest_path).unwrap();
        let manifest = super::super::manifest::parse_manifest(&manifest_snapshot, &manifest_path)
            .unwrap()
            .manifest;
        let ancestor = root.join("generated");
        std::fs::create_dir(&ancestor).unwrap();
        let from = ancestor.join("Node.lean");
        let to = ancestor.join("node.lean");
        std::fs::write(&from, b"inside\n").unwrap();
        std::fs::write(outside.path().join("Node.lean"), b"outside\n").unwrap();
        let snapshot = FileSnapshot::capture(&from).unwrap();
        let displaced = root.join("displaced");
        let hook_ancestor = ancestor.clone();
        let hook_outside = outside.path().to_path_buf();
        let hook_displaced = displaced.clone();
        crate::workspace::set_test_hook(
            crate::workspace::TestHookPoint::CaseRenameBeforeDirectoryBinding,
            move || {
                std::fs::rename(hook_ancestor, hook_displaced).unwrap();
                symlink(hook_outside, ancestor).unwrap();
            },
        );

        let error = apply_changes(
            &root,
            PreparedManifest {
                path: &manifest_path,
                snapshot: &manifest_snapshot,
                content: &manifest,
            },
            &[],
            Vec::new(),
            Vec::new(),
            vec![PreparedRename { from, to, snapshot }],
        )
        .unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert_eq!(
            std::fs::read(outside.path().join("Node.lean")).unwrap(),
            b"outside\n"
        );
        let outside_names = std::fs::read_dir(outside.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(outside_names, vec![std::ffi::OsString::from("Node.lean")]);
        assert_eq!(
            std::fs::read(displaced.join("Node.lean")).unwrap(),
            b"inside\n"
        );
    }
}
