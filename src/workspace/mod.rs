use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

mod mutation_lock;
mod safe_file;

pub(crate) use mutation_lock::WorkspaceMutationLock;
pub(crate) use safe_file::{
    atomic_create_if_missing, atomic_replace, ensure_regular_directory,
    ensure_regular_directory_exists, AppliedWrite, FileConflict, FileSnapshot,
};

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
                if entry.depth() > 0 && entry.path().join(".mdc").is_dir() {
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

/// Convert `path` to a POSIX-style string relative to `root`.
/// Caller must ensure both are canonicalized. Falls back to the absolute path on error.
pub fn to_rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
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

pub(crate) fn validate_workspace_relative_path(relative: &Path, original: &Path) -> Result<()> {
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
