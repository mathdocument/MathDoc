use anyhow::{bail, Context, Result};
use std::fs::File;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LockFileGeneration {
    device: u64,
    inode: u64,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

/// Workspace-wide interprocess lock for graph and node mutations.
pub(crate) struct WorkspaceMutationLock {
    file: File,
    root: PathBuf,
    control_identity: (u64, u64),
    lock_path: PathBuf,
    lock_generation: LockFileGeneration,
}

/// Interprocess lock for source mirror reconciliation and compiler workspaces.
pub(crate) struct WorkspaceWorkLock {
    file: File,
    root: PathBuf,
    control_identity: (u64, u64),
    lock_path: PathBuf,
    lock_generation: LockFileGeneration,
}

impl WorkspaceMutationLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        Self::acquire_inner(root, None)
    }

    pub(crate) fn acquire_with_timeout(root: &Path, timeout: std::time::Duration) -> Result<Self> {
        Self::acquire_inner(root, Some(std::time::Instant::now() + timeout))
    }

    fn acquire_inner(root: &Path, deadline: Option<std::time::Instant>) -> Result<Self> {
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
        let lock_generation = opened_lock_generation(&file, &path, "workspace mutation lock")?;
        loop {
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                bail!(
                    "timed out waiting for workspace mutation lock {}",
                    path.display()
                );
            }
            // SAFETY: `file` owns a live descriptor for the duration of the lock.
            let operation = libc::LOCK_EX | if deadline.is_some() { libc::LOCK_NB } else { 0 };
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error
                .raw_os_error()
                .is_some_and(|code| code == libc::EAGAIN || code == libc::EWOULDBLOCK)
            {
                if deadline.is_none() {
                    return Err(error).with_context(|| {
                        format!("locking workspace mutation lock {}", path.display())
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            } else {
                return Err(error).with_context(|| {
                    format!("locking workspace mutation lock {}", path.display())
                });
            }
        }
        super::safe_file::run_test_hook(super::safe_file::TestHookPoint::MutationLockAfterFlock);
        require_current_lock_file(
            &file,
            &path,
            lock_generation,
            "workspace mutation lock",
            "while acquiring it",
        )?;
        if directory_identity(&control_path)? != control_identity {
            return Err(super::WorkspaceGenerationError::new(format!(
                "workspace control directory changed while acquiring its mutation lock: {}",
                control_path.display()
            ))
            .into());
        }
        Ok(Self {
            file,
            root,
            control_identity,
            lock_path: path,
            lock_generation,
        })
    }

    pub(crate) fn root(&self) -> Result<&Path> {
        require_current_lock_file(
            &self.file,
            &self.lock_path,
            self.lock_generation,
            "workspace mutation lock",
            "while it was held",
        )?;
        let control_path = self.root.join(".mdc");
        if directory_identity(&control_path)? != self.control_identity {
            return Err(super::WorkspaceGenerationError::new(format!(
                "workspace control directory changed while its mutation lock was held: {}",
                control_path.display()
            ))
            .into());
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

impl WorkspaceWorkLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        let root = super::validate_mdcroot(root)?;
        let control_path = root.join(".mdc");
        let control_identity = directory_identity(&control_path)?;
        let path = control_path.join("work.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
            .with_context(|| format!("opening workspace work lock {}", path.display()))?;
        let lock_generation = opened_lock_generation(&file, &path, "workspace work lock")?;
        loop {
            // SAFETY: `file` owns a live descriptor for the duration of the lock.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error)
                    .with_context(|| format!("locking workspace work lock {}", path.display()));
            }
        }
        super::safe_file::run_test_hook(super::safe_file::TestHookPoint::WorkLockAfterFlock);
        require_current_lock_file(
            &file,
            &path,
            lock_generation,
            "workspace work lock",
            "while acquiring it",
        )?;
        if directory_identity(&control_path)? != control_identity {
            return Err(super::WorkspaceGenerationError::new(format!(
                "workspace control directory changed while acquiring its work lock: {}",
                control_path.display()
            ))
            .into());
        }
        Ok(Self {
            file,
            root,
            control_identity,
            lock_path: path,
            lock_generation,
        })
    }

    pub(crate) fn acquire_mutation_lock(&self) -> Result<WorkspaceMutationLock> {
        let mutation_lock = WorkspaceMutationLock::acquire(&self.root)?;
        self.require_current()?;
        Ok(mutation_lock)
    }

    pub(crate) fn root(&self) -> Result<&Path> {
        self.require_current()?;
        Ok(&self.root)
    }

    pub(crate) fn validate_root(&self, expected_root: &Path) -> Result<()> {
        let root = self.root()?;
        let expected_root = super::validate_mdcroot(expected_root)?;
        if root != expected_root {
            bail!(
                "workspace work lock root {} does not match requested root {}",
                root.display(),
                expected_root.display()
            );
        }
        Ok(())
    }

    pub(crate) fn require_current(&self) -> Result<()> {
        require_current_lock_file(
            &self.file,
            &self.lock_path,
            self.lock_generation,
            "workspace work lock",
            "while it was held",
        )?;
        let control_path = self.root.join(".mdc");
        if directory_identity(&control_path)? != self.control_identity {
            return Err(super::WorkspaceGenerationError::new(format!(
                "workspace control directory changed while its work lock was held: {}",
                control_path.display()
            ))
            .into());
        }
        Ok(())
    }

    pub(crate) fn validate_identity(
        &self,
        expected_root: &Path,
        expected_control_identity: (u64, u64),
    ) -> Result<()> {
        self.require_current()?;
        if self.root != expected_root {
            bail!(
                "workspace work lock root {} does not match cache root {}",
                self.root.display(),
                expected_root.display()
            );
        }
        if self.control_identity != expected_control_identity {
            bail!(
                "workspace work lock control directory does not match the cache for {}",
                expected_root.display()
            );
        }
        Ok(())
    }
}

fn opened_lock_generation(file: &File, path: &Path, name: &str) -> Result<LockFileGeneration> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspecting {name} {}", path.display()))?;
    lock_file_generation(&metadata, path, name)
}

fn require_current_lock_file(
    file: &File,
    path: &Path,
    expected: LockFileGeneration,
    name: &str,
    action: &str,
) -> Result<()> {
    let opened = opened_lock_generation(file, path, name)?;
    if opened != expected {
        return Err(super::WorkspaceGenerationError::new(format!(
            "{name} file generation changed {action}: {}",
            path.display()
        ))
        .into());
    }

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {name} pathname {} {action}", path.display()))?;
    let current = lock_file_generation(&metadata, path, name)?;
    if current != expected {
        return Err(super::WorkspaceGenerationError::new(format!(
            "{name} pathname changed {action}: {}",
            path.display()
        ))
        .into());
    }
    Ok(())
}

fn lock_file_generation(
    metadata: &std::fs::Metadata,
    path: &Path,
    name: &str,
) -> Result<LockFileGeneration> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.is_file() {
        return Err(super::WorkspaceGenerationError::new(format!(
            "{name} is not a regular file: {}",
            path.display()
        ))
        .into());
    }
    if metadata.nlink() != 1 {
        return Err(super::WorkspaceGenerationError::new(format!(
            "{name} must have exactly one link: {} ({} links)",
            path.display(),
            metadata.nlink()
        ))
        .into());
    }
    Ok(LockFileGeneration {
        device: metadata.dev(),
        inode: metadata.ino(),
        len: metadata.len(),
        mtime: metadata.mtime(),
        mtime_nsec: metadata.mtime_nsec(),
        ctime: metadata.ctime(),
        ctime_nsec: metadata.ctime_nsec(),
    })
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting workspace control directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(super::WorkspaceGenerationError::new(format!(
            "workspace control path must be a real directory: {}",
            path.display()
        ))
        .into());
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

impl Drop for WorkspaceWorkLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: `self.file` remains open until after Drop returns.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceMutationLock, WorkspaceWorkLock};
    use crate::workspace::{set_test_hook, TestHookPoint};

    fn replace_lock_path(path: &std::path::Path, displaced: &std::path::Path) {
        std::fs::rename(path, displaced).unwrap();
        std::fs::write(path, []).unwrap();
    }

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

    #[test]
    fn work_lock_does_not_exclude_node_mutations() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let work_lock = WorkspaceWorkLock::acquire(root).unwrap();

        let mutation_lock = WorkspaceMutationLock::acquire(root).unwrap();

        work_lock.require_current().unwrap();
        mutation_lock.root().unwrap();
    }

    #[test]
    fn work_lock_rejects_replaced_control_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let lock = WorkspaceWorkLock::acquire(root).unwrap();

        std::fs::rename(root.join(".mdc"), root.join("old-mdc")).unwrap();
        std::fs::create_dir(root.join(".mdc")).unwrap();

        let error = lock.require_current().unwrap_err();
        assert!(crate::workspace::error_has_infrastructure_failure(&error));
    }

    #[test]
    fn work_lock_rejects_another_workspace() {
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(first.path().join(".mdc")).unwrap();
        std::fs::create_dir(second.path().join(".mdc")).unwrap();
        let lock = WorkspaceWorkLock::acquire(first.path()).unwrap();

        let error = lock.validate_root(second.path()).unwrap_err();

        assert!(error.to_string().contains("does not match requested root"));
    }

    #[test]
    fn mutation_lock_rejects_replaced_path_while_held() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let lock = WorkspaceMutationLock::acquire(root).unwrap();
        let path = root.join(".mdc/mutation.lock");

        replace_lock_path(&path, &root.join("old-mutation.lock"));

        assert!(lock.root().is_err());
    }

    #[test]
    fn work_lock_rejects_replaced_path_while_held() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let lock = WorkspaceWorkLock::acquire(root).unwrap();
        let path = root.join(".mdc/work.lock");

        replace_lock_path(&path, &root.join("old-work.lock"));

        assert!(lock.require_current().is_err());
    }

    #[test]
    fn mutation_lock_rejects_unlinked_path_while_held() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let lock = WorkspaceMutationLock::acquire(root).unwrap();

        std::fs::remove_file(root.join(".mdc/mutation.lock")).unwrap();

        assert!(lock.root().is_err());
    }

    #[test]
    fn work_lock_rejects_a_hard_link_added_while_held() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let lock = WorkspaceWorkLock::acquire(root).unwrap();

        std::fs::hard_link(root.join(".mdc/work.lock"), root.join("work-lock-alias")).unwrap();

        let error = lock.require_current().unwrap_err();
        assert!(crate::workspace::error_has_infrastructure_failure(&error));
    }

    #[test]
    fn mutation_lock_rejects_an_existing_hard_link() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let path = root.join(".mdc/mutation.lock");
        std::fs::write(&path, []).unwrap();
        std::fs::hard_link(&path, root.join("mutation-lock-alias")).unwrap();

        let error = WorkspaceMutationLock::acquire(root)
            .err()
            .expect("hard-linked lock file must be rejected");

        assert!(format!("{error:#}").contains("exactly one link"));
        assert!(crate::workspace::error_has_infrastructure_failure(&error));
    }

    #[test]
    fn mutation_lock_timeout_bounds_contention_wait() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let _held = WorkspaceMutationLock::acquire(root).unwrap();

        let error =
            WorkspaceMutationLock::acquire_with_timeout(root, std::time::Duration::from_millis(30))
                .err()
                .expect("contended lock must time out");

        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn mutation_lock_rejects_path_replacement_during_acquisition() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let path = root.join(".mdc/mutation.lock");
        let displaced = root.join("old-mutation.lock");
        let hook_path = path.clone();
        set_test_hook(TestHookPoint::MutationLockAfterFlock, move || {
            replace_lock_path(&hook_path, &displaced);
        });

        let error = WorkspaceMutationLock::acquire(root)
            .err()
            .expect("replacement must invalidate acquisition");

        assert!(format!("{error:#}").contains("workspace mutation lock"));
    }

    #[test]
    fn work_lock_rejects_path_replacement_during_acquisition() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let path = root.join(".mdc/work.lock");
        let displaced = root.join("old-work.lock");
        let hook_path = path.clone();
        set_test_hook(TestHookPoint::WorkLockAfterFlock, move || {
            replace_lock_path(&hook_path, &displaced);
        });

        let error = WorkspaceWorkLock::acquire(root)
            .err()
            .expect("replacement must invalidate acquisition");

        assert!(format!("{error:#}").contains("workspace work lock"));
    }

    #[test]
    fn work_to_mutation_handoff_revalidates_the_work_lock() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let work_lock = WorkspaceWorkLock::acquire(root).unwrap();
        let path = root.join(".mdc/work.lock");
        let displaced = root.join("old-work.lock");
        let hook_path = path.clone();
        set_test_hook(TestHookPoint::MutationLockAfterFlock, move || {
            replace_lock_path(&hook_path, &displaced);
        });

        let error = work_lock
            .acquire_mutation_lock()
            .err()
            .expect("stale work lock must invalidate the handoff");

        assert!(format!("{error:#}").contains("workspace work lock"));
    }
}
