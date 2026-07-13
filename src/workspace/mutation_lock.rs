use anyhow::{bail, Context, Result};
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;

/// Workspace-wide interprocess lock for graph and node mutations.
pub(crate) struct WorkspaceMutationLock {
    file: File,
}

impl WorkspaceMutationLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        let root = super::validate_mdcroot(root)?;
        let path = root.join(".mdc").join("mutation.lock");
        acquire_lock(&path)
    }
}

fn acquire_lock(path: &Path) -> Result<WorkspaceMutationLock> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
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
            return Err(error)
                .with_context(|| format!("locking workspace mutation lock {}", path.display()));
        }
    }
    Ok(WorkspaceMutationLock { file })
}

impl Drop for WorkspaceMutationLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: `self.file` remains open until after Drop returns.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}
