use anyhow::{bail, Context, Result};
#[cfg(any(unix, windows))]
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
#[cfg(not(any(unix, windows)))]
use std::path::PathBuf;

/// Workspace-wide interprocess lock for graph and node mutations.
pub(crate) struct WorkspaceMutationLock {
    #[cfg(unix)]
    file: File,
    #[cfg(windows)]
    _file: File,
    #[cfg(not(any(unix, windows)))]
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

#[cfg(windows)]
fn acquire_lock(path: &Path) -> Result<WorkspaceMutationLock> {
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
        {
            Ok(file) => {
                let metadata = file.metadata()?;
                if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                    || !metadata.is_file()
                {
                    bail!(
                        "workspace mutation lock is not a regular file: {}",
                        path.display()
                    );
                }
                return Ok(WorkspaceMutationLock { _file: file });
            }
            Err(error) if matches!(error.raw_os_error(), Some(32) | Some(33)) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("opening workspace mutation lock {}", path.display()))
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
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

#[cfg(not(any(unix, windows)))]
impl Drop for WorkspaceMutationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    const CHILD_ROOT: &str = "MDC_MUTATION_LOCK_TEST_ROOT";

    #[test]
    fn lock_holder_process() {
        let Some(root) = std::env::var_os(CHILD_ROOT) else {
            return;
        };
        let _lock = WorkspaceMutationLock::acquire(Path::new(&root)).unwrap();
        std::process::exit(0);
    }

    #[test]
    fn lock_is_released_when_process_exits_without_drop() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".mdc")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mutation_lock::windows_tests::lock_holder_process",
                "--nocapture",
            ])
            .env(CHILD_ROOT, dir.path())
            .status()
            .unwrap();
        assert!(status.success());

        WorkspaceMutationLock::acquire(dir.path()).unwrap();
    }
}
