use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Walk up from `start` looking for a `.mdc/` directory. Returns the workspace root if found.
pub fn find_mdcroot(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".mdc").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
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
        if current.join(".mdc").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// Iterate all `.mdoc` files under `root`, skipping `.mdc/` directories and nested workspaces.
pub fn iter_mdoc_files(root: &Path) -> impl Iterator<Item = PathBuf> + '_ {
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
        .filter_map(|result| {
            let entry = result.ok()?;
            if entry.file_type().is_file()
                && entry.path().extension().and_then(|e| e.to_str()) == Some("mdoc")
            {
                Some(entry.into_path())
            } else {
                None
            }
        })
}

/// Convert `path` to a POSIX-style string relative to `root`.
/// Caller must ensure both are canonicalized. Falls back to the absolute path on error.
pub fn to_rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}
