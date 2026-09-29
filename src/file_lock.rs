//! File locks whose containing cache directory may be retired after deletion.
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

pub(crate) fn is_current(file: &File, path: &Path) -> io::Result<bool> {
    let current = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let opened = file.metadata()?;
    Ok((opened.dev(), opened.ino()) == (current.dev(), current.ino()))
}

fn try_lock(file: &File, path: &Path, exclusive: bool) -> io::Result<()> {
    let mode = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // A deleter may have retired the directory after this file was opened.
    // Owning that old inode must never authorize access to a recreated cache.
    if !is_current(file, path)? {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "cache changed while acquiring its lock; retry the command",
        ));
    }
    Ok(())
}

pub(crate) fn acquire(path: &Path, exclusive: bool) -> io::Result<File> {
    fs::create_dir_all(path.parent().expect("lock has a parent"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    try_lock(&file, path, exclusive)?;
    Ok(file)
}

/// Hold all affected leases through retirement. Never recursively delete the
/// original path: a same-name recreation may already own it after the rename.
pub(crate) fn remove_cache(root: &Path) -> io::Result<()> {
    let retired = tempfile::Builder::new()
        .prefix(".deleted-")
        .tempdir_in(root.parent().expect("cache has a parent"))?;
    fs::rename(root, retired.path().join("cache"))?;
    retired.close()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_locks_cannot_authorize_a_recreated_cache() {
        for exclusive in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("branch");
            let path = root.join("lease.lock");
            let owner = acquire(&path, true).unwrap();
            let stale = File::open(&path).unwrap();
            remove_cache(&root).unwrap();
            assert!(!root.exists());
            let new_owner = acquire(&path, true).unwrap();
            drop(owner);
            assert_eq!(
                try_lock(&stale, &path, exclusive).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
            assert_eq!(
                acquire(&path, exclusive).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            drop(new_owner);
            assert!(acquire(&path, exclusive).is_ok());
        }
    }
}
