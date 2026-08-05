use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

mod mutation_lock;
mod safe_file;

pub(crate) use mutation_lock::WorkspaceMutationLock;
pub(crate) use safe_file::{
    atomic_create_if_missing_beneath, ensure_regular_directory_tree, ensure_regular_file_beneath,
    error_has_file_conflict, error_has_infrastructure_failure, regular_directory_exists_beneath,
    remove_directory_tree_beneath, remove_empty_directory_beneath, AppliedRename, AppliedWrite,
    FileSnapshot, FileSnapshotBatch, PersistenceRecoveryError, ReadFileSnapshot,
};
#[cfg(test)]
pub(crate) use safe_file::{set_test_hook, FileConflict, TestHookPoint};

/// Initialize the workspace control directory and its default configuration.
pub fn initialize(root: &Path) -> Result<bool> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing workspace root {}", root.display()))?;
    let control_dir = root.join(".mdc");
    let mut changed = ensure_regular_directory_tree(&root, &control_dir)?;
    changed |= atomic_create_if_missing_beneath(
        &root,
        &control_dir.join("config.toml"),
        crate::config::config_template().as_bytes(),
    )?;
    Ok(changed)
}

/// Walk up from `start` looking for a `.mdc/` directory. Returns the workspace root if found.
pub fn find_mdcroot(start: &Path) -> Option<PathBuf> {
    let mut current = start.canonicalize().ok()?;
    loop {
        if is_regular_directory(&current.join(".mdc")) {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Canonicalize a workspace root and require its `.mdc` control path to be a
/// real directory rather than a symlink.
pub fn validate_mdcroot(root: &Path) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing workspace root {}", root.display()))?;
    let mdc = root.join(".mdc");
    let meta = std::fs::symlink_metadata(&mdc)
        .with_context(|| format!("inspecting workspace control directory {}", mdc.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        bail!(
            "workspace control path must be a real directory: {}",
            mdc.display()
        );
    }
    Ok(root)
}

fn is_regular_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| !meta.file_type().is_symlink() && meta.is_dir())
        .unwrap_or(false)
}

/// Find a nested mdoc root inside the given `root` workspace, searching from `path` upward.
/// `root` and `path` must be canonical (absolute, resolved). Returns the nested root if found.
pub fn find_nested_mdcroot(root: &Path, path: &Path) -> Option<PathBuf> {
    if !path.starts_with(root) {
        return None;
    }
    let mut current = path;
    loop {
        if current == root {
            return None;
        }
        if is_regular_directory(&current.join(".mdc")) {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// Iterate all `.mdoc` files under `root`, skipping `.mdc/` directories and nested workspaces.
pub fn iter_mdoc_files(root: &Path) -> impl Iterator<Item = Result<PathBuf>> + '_ {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.file_type().is_dir() {
                // Skip .mdc/ directories at any level
                if entry.file_name() == ".mdc" {
                    return false;
                }
                // Skip non-root directories that are nested workspace roots
                if entry.depth() > 0 && is_regular_directory(&entry.path().join(".mdc")) {
                    return false;
                }
            }
            true
        })
        .filter_map(|result| match result {
            Ok(entry)
                if entry.file_type().is_file()
                    && entry.path().extension().and_then(|e| e.to_str()) == Some("mdoc") =>
            {
                Some(Ok(entry.into_path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error.into())),
        })
}

/// Convert `path` to a display string relative to `root`.
/// Caller must ensure both are canonicalized. Falls back to the absolute path on error.
pub fn to_rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Resolve a `.mdoc` path without allowing it to escape the workspace, enter
/// `.mdc`, traverse symlinks, or cross into a nested workspace.
pub(crate) fn resolve_mdoc_path(root: &Path, file_path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let candidate = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        root.join(file_path)
    };

    if candidate.extension().and_then(|ext| ext.to_str()) != Some("mdoc") {
        bail!("mdoc path must end in .mdoc: {}", file_path.display());
    }
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing symlinked mdoc path: {}", file_path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Ok(relative) = candidate.strip_prefix(&root) {
        validate_workspace_relative_path(relative, file_path)?;
        let mut current = root.clone();
        for component in relative.components() {
            current.push(component.as_os_str());
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    bail!(
                        "refusing mdoc path with symlink component {}",
                        current.display()
                    )
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error.into()),
            }
        }
    }
    let resolved =
        match candidate.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut existing = candidate.as_path();
                let mut suffix = Vec::new();
                while !existing.exists() {
                    suffix.push(existing.file_name().ok_or_else(|| {
                        anyhow::anyhow!("invalid mdoc path {}", file_path.display())
                    })?);
                    existing = existing.parent().ok_or_else(|| {
                        anyhow::anyhow!("invalid mdoc path {}", file_path.display())
                    })?;
                }
                let mut resolved = existing.canonicalize()?;
                for component in suffix.into_iter().rev() {
                    resolved.push(component);
                }
                resolved
            }
            Err(error) => return Err(error.into()),
        };
    if !resolved.starts_with(&root) {
        bail!("mdoc path is outside workspace: {}", file_path.display());
    }
    let relative = resolved
        .strip_prefix(&root)
        .expect("workspace containment checked above");
    validate_workspace_relative_path(relative, file_path)?;

    let parent = resolved.parent().unwrap_or(&resolved);
    let parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    if let Some(nested) = find_nested_mdcroot(&root, &parent) {
        bail!("mdoc path is inside nested mdoc root: {}", nested.display());
    }
    Ok(resolved)
}

/// Resolve an unused path for a new `.mdoc` file. `raw_target` is relative to
/// the workspace and may omit the extension; `.` uses the node fnode as name.
pub(crate) fn resolve_new_mdoc_path(root: &Path, raw_target: &str, fnode: &str) -> Result<PathBuf> {
    let target = raw_target.trim();
    let candidate = if target.is_empty() || target == "." {
        root.join(format!("{fnode}.mdoc"))
    } else {
        let relative = Path::new(target);
        if relative.is_absolute() {
            bail!("target path must be relative to the mdoc root");
        }
        let joined = root.join(relative);
        if joined.extension().and_then(|ext| ext.to_str()) == Some("mdoc") {
            joined
        } else {
            let stem = joined
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid target path"))?
                .to_string_lossy();
            joined.with_file_name(format!("{stem}.mdoc"))
        }
    };

    validate_new_mdoc_path(root, &candidate)
}

pub(crate) fn validate_new_mdoc_path(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let resolved = resolve_mdoc_path(root, candidate)?;
    if std::fs::symlink_metadata(&resolved).is_ok() {
        bail!("mdoc file already exists: {}", resolved.display());
    }
    Ok(resolved)
}

fn validate_workspace_relative_path(relative: &Path, original: &Path) -> Result<()> {
    for (index, component) in relative.components().enumerate() {
        let std::path::Component::Normal(name) = component else {
            bail!("invalid mdoc path: {}", original.display());
        };
        if index == 0 && name == ".mdc" {
            bail!("mdoc path cannot be inside .mdc: {}", original.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn resolve_mdoc_path_rejects_a_final_symlink() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("target.mdoc");
        let link = workspace.path().join("link.mdoc");
        std::fs::write(&target, "@fnode: target\n@title: Target\n").unwrap();
        symlink(&target, &link).unwrap();

        assert!(resolve_mdoc_path(workspace.path(), &link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn initialize_cannot_follow_replaced_control_directory() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let control = workspace.path().join(".mdc");
        let displaced = workspace.path().join("displaced-mdc");
        std::fs::create_dir(&control).unwrap();
        let hook_control = control.clone();
        let hook_displaced = displaced.clone();
        let hook_outside = outside.path().to_path_buf();
        set_test_hook(TestHookPoint::WriteBeforeDirectoryBinding, move || {
            std::fs::rename(hook_control, hook_displaced).unwrap();
            symlink(hook_outside, control).unwrap();
        });

        let error = initialize(workspace.path()).unwrap_err();

        assert!(error_has_file_conflict(&error));
        assert!(!outside.path().join("config.toml").exists());
        assert!(!displaced.join("config.toml").exists());
    }
}
