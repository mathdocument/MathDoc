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

pub(super) fn apply_changes(
    manifest_path: &Path,
    manifest_snapshot: &FileSnapshot,
    manifest: &SourceBlockManifest,
    inputs: &[(PathBuf, FileSnapshot)],
    writes: Vec<PreparedWrite>,
    removals: Vec<PreparedRemoval>,
    renames: Vec<PreparedRename>,
) -> Result<()> {
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
    ensure_unchanged(manifest_path, manifest_snapshot)?;

    let mut manifest_content = serde_json::to_vec_pretty(manifest)?;
    manifest_content.push(b'\n');
    let manifest_changed = manifest_snapshot.content() != Some(manifest_content.as_slice());
    let write_paths: HashSet<&Path> = writes.iter().map(|write| write.path.as_path()).collect();
    let mut applied = Vec::new();
    let result = (|| -> Result<()> {
        for rename in &renames {
            applied.push(AppliedChange::Rename(
                rename.snapshot.case_rename(&rename.from, &rename.to)?,
            ));
        }
        for write in &writes {
            applied.push(AppliedChange::Write(
                write.snapshot.replace(&write.path, &write.content)?,
            ));
        }
        for removal in &removals {
            if let Some(write) = removal.snapshot.remove(&removal.path)? {
                applied.push(AppliedChange::Write(write));
            }
        }
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
        if manifest_changed {
            applied.push(AppliedChange::Write(
                manifest_snapshot.replace(manifest_path, &manifest_content)?,
            ));
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
        remove_empty_parents(&removal.path, &removal.type_root);
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
        let manifest_path = dir.path().join("source-blocks.json");
        let original_manifest = b"{\"version\":2,\"sources\":{}}\n";
        std::fs::write(&manifest_path, original_manifest).unwrap();
        let manifest_snapshot = FileSnapshot::capture(&manifest_path).unwrap();
        let manifest = super::super::manifest::parse_manifest(&manifest_snapshot, &manifest_path)
            .unwrap()
            .manifest;

        let first_path = dir.path().join("first.lean");
        std::fs::write(&first_path, b"first before\n").unwrap();
        let first_snapshot = FileSnapshot::capture(&first_path).unwrap();

        let failing_path = dir.path().join("readonly.lean");
        std::fs::write(&failing_path, b"readonly before\n").unwrap();
        std::fs::set_permissions(&failing_path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let failing_snapshot = FileSnapshot::capture(&failing_path).unwrap();

        let error = apply_changes(
            &manifest_path,
            &manifest_snapshot,
            &manifest,
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
}
