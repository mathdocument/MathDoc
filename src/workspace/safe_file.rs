use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

use std::os::unix::fs::MetadataExt;

#[derive(Debug, thiserror::Error)]
#[error("{path} changed before it could be replaced")]
pub(crate) struct FileConflict {
    path: PathBuf,
}

impl FileConflict {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum FileSnapshot {
    Missing,
    File {
        content: Vec<u8>,
        metadata: PreservedMetadata,
        identity: FileIdentity,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct PreservedMetadata {
    permissions: std::fs::Permissions,
    uid: u32,
    gid: u32,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    extended_attributes: Vec<(Vec<u8>, Vec<u8>)>,
}

impl PreservedMetadata {
    fn capture(path: &Path, metadata: &std::fs::Metadata) -> Result<Self> {
        if metadata.nlink() > 1 {
            bail!(
                "refusing to replace hard-linked file {} ({} links)",
                path.display(),
                metadata.nlink()
            );
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;

            if metadata.st_flags() != 0 {
                bail!(
                    "refusing to replace file with unsupported flags {}",
                    path.display()
                );
            }
            reject_extended_acl(path)?;
        }

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let extended_attributes = read_extended_attributes(path)?;

        #[cfg(target_os = "linux")]
        if extended_attributes
            .iter()
            .any(|(name, _)| name.starts_with(b"system.posix_acl_"))
        {
            bail!("refusing to replace file with ACLs {}", path.display());
        }

        Ok(Self {
            permissions: metadata.permissions(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            extended_attributes,
        })
    }

    fn matches(&self, other: &Self) -> bool {
        same_permissions(&self.permissions, &other.permissions)
            && self.uid == other.uid
            && self.gid == other.gid
            && {
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    self.extended_attributes == other.extended_attributes
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    true
                }
            }
    }

    fn apply(&self, file: &mut std::fs::File, path: &Path) -> Result<()> {
        {
            use std::os::fd::AsRawFd;

            let current = file.metadata()?;
            if current.uid() != self.uid || current.gid() != self.gid {
                let result = unsafe { libc::fchown(file.as_raw_fd(), self.uid, self.gid) };
                if result != 0 {
                    return Err(std::io::Error::last_os_error())
                        .with_context(|| format!("preserving ownership for {}", path.display()));
                }
            }
        }

        file.set_permissions(self.permissions.clone())
            .with_context(|| format!("preserving permissions for {}", path.display()))?;

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        replace_extended_attributes(path, &self.extended_attributes)?;

        Ok(())
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

fn file_identity(meta: &std::fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: meta.dev(),
        inode: meta.ino(),
    }
}

fn same_permissions(left: &std::fs::Permissions, right: &std::fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;
    left.mode() == right.mode()
}

impl FileSnapshot {
    pub(crate) fn capture(path: &Path) -> Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("refusing to access symlink {}", path.display())
            }
            Ok(meta) if !meta.is_file() => {
                bail!("refusing to access non-regular file {}", path.display())
            }
            Ok(meta) => Ok(Self::File {
                content: std::fs::read(path)
                    .with_context(|| format!("reading {}", path.display()))?,
                metadata: PreservedMetadata::capture(path, &meta)?,
                identity: file_identity(&meta),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Missing),
            Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
        }
    }

    pub(crate) fn content(&self) -> Option<&[u8]> {
        match self {
            Self::Missing => None,
            Self::File { content, .. } => Some(content),
        }
    }

    pub(crate) fn identity(&self) -> Option<&FileIdentity> {
        match self {
            Self::Missing => None,
            Self::File { identity, .. } => Some(identity),
        }
    }

    pub(crate) fn unchanged(&self, path: &Path) -> Result<bool> {
        let current = match Self::capture(path) {
            Ok(current) => current,
            Err(_) => return Ok(false),
        };
        Ok(match (self, current) {
            (Self::Missing, Self::Missing) => true,
            (
                Self::File {
                    content,
                    metadata,
                    identity,
                },
                Self::File {
                    content: current,
                    metadata: current_metadata,
                    identity: current_identity,
                },
            ) => {
                *content == current
                    && *identity == current_identity
                    && metadata.matches(&current_metadata)
            }
            _ => false,
        })
    }

    pub(crate) fn replace(&self, path: &Path, content: &[u8]) -> Result<AppliedWrite> {
        atomic_replace(path, self, content)
    }

    pub(crate) fn remove(&self, path: &Path) -> Result<Option<AppliedWrite>> {
        atomic_remove(path, self)
    }

    pub(crate) fn case_rename(&self, from: &Path, to: &Path) -> Result<AppliedRename> {
        atomic_case_rename(from, to, self)
    }
}

pub(crate) struct AppliedWrite {
    path: PathBuf,
    before: FileSnapshot,
    after: FileSnapshot,
}

pub(crate) struct AppliedRename {
    from: PathBuf,
    to: PathBuf,
    after: FileSnapshot,
}

impl AppliedRename {
    pub(crate) fn rollback(self) -> Result<()> {
        if !self.after.unchanged(&self.to)? {
            bail!(
                "refusing to roll back rename of {} because it changed after the operation",
                self.to.display()
            );
        }
        rename_through_temporary(&self.to, &self.from)
    }
}

impl AppliedWrite {
    pub(crate) fn rollback(self) -> Result<()> {
        if !self.after.unchanged(&self.path)? {
            bail!(
                "refusing to roll back {} because it changed after this operation wrote it",
                self.path.display()
            );
        }
        let current = FileSnapshot::capture(&self.path)?;
        match self.before {
            FileSnapshot::Missing => {
                std::fs::remove_file(&self.path)
                    .with_context(|| format!("removing {} during rollback", self.path.display()))?;
                sync_parent(&self.path);
            }
            before @ FileSnapshot::File { .. } => {
                let content = before
                    .content()
                    .expect("file snapshot has content")
                    .to_vec();
                let metadata = match &before {
                    FileSnapshot::File { metadata, .. } => metadata,
                    FileSnapshot::Missing => unreachable!(),
                };
                atomic_replace_inner(&self.path, &current, &content, Some(metadata))?;
            }
        }
        Ok(())
    }

    pub(crate) fn into_after(self) -> FileSnapshot {
        self.after
    }
}

fn atomic_replace(path: &Path, expected: &FileSnapshot, content: &[u8]) -> Result<AppliedWrite> {
    let metadata = match expected {
        FileSnapshot::Missing => None,
        FileSnapshot::File { metadata, .. } => Some(metadata),
    };
    atomic_replace_inner(path, expected, content, metadata)
}

fn atomic_replace_inner(
    path: &Path,
    expected: &FileSnapshot,
    content: &[u8],
    replacement_metadata: Option<&PreservedMetadata>,
) -> Result<AppliedWrite> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_regular_directory(parent)?;
    if !expected.unchanged(path)? {
        return Err(FileConflict::new(path).into());
    }
    if matches!(
        expected,
        FileSnapshot::File { metadata, .. } if metadata.permissions.readonly()
    ) {
        bail!("refusing to replace read-only file {}", path.display());
    }

    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    let mut temp = builder
        .tempfile_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    temp.write_all(content)
        .with_context(|| format!("writing temporary file for {}", path.display()))?;

    #[cfg(target_os = "macos")]
    reject_extended_acl(temp.path())?;
    if let Some(metadata) = replacement_metadata {
        let temp_path = temp.path().to_path_buf();
        metadata.apply(temp.as_file_mut(), &temp_path)?;
    }
    temp.as_file_mut()
        .sync_all()
        .with_context(|| format!("syncing temporary file for {}", path.display()))?;

    if !expected.unchanged(path)? {
        return Err(FileConflict::new(path).into());
    }
    match expected {
        FileSnapshot::Missing => {
            temp.persist_noclobber(path).map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::Error::from(FileConflict::new(path))
                } else {
                    anyhow::Error::from(error.error)
                }
            })?;
        }
        FileSnapshot::File { .. } => persist_replacement(temp, path)?,
    };
    sync_parent(path);

    let after = FileSnapshot::capture(path)?;
    Ok(AppliedWrite {
        path: path.to_path_buf(),
        before: expected.clone(),
        after,
    })
}

fn atomic_remove(path: &Path, expected: &FileSnapshot) -> Result<Option<AppliedWrite>> {
    if matches!(expected, FileSnapshot::Missing) {
        return Ok(None);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_regular_directory(parent)?;
    if !expected.unchanged(path)? {
        return Err(FileConflict::new(path).into());
    }
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    sync_parent(path);
    Ok(Some(AppliedWrite {
        path: path.to_path_buf(),
        before: expected.clone(),
        after: FileSnapshot::Missing,
    }))
}

fn atomic_case_rename(from: &Path, to: &Path, expected: &FileSnapshot) -> Result<AppliedRename> {
    if from == to {
        bail!("source and destination of a case rename must differ");
    }
    let Some(expected_identity) = expected.identity() else {
        bail!("cannot rename missing file {}", from.display());
    };
    ensure_regular_directory(from.parent().unwrap_or_else(|| Path::new(".")))?;
    ensure_regular_directory(to.parent().unwrap_or_else(|| Path::new(".")))?;
    if !expected.unchanged(from)? {
        return Err(FileConflict::new(from).into());
    }
    let destination = FileSnapshot::capture(to)?;
    if destination.identity() != Some(expected_identity) {
        bail!(
            "case-rename destination does not identify the source file: {}",
            to.display()
        );
    }

    rename_through_temporary(from, to)?;
    let after = FileSnapshot::capture(to)?;
    Ok(AppliedRename {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        after,
    })
}

fn rename_through_temporary(from: &Path, to: &Path) -> Result<()> {
    let parent = from.parent().unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".mdc-case-rename-")
        .tempfile_in(parent)
        .with_context(|| format!("creating case-rename temporary in {}", parent.display()))?;
    let (_, temporary_path) = temporary
        .keep()
        .map_err(|error| error.error)
        .with_context(|| format!("retaining case-rename temporary in {}", parent.display()))?;
    if let Err(error) = std::fs::rename(from, &temporary_path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error).with_context(|| format!("renaming {}", from.display()));
    }
    if let Err(error) = std::fs::rename(&temporary_path, to) {
        let restore = std::fs::rename(&temporary_path, from);
        return match restore {
            Ok(()) => Err(error).with_context(|| format!("renaming to {}", to.display())),
            Err(restore_error) => bail!(
                "renaming to {} failed: {error}; restoring {} also failed: {restore_error}",
                to.display(),
                from.display()
            ),
        };
    }
    sync_parent(from);
    if from.parent() != to.parent() {
        sync_parent(to);
    }
    Ok(())
}

pub(crate) fn atomic_create_if_missing(path: &Path, content: &[u8]) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("refusing to access symlink {}", path.display())
        }
        Ok(meta) if !meta.is_file() => {
            bail!("refusing to access non-regular file {}", path.display())
        }
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    atomic_replace(path, &FileSnapshot::Missing, content)?;
    Ok(true)
}

pub(crate) fn ensure_regular_directory_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            ensure_regular_directory(path)?;
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(path) {
                Ok(()) => return Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            ensure_regular_directory(path)?;
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn ensure_regular_directory(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting directory {}", path.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        bail!("refusing to use non-directory path {}", path.display());
    }
    Ok(())
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        let _ = std::fs::File::open(parent).and_then(|dir| dir.sync_all());
    }
}

fn persist_replacement(temp: tempfile::NamedTempFile, path: &Path) -> Result<()> {
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn path_c_string(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path contains a null byte: {}", path.display()))
}

#[cfg(target_os = "macos")]
fn reject_extended_acl(path: &Path) -> Result<()> {
    use std::ffi::{c_char, c_int, c_void};

    unsafe extern "C" {
        fn acl_get_file(path: *const c_char, acl_type: c_int) -> *mut c_void;
        fn acl_free(value: *mut c_void) -> c_int;
    }

    const ACL_TYPE_EXTENDED: c_int = 0x100;
    let path_string = path_c_string(path)?;
    let acl = unsafe { acl_get_file(path_string.as_ptr(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(());
        }
        return Err(error).with_context(|| format!("inspecting ACLs on {}", path.display()));
    }
    unsafe {
        acl_free(acl);
    }
    bail!("refusing to replace file with ACLs {}", path.display())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_extended_attributes(path: &Path) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let path_string = path_c_string(path)?;
    let size = unsafe { list_xattr(path_string.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("listing extended attributes on {}", path.display()));
    }

    let mut list = vec![0_u8; size as usize];
    if size > 0 {
        let read =
            unsafe { list_xattr(path_string.as_ptr(), list.as_mut_ptr().cast(), list.len()) };
        if read < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("listing extended attributes on {}", path.display()));
        }
        list.truncate(read as usize);
    }

    let mut attributes = Vec::new();
    for name in list
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name_string = std::ffi::CString::new(name).expect("xattr name has no null bytes");
        let size = unsafe {
            get_xattr(
                path_string.as_ptr(),
                name_string.as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        };
        if size < 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "reading extended attribute {} on {}",
                    String::from_utf8_lossy(name),
                    path.display()
                )
            });
        }
        let mut value = vec![0_u8; size as usize];
        if size > 0 {
            let read = unsafe {
                get_xattr(
                    path_string.as_ptr(),
                    name_string.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                )
            };
            if read < 0 {
                return Err(std::io::Error::last_os_error()).with_context(|| {
                    format!(
                        "reading extended attribute {} on {}",
                        String::from_utf8_lossy(name),
                        path.display()
                    )
                });
            }
            value.truncate(read as usize);
        }
        attributes.push((name.to_vec(), value));
    }
    attributes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(attributes)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn replace_extended_attributes(path: &Path, attributes: &[(Vec<u8>, Vec<u8>)]) -> Result<()> {
    let path_string = path_c_string(path)?;
    for (name, _) in read_extended_attributes(path)? {
        let name_string = std::ffi::CString::new(name).expect("xattr name has no null bytes");
        let result = unsafe { remove_xattr(path_string.as_ptr(), name_string.as_ptr()) };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("clearing extended attributes on {}", path.display()));
        }
    }
    for (name, value) in attributes {
        let name_string =
            std::ffi::CString::new(name.as_slice()).expect("xattr name has no null bytes");
        let result = unsafe {
            set_xattr(
                path_string.as_ptr(),
                name_string.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "preserving extended attribute {} on {}",
                    String::from_utf8_lossy(name),
                    path.display()
                )
            });
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
unsafe fn list_xattr(path: *const libc::c_char, list: *mut libc::c_char, size: usize) -> isize {
    libc::listxattr(path, list, size, 0)
}

#[cfg(target_os = "linux")]
unsafe fn list_xattr(path: *const libc::c_char, list: *mut libc::c_char, size: usize) -> isize {
    libc::listxattr(path, list, size)
}

#[cfg(target_os = "macos")]
unsafe fn get_xattr(
    path: *const libc::c_char,
    name: *const libc::c_char,
    value: *mut libc::c_void,
    size: usize,
) -> isize {
    libc::getxattr(path, name, value, size, 0, 0)
}

#[cfg(target_os = "linux")]
unsafe fn get_xattr(
    path: *const libc::c_char,
    name: *const libc::c_char,
    value: *mut libc::c_void,
    size: usize,
) -> isize {
    libc::getxattr(path, name, value, size)
}

#[cfg(target_os = "macos")]
unsafe fn set_xattr(
    path: *const libc::c_char,
    name: *const libc::c_char,
    value: *const libc::c_void,
    size: usize,
) -> libc::c_int {
    libc::setxattr(path, name, value, size, 0, 0)
}

#[cfg(target_os = "linux")]
unsafe fn set_xattr(
    path: *const libc::c_char,
    name: *const libc::c_char,
    value: *const libc::c_void,
    size: usize,
) -> libc::c_int {
    libc::setxattr(path, name, value, size, 0)
}

#[cfg(target_os = "macos")]
unsafe fn remove_xattr(path: *const libc::c_char, name: *const libc::c_char) -> libc::c_int {
    libc::removexattr(path, name, 0)
}

#[cfg(target_os = "linux")]
unsafe fn remove_xattr(path: *const libc::c_char, name: *const libc::c_char) -> libc::c_int {
    libc::removexattr(path, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_linked_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        std::fs::hard_link(&path, dir.path().join("alias.mdoc")).unwrap();

        let error = match FileSnapshot::capture(&path) {
            Ok(_) => panic!("hard-linked file should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("hard-linked file"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "before");
    }

    #[test]
    fn removed_file_rollback_restores_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("generated.lean");
        std::fs::write(&path, "before").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let snapshot = FileSnapshot::capture(&path).unwrap();

        atomic_remove(&path, &snapshot)
            .unwrap()
            .unwrap()
            .rollback()
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extended_attributes_survive_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        let mut attributes = read_extended_attributes(&path).unwrap();
        attributes.push((b"com.mathdoc.test".to_vec(), b"preserved".to_vec()));
        attributes.sort_by(|left, right| left.0.cmp(&right.0));
        replace_extended_attributes(&path, &attributes).unwrap();

        let snapshot = FileSnapshot::capture(&path).unwrap();
        atomic_replace(&path, &snapshot, b"after").unwrap();

        let attributes = read_extended_attributes(&path).unwrap();
        assert!(attributes
            .iter()
            .any(|(name, value)| name == b"com.mathdoc.test" && value == b"preserved"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn files_with_acls_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        let status = std::process::Command::new("chmod")
            .args(["+a", "everyone allow read"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());

        let error = match FileSnapshot::capture(&path) {
            Ok(_) => panic!("file with an ACL should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("with ACLs"));

        let _ = std::process::Command::new("chmod")
            .arg("-N")
            .arg(&path)
            .status();
    }
}
