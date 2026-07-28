use anyhow::{bail, Context, Result};
use std::fs::File;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

/// Workspace-wide interprocess lock for graph and node mutations.
pub(crate) struct WorkspaceMutationLock {
    file: File,
    root: PathBuf,
    control_identity: (u64, u64),
}

impl WorkspaceMutationLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        let root = super::validate_mdcroot(root)?;
        let control_path = root.join(".mdc");
        let control_identity = directory_identity(&control_path)?;
        let path = control_path.join("mutation.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
            .with_context(|| format!("opening workspace mutation lock {}", path.display()))?;
        if !file.metadata()?.is_file() {
            bail!(
                "workspace mutation lock is not a regular file: {}",
                path.display()
            );
        }
        loop {
            // SAFETY: `file` owns a live descriptor for the duration of the lock.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).with_context(|| {
                    format!("locking workspace mutation lock {}", path.display())
                });
            }
        }
        if directory_identity(&control_path)? != control_identity {
            bail!(
                "workspace control directory changed while acquiring its mutation lock: {}",
                control_path.display()
            );
        }
        Ok(Self {
            file,
            root,
            control_identity,
        })
    }

    pub(crate) fn root(&self) -> Result<&Path> {
        let control_path = self.root.join(".mdc");
        if directory_identity(&control_path)? != self.control_identity {
            bail!(
                "workspace control directory changed while its mutation lock was held: {}",
                control_path.display()
            );
        }
        Ok(&self.root)
    }

    pub(crate) fn control_identity(&self) -> Result<(u64, u64)> {
        self.root()?;
        Ok(self.control_identity)
    }

    pub(crate) fn validate_identity(
        &self,
        expected_root: &Path,
        expected_control_identity: (u64, u64),
    ) -> Result<()> {
        let root = self.root()?;
        if root != expected_root {
            bail!(
                "workspace mutation lock root {} does not match cache root {}",
                root.display(),
                expected_root.display()
            );
        }
        if self.control_identity != expected_control_identity {
            bail!(
                "workspace mutation lock control directory does not match the cache for {}",
                expected_root.display()
            );
        }
        Ok(())
    }
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting workspace control directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "workspace control path must be a real directory: {}",
            path.display()
        );
    }
    Ok((metadata.dev(), metadata.ino()))
}

impl Drop for WorkspaceMutationLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: `self.file` remains open until after Drop returns.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceMutationLock;

    #[test]
    fn lock_rejects_replaced_control_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let lock = WorkspaceMutationLock::acquire(root).unwrap();

        std::fs::rename(root.join(".mdc"), root.join("old-mdc")).unwrap();
        std::fs::create_dir(root.join(".mdc")).unwrap();

        assert!(lock.root().is_err());
    }
}
