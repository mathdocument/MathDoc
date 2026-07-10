use anyhow::{bail, Context, Result};
#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
#[cfg(not(unix))]
use std::path::PathBuf;

/// Workspace-wide interprocess lock for graph and node mutations.
pub(crate) struct WorkspaceMutationLock {
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl WorkspaceMutationLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        let root = crate::workspace::validate_mdcroot(root)?;
        let path = root.join(".mdc").join("mutation.lock");
        acquire_lock(&path)
    }
}

#[cfg(unix)]
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

#[cfg(not(unix))]
fn acquire_lock(path: &Path) -> Result<WorkspaceMutationLock> {
    loop {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(_) => {
                return Ok(WorkspaceMutationLock {
                    path: path.to_path_buf(),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("creating workspace mutation lock {}", path.display())
                })
            }
        }
    }
}

#[cfg(unix)]
impl Drop for WorkspaceMutationLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: `self.file` remains open until after Drop returns.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(not(unix))]
impl Drop for WorkspaceMutationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
