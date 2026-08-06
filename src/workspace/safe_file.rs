use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use std::os::unix::fs::MetadataExt;

#[derive(Debug, thiserror::Error)]
#[error("{path} changed before it could be replaced")]
pub(crate) struct FileConflict {
    path: PathBuf,
}

impl FileConflict {
    pub(crate) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PersistenceRecoveryError {
    message: String,
    primary: anyhow::Error,
    rollback: Option<anyhow::Error>,
    index_repair: Option<anyhow::Error>,
}

impl PersistenceRecoveryError {
    pub(crate) fn new(
        message: String,
        primary: anyhow::Error,
        rollback: Option<anyhow::Error>,
        index_repair: Option<anyhow::Error>,
    ) -> Self {
        Self {
            message,
            primary,
            rollback,
            index_repair,
        }
    }

    pub(crate) fn from_attempts(
        primary: anyhow::Error,
        rollback: Result<()>,
        index_repair: Result<()>,
        rollback_action: &str,
        index_repair_action: &str,
    ) -> anyhow::Error {
        let rollback = rollback.err();
        let index_repair = index_repair.err();
        if rollback.is_none() && index_repair.is_none() {
            return primary;
        }

        let mut message = primary.to_string();
        if let Some(error) = &rollback {
            message.push_str(&format!(
                "; additionally failed to {rollback_action}: {error}"
            ));
        }
        if let Some(error) = &index_repair {
            message.push_str(&format!(
                "; additionally failed to {index_repair_action}: {error}"
            ));
        }
        Self::new(message, primary, rollback, index_repair).into()
    }

    fn errors(&self) -> impl Iterator<Item = &anyhow::Error> {
        std::iter::once(&self.primary)
            .chain(self.rollback.iter())
            .chain(self.index_repair.iter())
    }

    pub(crate) fn has_file_conflict(&self) -> bool {
        self.errors().any(error_has_file_conflict)
    }

    pub(crate) fn has_infrastructure_failure(&self) -> bool {
        self.index_repair.is_some()
            || self.errors().any(|error| {
                error_chain_has::<std::io::Error>(error)
                    || error_chain_has::<rusqlite::Error>(error)
                    || error_chain_has::<super::WorkspaceGenerationError>(error)
            })
    }
}

impl std::fmt::Display for PersistenceRecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PersistenceRecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

fn error_chain_has<T: std::error::Error + 'static>(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<T>().is_some())
}

pub(crate) fn error_has_file_conflict(error: &anyhow::Error) -> bool {
    error_chain_has::<FileConflict>(error)
        || error.chain().any(|cause| {
            cause
                .downcast_ref::<PersistenceRecoveryError>()
                .is_some_and(PersistenceRecoveryError::has_file_conflict)
        })
}

pub(crate) fn error_has_infrastructure_failure(error: &anyhow::Error) -> bool {
    error_chain_has::<std::io::Error>(error)
        || error_chain_has::<rusqlite::Error>(error)
        || error_chain_has::<super::WorkspaceGenerationError>(error)
        || error.chain().any(|cause| {
            cause
                .downcast_ref::<PersistenceRecoveryError>()
                .is_some_and(PersistenceRecoveryError::has_infrastructure_failure)
        })
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

#[derive(Debug)]
pub(crate) struct ReadFileSnapshot {
    content: Vec<u8>,
    metadata: std::fs::Metadata,
    identity: FileIdentity,
}

impl ReadFileSnapshot {
    pub(crate) fn content(&self) -> &[u8] {
        &self.content
    }

    pub(crate) fn metadata(&self) -> &std::fs::Metadata {
        &self.metadata
    }

    pub(crate) fn identity(&self) -> &FileIdentity {
        &self.identity
    }

    pub(crate) fn matches(&self, other: Option<&Self>) -> bool {
        let Some(other) = other else {
            return false;
        };
        self.content == other.content
            && self.identity == other.identity
            && same_read_generation(&self.metadata, &other.metadata)
            && self.metadata.uid() == other.metadata.uid()
            && self.metadata.gid() == other.metadata.gid()
            && same_permissions(&self.metadata.permissions(), &other.metadata.permissions())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PreservedMetadata {
    permissions: std::fs::Permissions,
    uid: u32,
    gid: u32,
    generation: ReadGeneration,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    extended_attributes: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadGeneration {
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl ReadGeneration {
    fn capture(metadata: &std::fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        }
    }
}

impl PreservedMetadata {
    fn capture(path: &Path, file: &std::fs::File, metadata: &std::fs::Metadata) -> Result<Self> {
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
            reject_extended_acl(file, path)?;
        }

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let extended_attributes = read_extended_attributes(file, path)?;

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
            generation: ReadGeneration::capture(metadata),
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

    fn matches_file_metadata(&self, other: &std::fs::Metadata) -> bool {
        same_permissions(&self.permissions, &other.permissions())
            && self.uid == other.uid()
            && self.gid == other.gid()
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
        replace_extended_attributes(file, path, &self.extended_attributes)?;

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

#[derive(Debug)]
struct DirectoryBinding {
    parent: PathBuf,
    target: CString,
    identities: Vec<(PathBuf, FileIdentity)>,
    directory: std::fs::File,
}

#[derive(Debug)]
struct DirectoryBindingSnapshot {
    parent: PathBuf,
    target: CString,
    identities: Vec<(PathBuf, FileIdentity)>,
}

#[derive(Debug)]
pub(crate) struct DirectoryGeneration {
    path: PathBuf,
    binding: DirectoryBinding,
    directory: std::fs::File,
    identity: FileIdentity,
}

impl DirectoryGeneration {
    pub(crate) fn open_beneath(root: &Path, path: &Path) -> Result<Self> {
        let binding = DirectoryBinding::open_beneath(root, path)?;
        let directory =
            open_directory_entry(binding.raw_fd(), binding.target_name()).map_err(|error| {
                anyhow::Error::from(FileConflict::new(path)).context(format!(
                    "opening directory generation {}: {error}",
                    path.display()
                ))
            })?;
        let identity = file_identity(&directory.metadata()?);
        let generation = Self {
            path: path.to_path_buf(),
            binding,
            directory,
            identity,
        };
        generation.require_current()?;
        Ok(generation)
    }

    pub(crate) fn open_descendant(&self, root: &Path, path: &Path) -> Result<Self> {
        self.require_current()?;
        if !path.starts_with(&self.path) || path == self.path {
            bail!(
                "directory {} is not beneath generation {}",
                path.display(),
                self.path.display()
            );
        }
        let descendant = Self::open_beneath(root, path)?;
        self.require_current()?;
        if !descendant
            .binding
            .identities
            .iter()
            .any(|(path, identity)| path == &self.path && identity == &self.identity)
        {
            return Err(FileConflict::new(path).into());
        }
        Ok(descendant)
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.directory.as_raw_fd()
    }

    pub(crate) fn require_current(&self) -> Result<()> {
        self.binding.require_current(&self.path)?;
        if file_identity(&self.directory.metadata()?) != self.identity {
            return Err(FileConflict::new(&self.path).into());
        }
        let current = open_directory_entry(self.binding.raw_fd(), self.binding.target_name())
            .map_err(|error| {
                anyhow::Error::from(FileConflict::new(&self.path)).context(format!(
                    "revalidating directory generation {}: {error}",
                    self.path.display()
                ))
            })?;
        if file_identity(&current.metadata()?) != self.identity {
            return Err(FileConflict::new(&self.path).into());
        }
        Ok(())
    }
}

impl DirectoryBinding {
    fn open(path: &Path) -> Result<Self> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let parent = absolute
            .parent()
            .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?
            .canonicalize()
            .with_context(|| format!("canonicalizing parent of {}", path.display()))?;
        let file_name = absolute
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", path.display()))?;
        use std::os::unix::ffi::OsStrExt;
        let target = CString::new(file_name.as_bytes())
            .with_context(|| format!("path contains a null byte: {}", path.display()))?;
        let identities = capture_directory_identities(&parent)?;
        let directory = open_directory(&parent)?;
        let expected = identities
            .last()
            .map(|(_, identity)| identity)
            .expect("absolute parent has at least the root directory");
        if file_identity(&directory.metadata()?) != *expected {
            return Err(FileConflict::new(path).into());
        }
        let binding = Self {
            parent,
            target,
            identities,
            directory,
        };
        if !binding.is_current()? {
            return Err(FileConflict::new(path).into());
        }
        Ok(binding)
    }

    fn open_beneath(root: &Path, path: &Path) -> Result<Self> {
        let canonical_root = root
            .canonicalize()
            .with_context(|| format!("canonicalizing write root {}", root.display()))?;
        if canonical_root != root {
            return Err(FileConflict::new(path).into());
        }
        let root = canonical_root;
        let relative = path.strip_prefix(&root).map_err(|_| {
            anyhow::anyhow!(
                "write path {} is outside root {}",
                path.display(),
                root.display()
            )
        })?;
        let parent_relative = relative
            .parent()
            .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
        let target = relative
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", path.display()))?;
        let target = CString::new(target.as_bytes())
            .with_context(|| format!("path contains a null byte: {}", path.display()))?;

        let root_metadata = std::fs::symlink_metadata(&root)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            bail!("refusing to use non-directory root {}", root.display());
        }
        let mut directory = open_directory(&root)?;
        let root_identity = file_identity(&root_metadata);
        if file_identity(&directory.metadata()?) != root_identity {
            return Err(FileConflict::new(path).into());
        }
        let mut identities = vec![(root.clone(), root_identity)];
        let mut parent = root;

        for component in parent_relative.components() {
            let std::path::Component::Normal(name) = component else {
                bail!("refusing non-normalized write path {}", path.display());
            };
            let name = CString::new(name.as_bytes())
                .with_context(|| format!("path contains a null byte: {}", path.display()))?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                return if matches!(
                    error.raw_os_error(),
                    Some(libc::ELOOP) | Some(libc::ENOTDIR)
                ) {
                    Err(FileConflict::new(path).into())
                } else {
                    Err(error).with_context(|| format!("binding write path {}", path.display()))
                };
            }
            directory = unsafe { std::fs::File::from_raw_fd(fd) };
            let metadata = directory.metadata()?;
            if !metadata.is_dir() {
                return Err(FileConflict::new(path).into());
            }
            parent.push(std::ffi::OsStr::from_bytes(name.to_bytes()));
            identities.push((parent.clone(), file_identity(&metadata)));
        }

        let binding = Self {
            parent,
            target,
            identities,
            directory,
        };
        if !binding.is_current()? {
            return Err(FileConflict::new(path).into());
        }
        Ok(binding)
    }

    fn target_name(&self) -> &CStr {
        &self.target
    }

    fn raw_fd(&self) -> RawFd {
        self.directory.as_raw_fd()
    }

    fn path_for(&self, name: &CStr) -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        self.parent
            .join(std::ffi::OsStr::from_bytes(name.to_bytes()))
    }

    fn display_path(&self, name: &CStr) -> String {
        self.path_for(name).display().to_string()
    }

    fn is_current(&self) -> Result<bool> {
        if file_identity(&self.directory.metadata()?)
            != self
                .identities
                .last()
                .expect("directory identity chain is non-empty")
                .1
        {
            return Ok(false);
        }
        for (path, expected) in &self.identities {
            let metadata = match std::fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.into()),
            };
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || file_identity(&metadata) != *expected
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn require_current(&self, path: &Path) -> Result<()> {
        if self.is_current()? {
            Ok(())
        } else {
            Err(FileConflict::new(path).into())
        }
    }

    fn sync(&self) {
        let _ = self.directory.sync_all();
    }

    fn into_snapshot(self) -> DirectoryBindingSnapshot {
        DirectoryBindingSnapshot {
            parent: self.parent,
            target: self.target,
            identities: self.identities,
        }
    }
}

impl DirectoryBindingSnapshot {
    fn reopen(self, path: &Path) -> Result<DirectoryBinding> {
        let expected_path = self
            .parent
            .join(std::ffi::OsStr::from_bytes(self.target.to_bytes()));
        let path_name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", path.display()))?;
        let current_path = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?
            .canonicalize()
            .with_context(|| format!("canonicalizing parent of {}", path.display()))?
            .join(path_name);
        if expected_path != current_path {
            return Err(FileConflict::new(path).into());
        }
        for (directory_path, expected) in &self.identities {
            let metadata = std::fs::symlink_metadata(directory_path)
                .with_context(|| format!("inspecting directory {}", directory_path.display()))?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || file_identity(&metadata) != *expected
            {
                return Err(FileConflict::new(path).into());
            }
        }
        let directory = open_directory(&self.parent)?;
        let binding = DirectoryBinding {
            parent: self.parent,
            target: self.target,
            identities: self.identities,
            directory,
        };
        binding.require_current(path)?;
        Ok(binding)
    }
}

fn capture_directory_identities(path: &Path) -> Result<Vec<(PathBuf, FileIdentity)>> {
    let mut current = PathBuf::new();
    let mut identities = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir => current.push(Path::new("/")),
            std::path::Component::Normal(name) => current.push(name),
            std::path::Component::CurDir => continue,
            std::path::Component::ParentDir | std::path::Component::Prefix(_) => {
                bail!("refusing non-normalized directory path {}", path.display())
            }
        }
        let metadata = std::fs::symlink_metadata(&current)
            .with_context(|| format!("inspecting directory {}", current.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("refusing to use non-directory path {}", current.display());
        }
        identities.push((current.clone(), file_identity(&metadata)));
    }
    Ok(identities)
}

fn open_directory(path: &Path) -> Result<std::fs::File> {
    let path_string = path_c_string(path)?;
    let fd = unsafe {
        libc::open(
            path_string.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("opening directory {}", path.display()));
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestHookPoint {
    MutationLockAfterFlock,
    WorkLockAfterFlock,
    ProcessAfterCwdOpen,
    IndexBeforeConnectionOpen,
    IndexAfterConnectionOpen,
    IndexAfterNodeUpsert,
    ReadAfterContent,
    WriteBeforeDirectoryBinding,
    RemoveBeforeDirectoryBinding,
    RemoveTreeBeforeDirectoryBinding,
    RemoveTreeAfterDirectoryBinding,
    RemoveTreeBeforeTargetOpen,
    RemoveTreeAfterTargetOpen,
    RemoveTreeBeforeQuarantineDiscard,
    CaseRenameBeforeDirectoryBinding,
    RollbackAfterInitialVerification,
    RollbackAfterQuarantineVerification,
    WriteAfterInitialVerification,
    WriteAfterTempCreation,
    WriteBeforePersistence,
    WriteAfterPersistence,
}

#[cfg(test)]
type TestHook = (TestHookPoint, Box<dyn FnOnce()>);

#[cfg(test)]
thread_local! {
    static TEST_HOOK: std::cell::RefCell<Option<TestHook>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn set_test_hook(point: TestHookPoint, hook: impl FnOnce() + 'static) {
    TEST_HOOK.with(|slot| {
        let previous = slot.replace(Some((point, Box::new(hook))));
        assert!(
            previous.is_none(),
            "safe-file test hook was already installed"
        );
    });
}

pub(crate) fn run_test_hook(point: TestHookPoint) {
    #[cfg(test)]
    TEST_HOOK.with(|slot| {
        let hook = {
            let mut slot = slot.borrow_mut();
            if slot
                .as_ref()
                .is_some_and(|(candidate, _)| *candidate == point)
            {
                slot.take().map(|(_, hook)| hook)
            } else {
                None
            }
        };
        if let Some(hook) = hook {
            hook();
        }
    });
    #[cfg(not(test))]
    let _ = point;
}

impl FileSnapshot {
    pub(crate) fn capture(path: &Path) -> Result<Self> {
        let binding = DirectoryBinding::open(path)?;
        let snapshot = Self::capture_at(&binding, binding.target_name())?;
        if !binding.is_current()? {
            return Err(FileConflict::new(path).into());
        }
        Ok(snapshot)
    }

    pub(crate) fn capture_beneath(root: &Path, path: &Path) -> Result<Self> {
        let binding = DirectoryBinding::open_beneath(root, path)?;
        let snapshot = Self::capture_at(&binding, binding.target_name())?;
        binding.require_current(path)?;
        Ok(snapshot)
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
        let binding = DirectoryBinding::open(path)?;
        let unchanged = self.unchanged_at(&binding, binding.target_name())?;
        Ok(unchanged && binding.is_current()?)
    }

    /// Compare only the target file generation. This is used around compiler
    /// processes that legitimately mutate sibling build directories.
    pub(crate) fn file_generation_unchanged(&self, path: &Path) -> Result<bool> {
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(matches!(self, Self::Missing))
            }
            Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() > 1 {
            return Ok(false);
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        let final_metadata = file.metadata()?;
        if file_identity(&metadata) != file_identity(&final_metadata)
            || ReadGeneration::capture(&metadata) != ReadGeneration::capture(&final_metadata)
        {
            return Ok(false);
        }
        let current = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
        };
        let current_metadata = current.metadata()?;
        if !current_metadata.is_file()
            || current_metadata.nlink() > 1
            || file_identity(&final_metadata) != file_identity(&current_metadata)
            || ReadGeneration::capture(&final_metadata)
                != ReadGeneration::capture(&current_metadata)
        {
            return Ok(false);
        }
        Ok(match self {
            Self::Missing => false,
            Self::File {
                content: expected_content,
                metadata: expected_metadata,
                identity,
            } => {
                *identity == file_identity(&metadata)
                    && expected_metadata.generation == ReadGeneration::capture(&metadata)
                    && *expected_content == content
            }
        })
    }

    pub(crate) fn unchanged_beneath(&self, root: &Path, path: &Path) -> Result<bool> {
        let mut snapshots = FileSnapshotBatch::new(root)?;
        let current = snapshots.capture_read(path)?;
        snapshots.finish()?;
        Ok(self.matches_read(current.as_ref()))
    }

    fn capture_at(binding: &DirectoryBinding, name: &CStr) -> Result<Self> {
        Self::capture_from_directory(binding.raw_fd(), name, &binding.path_for(name))
    }

    fn capture_from_directory(directory_fd: RawFd, name: &CStr, path: &Path) -> Result<Self> {
        let fd = unsafe {
            libc::openat(
                directory_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ENOENT) => Ok(Self::Missing),
                Some(libc::ELOOP) => bail!("refusing to access symlink {}", path.display()),
                _ => Err(error).with_context(|| format!("inspecting {}", path.display())),
            };
        }

        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = file
            .metadata()
            .with_context(|| format!("inspecting {}", path.display()))?;
        if !metadata.is_file() {
            bail!("refusing to access non-regular file {}", path.display());
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(Self::File {
            content,
            metadata: PreservedMetadata::capture(path, &file, &metadata)?,
            identity: file_identity(&metadata),
        })
    }

    fn unchanged_at(&self, binding: &DirectoryBinding, name: &CStr) -> Result<bool> {
        let current = Self::capture_at(binding, name)?;
        Ok(self.matches(&current))
    }

    pub(crate) fn matches(&self, current: &Self) -> bool {
        match (self, current) {
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
                *content == *current
                    && *identity == *current_identity
                    && metadata.matches(current_metadata)
            }
            _ => false,
        }
    }

    pub(crate) fn matches_read(&self, current: Option<&ReadFileSnapshot>) -> bool {
        match (self, current) {
            (Self::Missing, None) => true,
            (
                Self::File {
                    content,
                    metadata,
                    identity,
                },
                Some(current),
            ) => {
                content == current.content()
                    && identity == current.identity()
                    && metadata.generation == ReadGeneration::capture(current.metadata())
                    && metadata.matches_file_metadata(current.metadata())
            }
            _ => false,
        }
    }

    // Path-only mutation is available only to this module's low-level tests.
    #[cfg(test)]
    fn replace(&self, path: &Path, content: &[u8]) -> Result<AppliedWrite> {
        atomic_replace(path, self, content)
    }

    pub(crate) fn replace_beneath(
        &self,
        root: &Path,
        path: &Path,
        content: &[u8],
    ) -> Result<AppliedWrite> {
        atomic_replace_beneath(root, path, self, content)
    }

    pub(crate) fn remove_beneath(&self, root: &Path, path: &Path) -> Result<Option<AppliedWrite>> {
        atomic_remove_beneath(root, path, self)
    }

    pub(crate) fn case_rename_beneath(
        &self,
        root: &Path,
        from: &Path,
        to: &Path,
    ) -> Result<AppliedRename> {
        atomic_case_rename_beneath(root, from, to, self)
    }
}

pub(crate) struct FileSnapshotBatch {
    root: PathBuf,
    root_identity: FileIdentity,
    root_directory: std::fs::File,
    directories: HashMap<PathBuf, SnapshotDirectory>,
    identities: HashMap<PathBuf, FileIdentity>,
}

struct SnapshotDirectory {
    identity: FileIdentity,
    directory: std::fs::File,
}

const SNAPSHOT_DIRECTORY_CACHE_SIZE: usize = 16;

impl FileSnapshotBatch {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let canonical_root = root
            .canonicalize()
            .with_context(|| format!("canonicalizing snapshot root {}", root.display()))?;
        if canonical_root != root {
            return Err(FileConflict::new(root).into());
        }
        let metadata = std::fs::symlink_metadata(root)
            .with_context(|| format!("inspecting snapshot root {}", root.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("refusing to use non-directory root {}", root.display());
        }
        let identity = file_identity(&metadata);
        let directory = open_directory(root)?;
        if file_identity(&directory.metadata()?) != identity {
            return Err(FileConflict::new(root).into());
        }
        Ok(Self {
            root: root.to_path_buf(),
            root_identity: identity.clone(),
            root_directory: directory,
            directories: HashMap::new(),
            identities: HashMap::from([(root.to_path_buf(), identity)]),
        })
    }

    pub(crate) fn capture(&mut self, path: &Path) -> Result<FileSnapshot> {
        let (directory_fd, file_name) = self.bind_file(path)?;
        FileSnapshot::capture_from_directory(directory_fd, &file_name, path)
    }

    pub(crate) fn capture_read(&mut self, path: &Path) -> Result<Option<ReadFileSnapshot>> {
        let Some((directory_fd, file_name)) = self.bind_file_if_parent_exists(path)? else {
            return Ok(None);
        };
        let fd = unsafe {
            libc::openat(
                directory_fd,
                file_name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ENOENT) => Ok(None),
                Some(libc::ELOOP) => bail!("refusing to access symlink {}", path.display()),
                _ => Err(error).with_context(|| format!("opening {}", path.display())),
            };
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = file
            .metadata()
            .with_context(|| format!("inspecting {}", path.display()))?;
        if !metadata.is_file() {
            bail!("refusing to access non-regular file {}", path.display());
        }
        if metadata.nlink() > 1 {
            bail!(
                "refusing to access hard-linked file {} ({} links)",
                path.display(),
                metadata.nlink()
            );
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .with_context(|| format!("reading {}", path.display()))?;
        let final_metadata = file
            .metadata()
            .with_context(|| format!("reinspecting {}", path.display()))?;
        if !same_read_generation(&metadata, &final_metadata) {
            return Err(FileConflict::new(path).into());
        }
        run_test_hook(TestHookPoint::ReadAfterContent);
        let current_fd = unsafe {
            libc::openat(
                directory_fd,
                file_name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if current_fd < 0 {
            let error = std::io::Error::last_os_error();
            return Err(anyhow::Error::from(FileConflict::new(path)))
                .context(format!("could not verify {}: {error}", path.display()));
        }
        let current = unsafe { std::fs::File::from_raw_fd(current_fd) };
        let current_metadata = current
            .metadata()
            .with_context(|| format!("verifying {}", path.display()))?;
        if !current_metadata.is_file()
            || current_metadata.nlink() > 1
            || file_identity(&current_metadata) != file_identity(&final_metadata)
            || !same_read_generation(&final_metadata, &current_metadata)
        {
            return Err(FileConflict::new(path).into());
        }
        Ok(Some(ReadFileSnapshot {
            content,
            identity: file_identity(&final_metadata),
            metadata: final_metadata,
        }))
    }

    fn bind_file(&mut self, path: &Path) -> Result<(RawFd, CString)> {
        self.bind_file_if_parent_exists(path)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("snapshot parent does not exist for {}", path.display()),
            )
            .into()
        })
    }

    fn bind_file_if_parent_exists(&mut self, path: &Path) -> Result<Option<(RawFd, CString)>> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            anyhow::anyhow!(
                "snapshot path {} is outside root {}",
                path.display(),
                self.root.display()
            )
        })?;
        let parent = relative
            .parent()
            .ok_or_else(|| anyhow::anyhow!("snapshot path has no parent: {}", path.display()))?;
        let file_name = relative
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("snapshot path has no file name: {}", path.display()))?;
        let file_name = CString::new(file_name.as_bytes())
            .with_context(|| format!("path contains a null byte: {}", path.display()))?;
        let Some(directory_fd) = self.bind_directory_if_exists(parent, path)? else {
            return Ok(None);
        };
        Ok(Some((directory_fd, file_name)))
    }

    pub(crate) fn finish(self) -> Result<()> {
        if file_identity(&self.root_directory.metadata()?) != self.root_identity {
            return Err(FileConflict::new(&self.root).into());
        }
        for (relative, bound) in &self.directories {
            if file_identity(&bound.directory.metadata()?) != bound.identity {
                return Err(FileConflict::new(&self.root.join(relative)).into());
            }
        }
        for (path, identity) in self.identities {
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("verifying snapshot directory {}", path.display()))?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || file_identity(&metadata) != identity
            {
                return Err(FileConflict::new(&path).into());
            }
        }
        Ok(())
    }

    fn bind_directory_if_exists(
        &mut self,
        relative: &Path,
        target_path: &Path,
    ) -> Result<Option<RawFd>> {
        if relative.as_os_str().is_empty() {
            return Ok(Some(self.root_directory.as_raw_fd()));
        }
        if let Some(directory) = self.directories.get(relative) {
            return Ok(Some(directory.directory.as_raw_fd()));
        }

        let mut absolute_path = self.root.clone();
        let mut directory_fd = self.root_directory.as_raw_fd();
        let mut opened_directory = None;
        for component in relative.components() {
            let std::path::Component::Normal(name) = component else {
                bail!(
                    "refusing non-normalized snapshot path {}",
                    target_path.display()
                );
            };
            absolute_path.push(name);
            let name = CString::new(name.as_bytes())
                .with_context(|| format!("path contains a null byte: {}", target_path.display()))?;
            let fd = unsafe {
                libc::openat(
                    directory_fd,
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                return if matches!(
                    error.raw_os_error(),
                    Some(libc::ELOOP) | Some(libc::ENOTDIR)
                ) {
                    Err(FileConflict::new(target_path).into())
                } else if error.kind() == std::io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(error).with_context(|| {
                        format!("binding snapshot directory {}", absolute_path.display())
                    })
                };
            }
            let directory = unsafe { std::fs::File::from_raw_fd(fd) };
            let metadata = directory.metadata()?;
            if !metadata.is_dir() {
                return Err(FileConflict::new(target_path).into());
            }
            let identity = file_identity(&metadata);
            if self
                .identities
                .get(&absolute_path)
                .is_some_and(|expected| *expected != identity)
            {
                return Err(FileConflict::new(target_path).into());
            }
            self.identities
                .entry(absolute_path.clone())
                .or_insert_with(|| identity.clone());
            directory_fd = directory.as_raw_fd();
            opened_directory = Some((directory, identity));
        }
        if self.directories.len() >= SNAPSHOT_DIRECTORY_CACHE_SIZE {
            let evicted = self
                .directories
                .keys()
                .next()
                .expect("nonempty full directory cache")
                .clone();
            self.directories.remove(&evicted);
        }
        let (directory, identity) =
            opened_directory.expect("a nonempty relative directory opens at least one component");
        self.directories.insert(
            relative.to_path_buf(),
            SnapshotDirectory {
                identity,
                directory,
            },
        );
        Ok(Some(
            self.directories
                .get(relative)
                .expect("bound directory was inserted")
                .directory
                .as_raw_fd(),
        ))
    }
}

#[cfg(test)]
fn read_regular_file_beneath(root: &Path, path: &Path) -> Result<ReadFileSnapshot> {
    let binding = DirectoryBinding::open_beneath(root, path)?;
    let fd = unsafe {
        libc::openat(
            binding.raw_fd(),
            binding.target_name().as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ELOOP) => bail!("refusing to access symlink {}", path.display()),
            _ => Err(error).with_context(|| format!("opening {}", path.display())),
        };
    }

    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .with_context(|| format!("inspecting {}", path.display()))?;
    if !metadata.is_file() {
        bail!("refusing to access non-regular file {}", path.display());
    }
    if metadata.nlink() > 1 {
        bail!(
            "refusing to access hard-linked file {} ({} links)",
            path.display(),
            metadata.nlink()
        );
    }
    let identity = file_identity(&metadata);
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .with_context(|| format!("reading {}", path.display()))?;
    let final_metadata = file
        .metadata()
        .with_context(|| format!("reinspecting {}", path.display()))?;
    if !same_read_generation(&metadata, &final_metadata) {
        return Err(FileConflict::new(path).into());
    }
    run_test_hook(TestHookPoint::ReadAfterContent);
    binding.require_current(path)?;
    let current_fd = unsafe {
        libc::openat(
            binding.raw_fd(),
            binding.target_name().as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if current_fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(anyhow::Error::from(FileConflict::new(path)))
            .context(format!("could not verify {}: {error}", path.display()));
    }
    let current = unsafe { std::fs::File::from_raw_fd(current_fd) };
    let current_metadata = current
        .metadata()
        .with_context(|| format!("verifying {}", path.display()))?;
    if !current_metadata.is_file()
        || current_metadata.nlink() > 1
        || file_identity(&current_metadata) != identity
        || !same_read_generation(&final_metadata, &current_metadata)
    {
        return Err(FileConflict::new(path).into());
    }
    binding.require_current(path)?;
    Ok(ReadFileSnapshot {
        content,
        identity,
        metadata: final_metadata,
    })
}

fn same_read_generation(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    file_identity(left) == file_identity(right)
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[derive(Debug)]
pub(crate) struct AppliedWrite {
    path: PathBuf,
    before: FileSnapshot,
    after: FileSnapshot,
    binding: DirectoryBindingSnapshot,
}

pub(crate) struct AppliedRename {
    from: PathBuf,
    to: PathBuf,
    after: FileSnapshot,
    from_binding: DirectoryBindingSnapshot,
    to_binding: DirectoryBindingSnapshot,
}

impl AppliedRename {
    pub(crate) fn rollback(self) -> Result<()> {
        let from_binding = self.from_binding.reopen(&self.from)?;
        let to_binding = self.to_binding.reopen(&self.to)?;
        case_rename_bound(
            &self.to,
            &to_binding,
            &self.from,
            &from_binding,
            &self.after,
        )?;
        Ok(())
    }
}

impl AppliedWrite {
    pub(crate) fn rollback(self) -> Result<()> {
        let binding = self.binding.reopen(&self.path)?;
        if !binding.is_current()? {
            return Err(FileConflict::new(&self.path).into());
        }
        require_generation(&self.path, &binding, binding.target_name(), &self.after)?;
        run_test_hook(TestHookPoint::RollbackAfterInitialVerification);
        rollback_applied(&self.path, &binding, &self.before, &self.after, true)
    }
}

#[cfg(test)]
fn atomic_replace(path: &Path, expected: &FileSnapshot, content: &[u8]) -> Result<AppliedWrite> {
    let metadata = match expected {
        FileSnapshot::Missing => None,
        FileSnapshot::File { metadata, .. } => Some(metadata),
    };
    let binding = DirectoryBinding::open(path)?;
    atomic_replace_inner(path, expected, content, metadata, binding)
}

fn atomic_replace_beneath(
    root: &Path,
    path: &Path,
    expected: &FileSnapshot,
    content: &[u8],
) -> Result<AppliedWrite> {
    let metadata = match expected {
        FileSnapshot::Missing => None,
        FileSnapshot::File { metadata, .. } => Some(metadata),
    };
    run_test_hook(TestHookPoint::WriteBeforeDirectoryBinding);
    let binding = DirectoryBinding::open_beneath(root, path)?;
    atomic_replace_inner(path, expected, content, metadata, binding)
}

fn atomic_replace_inner(
    path: &Path,
    expected: &FileSnapshot,
    content: &[u8],
    replacement_metadata: Option<&PreservedMetadata>,
    binding: DirectoryBinding,
) -> Result<AppliedWrite> {
    require_generation(path, &binding, binding.target_name(), expected)?;
    if matches!(
        expected,
        FileSnapshot::File { metadata, .. } if metadata.permissions.readonly()
    ) {
        bail!("refusing to replace read-only file {}", path.display());
    }

    run_test_hook(TestHookPoint::WriteAfterInitialVerification);
    binding.require_current(path)?;
    let mut temp = TemporaryEntry::create(&binding, ".mdc-write-")?;
    run_test_hook(TestHookPoint::WriteAfterTempCreation);
    binding.require_current(path)?;
    temp.file
        .write_all(content)
        .with_context(|| format!("writing temporary file for {}", path.display()))?;

    #[cfg(target_os = "macos")]
    reject_extended_acl(&temp.file, path)?;
    if let Some(metadata) = replacement_metadata {
        metadata.apply(&mut temp.file, path)?;
    }
    temp.file
        .sync_all()
        .with_context(|| format!("syncing temporary file for {}", path.display()))?;
    let written = temp.snapshot(content, path)?;

    require_generation(path, &binding, binding.target_name(), expected)?;
    run_test_hook(TestHookPoint::WriteBeforePersistence);
    binding.require_current(path)?;

    let mut previous = if matches!(expected, FileSnapshot::File { .. }) {
        let mut quarantine = QuarantinedEntry::take(&binding, binding.target_name())?;
        require_quarantined_generation(path, &binding, &mut quarantine, expected)?;
        Some(quarantine)
    } else {
        None
    };

    if !binding.is_current()? {
        if let Some(quarantine) = &mut previous {
            let _ = quarantine.restore(binding.target_name());
        }
        return Err(FileConflict::new(path).into());
    }
    if let Err(error) = temp.persist(binding.target_name()) {
        if let Some(quarantine) = &mut previous {
            let _ = quarantine.restore(binding.target_name());
        }
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(FileConflict::new(path).into());
        }
        return Err(error).with_context(|| format!("persisting {}", path.display()));
    }
    drop(temp);

    run_test_hook(TestHookPoint::WriteAfterPersistence);
    let after = FileSnapshot::capture_at(&binding, binding.target_name()).map_err(|error| {
        anyhow::Error::from(FileConflict::new(path)).context(format!(
            "could not verify the persisted generation of {}: {error}",
            path.display()
        ))
    })?;
    if !written.matches(&after) || !binding.is_current()? {
        if written.matches(&after) {
            let _ = undo_persisted_write(path, &binding, &written, previous.as_mut());
        }
        return Err(FileConflict::new(path).into());
    }

    binding.require_current(path)?;
    if let Some(quarantine) = &mut previous {
        discard_quarantine(path, &binding, quarantine, expected, false)?;
    }
    binding.require_current(path)?;
    binding.sync();
    Ok(AppliedWrite {
        path: path.to_path_buf(),
        before: expected.clone(),
        after,
        binding: binding.into_snapshot(),
    })
}

#[cfg(test)]
fn atomic_remove(path: &Path, expected: &FileSnapshot) -> Result<Option<AppliedWrite>> {
    if matches!(expected, FileSnapshot::Missing) {
        return Ok(None);
    }
    let binding = DirectoryBinding::open(path)?;
    atomic_remove_inner(path, expected, binding)
}

fn atomic_remove_beneath(
    root: &Path,
    path: &Path,
    expected: &FileSnapshot,
) -> Result<Option<AppliedWrite>> {
    if matches!(expected, FileSnapshot::Missing) {
        return Ok(None);
    }
    run_test_hook(TestHookPoint::RemoveBeforeDirectoryBinding);
    let binding = DirectoryBinding::open_beneath(root, path)?;
    atomic_remove_inner(path, expected, binding)
}

fn atomic_remove_inner(
    path: &Path,
    expected: &FileSnapshot,
    binding: DirectoryBinding,
) -> Result<Option<AppliedWrite>> {
    require_generation(path, &binding, binding.target_name(), expected)?;
    binding.require_current(path)?;
    let mut quarantine = QuarantinedEntry::take(&binding, binding.target_name())?;
    require_quarantined_generation(path, &binding, &mut quarantine, expected)?;
    if require_generation(
        path,
        &binding,
        binding.target_name(),
        &FileSnapshot::Missing,
    )
    .is_err()
        || !binding.is_current()?
    {
        return conflict_with_restoration(path, &binding, &mut quarantine);
    }
    discard_quarantine(path, &binding, &mut quarantine, expected, false)?;
    binding.require_current(path)?;
    binding.sync();
    Ok(Some(AppliedWrite {
        path: path.to_path_buf(),
        before: expected.clone(),
        after: FileSnapshot::Missing,
        binding: binding.into_snapshot(),
    }))
}

#[derive(Debug)]
struct TemporaryEntry<'binding> {
    binding: &'binding DirectoryBinding,
    name: CString,
    file: std::fs::File,
    live: bool,
}

impl<'binding> TemporaryEntry<'binding> {
    fn create(binding: &'binding DirectoryBinding, prefix: &str) -> Result<Self> {
        for _ in 0..128 {
            let name = unique_name(prefix)?;
            let fd = unsafe {
                libc::openat(
                    binding.raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o666,
                )
            };
            if fd >= 0 {
                return Ok(Self {
                    binding,
                    name,
                    file: unsafe { std::fs::File::from_raw_fd(fd) },
                    live: true,
                });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error).with_context(|| {
                    format!("creating temporary file in {}", binding.parent.display())
                });
            }
        }
        bail!(
            "could not allocate a unique temporary file in {}",
            binding.parent.display()
        )
    }

    fn snapshot(&self, content: &[u8], path: &Path) -> Result<FileSnapshot> {
        let metadata = self.file.metadata()?;
        Ok(FileSnapshot::File {
            content: content.to_vec(),
            metadata: PreservedMetadata::capture(path, &self.file, &metadata)?,
            identity: file_identity(&metadata),
        })
    }

    fn persist(&mut self, target: &CStr) -> std::io::Result<()> {
        rename_noreplace(self.binding.raw_fd(), &self.name, target)?;
        self.live = false;
        Ok(())
    }
}

impl Drop for TemporaryEntry<'_> {
    fn drop(&mut self) {
        if self.live {
            let _ = unlink_entry(self.binding.raw_fd(), &self.name);
        }
    }
}

#[derive(Debug)]
struct QuarantinedEntry<'binding> {
    binding: &'binding DirectoryBinding,
    name: CString,
    live: bool,
}

impl<'binding> QuarantinedEntry<'binding> {
    fn take(binding: &'binding DirectoryBinding, source: &CStr) -> Result<Self> {
        for _ in 0..128 {
            let name = unique_name(".mdc-quarantine-")?;
            match rename_noreplace(binding.raw_fd(), source, &name) {
                Ok(()) => {
                    return Ok(Self {
                        binding,
                        name,
                        live: true,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(FileConflict::new(&binding.path_for(source)).into())
                }
                Err(error) => return Err(error.into()),
            }
        }
        bail!(
            "could not allocate a quarantine name in {}",
            binding.parent.display()
        )
    }

    fn name(&self) -> &CStr {
        &self.name
    }

    fn restore(&mut self, target: &CStr) -> std::io::Result<()> {
        rename_noreplace(self.binding.raw_fd(), &self.name, target)?;
        self.live = false;
        Ok(())
    }

    fn discard(&mut self) -> std::io::Result<()> {
        unlink_entry(self.binding.raw_fd(), &self.name)?;
        self.live = false;
        Ok(())
    }
}

fn unique_name(prefix: &str) -> Result<CString> {
    CString::new(format!("{prefix}{}", uuid::Uuid::new_v4()))
        .context("generated temporary name contained a null byte")
}

fn conflict_with_restoration<T>(
    path: &Path,
    binding: &DirectoryBinding,
    quarantine: &mut QuarantinedEntry<'_>,
) -> Result<T> {
    let quarantine_path = binding.path_for(quarantine.name());
    let restore = quarantine.restore(binding.target_name());
    let conflict = anyhow::Error::from(FileConflict::new(path));
    match restore {
        Ok(()) => Err(conflict),
        Err(error) => Err(conflict.context(format!(
            "uncertain generation preserved at {}; restoring {} failed: {error}",
            quarantine_path.display(),
            path.display()
        ))),
    }
}

fn require_generation(
    path: &Path,
    binding: &DirectoryBinding,
    name: &CStr,
    expected: &FileSnapshot,
) -> Result<()> {
    match expected.unchanged_at(binding, name) {
        Ok(true) => Ok(()),
        Ok(false) => Err(FileConflict::new(path).into()),
        Err(error) => Err(
            anyhow::Error::from(FileConflict::new(path)).context(format!(
                "could not verify the generation of {}: {error}",
                binding.display_path(name)
            )),
        ),
    }
}

fn require_quarantined_generation(
    path: &Path,
    binding: &DirectoryBinding,
    quarantine: &mut QuarantinedEntry<'_>,
    expected: &FileSnapshot,
) -> Result<()> {
    let verification = expected.unchanged_at(binding, quarantine.name());
    match verification {
        Ok(true) => Ok(()),
        Ok(false) => conflict_with_restoration(path, binding, quarantine),
        Err(error) => conflict_with_restoration(path, binding, quarantine).map_err(|conflict| {
            conflict.context(format!(
                "could not verify quarantined generation of {}: {error}",
                path.display()
            ))
        }),
    }
}

fn discard_quarantine(
    path: &Path,
    binding: &DirectoryBinding,
    quarantine: &mut QuarantinedEntry<'_>,
    expected: &FileSnapshot,
    rollback_hook: bool,
) -> Result<()> {
    require_quarantined_generation(path, binding, quarantine, expected)?;
    if rollback_hook {
        run_test_hook(TestHookPoint::RollbackAfterQuarantineVerification);
    }
    require_quarantined_generation(path, binding, quarantine, expected)?;
    binding.require_current(path)?;
    // Unix has no content-CAS unlink. This final descriptor-relative check
    // catches observable open-fd edits; any uncertainty preserves quarantine.
    quarantine
        .discard()
        .with_context(|| format!("discarding verified generation of {}", path.display()))?;
    Ok(())
}

fn prepare_snapshot_temp<'binding>(
    binding: &'binding DirectoryBinding,
    path: &Path,
    snapshot: &FileSnapshot,
) -> Result<(TemporaryEntry<'binding>, FileSnapshot)> {
    let FileSnapshot::File {
        content, metadata, ..
    } = snapshot
    else {
        bail!("cannot prepare a temporary file for a missing snapshot");
    };
    let mut temp = TemporaryEntry::create(binding, ".mdc-restore-")?;
    temp.file.write_all(content)?;
    #[cfg(target_os = "macos")]
    reject_extended_acl(&temp.file, path)?;
    metadata.apply(&mut temp.file, path)?;
    temp.file.sync_all()?;
    let written = temp.snapshot(content, path)?;
    Ok((temp, written))
}

fn undo_persisted_write(
    path: &Path,
    binding: &DirectoryBinding,
    written: &FileSnapshot,
    previous: Option<&mut QuarantinedEntry<'_>>,
) -> Result<()> {
    let mut current = QuarantinedEntry::take(binding, binding.target_name())?;
    require_quarantined_generation(path, binding, &mut current, written)?;
    if let Some(previous) = previous {
        previous.restore(binding.target_name()).with_context(|| {
            format!("restoring {} after interrupted persistence", path.display())
        })?;
    }
    discard_quarantine(path, binding, &mut current, written, false)
}

fn rollback_applied(
    path: &Path,
    binding: &DirectoryBinding,
    before: &FileSnapshot,
    after: &FileSnapshot,
    rollback_hook: bool,
) -> Result<()> {
    match after {
        FileSnapshot::Missing => {
            require_generation(path, binding, binding.target_name(), after)?;
            let (mut temp, restored) = prepare_snapshot_temp(binding, path, before)?;
            binding.require_current(path)?;
            temp.persist(binding.target_name()).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::Error::from(FileConflict::new(path))
                } else {
                    anyhow::Error::from(error)
                }
            })?;
            if require_generation(path, binding, binding.target_name(), &restored).is_err()
                || !binding.is_current()?
            {
                return Err(FileConflict::new(path).into());
            }
        }
        FileSnapshot::File { .. } => {
            let mut written = QuarantinedEntry::take(binding, binding.target_name())?;
            require_quarantined_generation(path, binding, &mut written, after)?;
            if rollback_hook {
                run_test_hook(TestHookPoint::RollbackAfterQuarantineVerification);
            }
            match before {
                FileSnapshot::Missing => {
                    discard_quarantine(path, binding, &mut written, after, false)?;
                }
                FileSnapshot::File { .. } => {
                    let (mut temp, restored) = prepare_snapshot_temp(binding, path, before)?;
                    if !binding.is_current()? {
                        return conflict_with_restoration(path, binding, &mut written);
                    }
                    temp.persist(binding.target_name()).map_err(|error| {
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            anyhow::Error::from(FileConflict::new(path))
                        } else {
                            anyhow::Error::from(error)
                        }
                    })?;
                    if require_generation(path, binding, binding.target_name(), &restored).is_err()
                        || !binding.is_current()?
                    {
                        let _ = undo_persisted_write(path, binding, &restored, Some(&mut written));
                        return Err(FileConflict::new(path).into());
                    }
                    discard_quarantine(path, binding, &mut written, after, false)?;
                }
            }
        }
    }
    binding.require_current(path)?;
    binding.sync();
    Ok(())
}

fn unlink_entry(directory_fd: RawFd, name: &CStr) -> std::io::Result<()> {
    if unsafe { libc::unlinkat(directory_fd, name.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn rename_noreplace(directory_fd: RawFd, from: &CStr, to: &CStr) -> std::io::Result<()> {
    rename_noreplace_at(directory_fd, from, directory_fd, to)
}

#[cfg(target_os = "macos")]
fn rename_noreplace_at(
    from_fd: RawFd,
    from: &CStr,
    to_fd: RawFd,
    to: &CStr,
) -> std::io::Result<()> {
    unsafe extern "C" {
        fn renameatx_np(
            from_fd: libc::c_int,
            from: *const libc::c_char,
            to_fd: libc::c_int,
            to: *const libc::c_char,
            flags: libc::c_uint,
        ) -> libc::c_int;
    }
    const RENAME_EXCL: libc::c_uint = 0x0000_0004;
    if unsafe { renameatx_np(from_fd, from.as_ptr(), to_fd, to.as_ptr(), RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace(directory_fd: RawFd, from: &CStr, to: &CStr) -> std::io::Result<()> {
    rename_noreplace_at(directory_fd, from, directory_fd, to)
}

#[cfg(target_os = "linux")]
fn rename_noreplace_at(
    from_fd: RawFd,
    from: &CStr,
    to_fd: RawFd,
    to: &CStr,
) -> std::io::Result<()> {
    const RENAME_NOREPLACE: libc::c_uint = 1;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from_fd,
            from.as_ptr(),
            to_fd,
            to.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_noreplace(directory_fd: RawFd, from: &CStr, to: &CStr) -> std::io::Result<()> {
    rename_noreplace_at(directory_fd, from, directory_fd, to)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_noreplace_at(
    from_fd: RawFd,
    from: &CStr,
    to_fd: RawFd,
    to: &CStr,
) -> std::io::Result<()> {
    if unsafe { libc::linkat(from_fd, from.as_ptr(), to_fd, to.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if let Err(error) = unlink_entry(from_fd, from) {
        let _ = unlink_entry(to_fd, to);
        return Err(error);
    }
    Ok(())
}

fn atomic_case_rename_beneath(
    root: &Path,
    from: &Path,
    to: &Path,
    expected: &FileSnapshot,
) -> Result<AppliedRename> {
    if from == to {
        bail!("source and destination of a case rename must differ");
    }
    if expected.identity().is_none() {
        bail!("cannot rename missing file {}", from.display());
    }
    run_test_hook(TestHookPoint::CaseRenameBeforeDirectoryBinding);
    let from_binding = DirectoryBinding::open_beneath(root, from)?;
    let to_binding = DirectoryBinding::open_beneath(root, to)?;
    let after = case_rename_bound(from, &from_binding, to, &to_binding, expected)?;
    Ok(AppliedRename {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        after,
        from_binding: from_binding.into_snapshot(),
        to_binding: to_binding.into_snapshot(),
    })
}

fn case_rename_bound(
    from: &Path,
    from_binding: &DirectoryBinding,
    to: &Path,
    to_binding: &DirectoryBinding,
    expected: &FileSnapshot,
) -> Result<FileSnapshot> {
    require_generation(from, from_binding, from_binding.target_name(), expected)?;
    let destination = FileSnapshot::capture_at(to_binding, to_binding.target_name())?;
    if destination.identity() != expected.identity() {
        bail!(
            "case-rename destination does not identify the source file: {}",
            to.display()
        );
    }
    from_binding.require_current(from)?;
    to_binding.require_current(to)?;

    rename_entry_through_temporary(from, from_binding, to, to_binding)?;
    let after = FileSnapshot::capture_at(to_binding, to_binding.target_name())?;
    if !expected.matches(&after) || !from_binding.is_current()? || !to_binding.is_current()? {
        if expected.matches(&after) {
            let _ = rename_entry_through_temporary(to, to_binding, from, from_binding);
        }
        return Err(FileConflict::new(from).into());
    }
    from_binding.sync();
    if file_identity(&from_binding.directory.metadata()?)
        != file_identity(&to_binding.directory.metadata()?)
    {
        to_binding.sync();
    }
    Ok(after)
}

fn rename_entry_through_temporary(
    from: &Path,
    from_binding: &DirectoryBinding,
    to: &Path,
    to_binding: &DirectoryBinding,
) -> Result<()> {
    let temporary = loop {
        let candidate = unique_name(".mdc-case-rename-")?;
        match rename_noreplace(
            from_binding.raw_fd(),
            from_binding.target_name(),
            &candidate,
        ) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("renaming {}", from.display()))
            }
        }
    };
    if let Err(error) = rename_noreplace_at(
        from_binding.raw_fd(),
        &temporary,
        to_binding.raw_fd(),
        to_binding.target_name(),
    ) {
        let restore = rename_noreplace(
            from_binding.raw_fd(),
            &temporary,
            from_binding.target_name(),
        );
        return match restore {
            Ok(()) => Err(error).with_context(|| format!("renaming to {}", to.display())),
            Err(restore_error) => bail!(
                "renaming to {} failed: {error}; restoring {} also failed: {restore_error}",
                to.display(),
                from.display()
            ),
        };
    }
    Ok(())
}

pub(crate) fn atomic_create_if_missing_beneath(
    root: &Path,
    path: &Path,
    content: &[u8],
) -> Result<bool> {
    run_test_hook(TestHookPoint::WriteBeforeDirectoryBinding);
    let binding = DirectoryBinding::open_beneath(root, path)?;
    let snapshot = FileSnapshot::capture_at(&binding, binding.target_name())?;
    if !matches!(snapshot, FileSnapshot::Missing) {
        binding.require_current(path)?;
        return Ok(false);
    }
    atomic_replace_inner(path, &snapshot, content, None, binding)?;
    Ok(true)
}

pub(crate) fn ensure_regular_file_beneath(root: &Path, path: &Path) -> Result<()> {
    let binding = DirectoryBinding::open_beneath(root, path)?;
    let fd = unsafe {
        libc::openat(
            binding.raw_fd(),
            binding.target_name().as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ELOOP) {
            bail!("refusing to access symlink {}", path.display())
        } else {
            Err(error).with_context(|| format!("opening regular file {}", path.display()))
        };
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("refusing to access non-regular file {}", path.display());
    }
    let identity = file_identity(&metadata);
    binding.require_current(path)?;
    let current_fd = unsafe {
        libc::openat(
            binding.raw_fd(),
            binding.target_name().as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if current_fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(anyhow::Error::from(FileConflict::new(path))).context(format!(
            "could not verify regular file {}: {error}",
            path.display()
        ));
    }
    let current = unsafe { std::fs::File::from_raw_fd(current_fd) };
    let current_metadata = current.metadata()?;
    if !current_metadata.is_file() || file_identity(&current_metadata) != identity {
        return Err(FileConflict::new(path).into());
    }
    binding.require_current(path)
}

pub(crate) fn regular_directory_exists_beneath(root: &Path, directory: &Path) -> Result<bool> {
    let binding = DirectoryBinding::open_beneath(root, directory)?;
    binding.require_current(directory)?;
    match open_directory_entry(binding.raw_fd(), binding.target_name()) {
        Ok(_) => {
            binding.require_current(directory)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            binding.require_current(directory)?;
            Ok(false)
        }
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ELOOP) | Some(libc::ENOTDIR)
            ) =>
        {
            bail!("refusing to use non-directory path {}", directory.display())
        }
        Err(error) => {
            Err(error).with_context(|| format!("opening regular directory {}", directory.display()))
        }
    }
}

pub(crate) fn ensure_regular_directory_tree(root: &Path, directory: &Path) -> Result<bool> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing directory root {}", root.display()))?;
    let relative = directory.strip_prefix(&root).map_err(|_| {
        anyhow::anyhow!(
            "directory {} is outside root {}",
            directory.display(),
            root.display()
        )
    })?;
    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => {
                use std::os::unix::ffi::OsStrExt;
                CString::new(name.as_bytes()).context("directory name contains a null byte")
            }
            _ => bail!(
                "refusing non-normalized directory path {}",
                directory.display()
            ),
        })
        .collect::<Result<Vec<_>>>()?;

    let root_metadata = std::fs::symlink_metadata(&root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        bail!("refusing to use non-directory root {}", root.display());
    }
    let mut paths = vec![root.clone()];
    let mut identities = vec![file_identity(&root_metadata)];
    let mut directories = vec![open_directory(&root)?];
    if file_identity(&directories[0].metadata()?) != identities[0] {
        return Err(FileConflict::new(directory).into());
    }
    let mut created = Vec::with_capacity(components.len());

    for name in &components {
        if !directory_chain_is_current(&paths, &identities)? {
            rollback_created_directories(&directories, &components, &created);
            return Err(FileConflict::new(directory).into());
        }
        let parent_fd = directories
            .last()
            .expect("directory traversal always retains the root")
            .as_raw_fd();
        let was_created = if unsafe { libc::mkdirat(parent_fd, name.as_ptr(), 0o777) } == 0 {
            true
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                false
            } else {
                rollback_created_directories(&directories, &components, &created);
                return Err(error).with_context(|| {
                    format!("creating directory tree for {}", directory.display())
                });
            }
        };
        let fd = unsafe {
            libc::openat(
                parent_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            rollback_created_directories(&directories, &components, &created);
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("opening directory tree for {}", directory.display()));
        }
        let child = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = child.metadata()?;
        paths.push(
            paths
                .last()
                .expect("root path exists")
                .join(std::ffi::OsStr::from_bytes(name.to_bytes())),
        );
        identities.push(file_identity(&metadata));
        directories.push(child);
        created.push(was_created);
        if !directory_chain_is_current(&paths, &identities)? {
            rollback_created_directories(&directories, &components, &created);
            return Err(FileConflict::new(directory).into());
        }
    }

    Ok(created.into_iter().any(|value| value))
}

pub(crate) fn remove_empty_directory_beneath(root: &Path, directory: &Path) -> Result<bool> {
    let binding = DirectoryBinding::open_beneath(root, directory)?;
    binding.require_current(directory)?;
    let result = unsafe {
        libc::unlinkat(
            binding.raw_fd(),
            binding.target_name().as_ptr(),
            libc::AT_REMOVEDIR,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOENT) | Some(libc::ENOTEMPTY) | Some(libc::EEXIST) => Ok(false),
            Some(libc::ENOTDIR) | Some(libc::ELOOP) => Err(FileConflict::new(directory).into()),
            _ => Err(error)
                .with_context(|| format!("removing empty directory {}", directory.display())),
        };
    }
    binding.require_current(directory)?;
    binding.sync();
    Ok(true)
}

pub(crate) fn remove_directory_tree_beneath(root: &Path, directory: &Path) -> Result<bool> {
    run_test_hook(TestHookPoint::RemoveTreeBeforeDirectoryBinding);
    let binding = DirectoryBinding::open_beneath(root, directory)?;
    run_test_hook(TestHookPoint::RemoveTreeAfterDirectoryBinding);
    binding.require_current(directory)?;
    run_test_hook(TestHookPoint::RemoveTreeBeforeTargetOpen);

    let tree = match open_directory_entry(binding.raw_fd(), binding.target_name()) {
        Ok(tree) => tree,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            binding.require_current(directory)?;
            require_generation(
                directory,
                &binding,
                binding.target_name(),
                &FileSnapshot::Missing,
            )?;
            return Ok(false);
        }
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ELOOP) | Some(libc::ENOTDIR)
            ) =>
        {
            bail!(
                "refusing to remove non-directory tree {}",
                directory.display()
            )
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("opening directory tree {}", directory.display()))
        }
    };
    let identity = file_identity(&tree.metadata()?);
    run_test_hook(TestHookPoint::RemoveTreeAfterTargetOpen);
    binding.require_current(directory)?;
    let current =
        open_directory_entry(binding.raw_fd(), binding.target_name()).map_err(|error| {
            anyhow::Error::from(FileConflict::new(directory)).context(format!(
                "could not verify directory tree {} before cleanup: {error}",
                directory.display()
            ))
        })?;
    if file_identity(&current.metadata()?) != identity {
        return Err(FileConflict::new(directory).into());
    }
    drop(current);
    let quarantine = unique_name(".mdc-tree-quarantine-")?;
    rename_noreplace(binding.raw_fd(), binding.target_name(), &quarantine)
        .with_context(|| format!("quarantining directory tree {}", directory.display()))?;
    let quarantined = open_directory_entry(binding.raw_fd(), &quarantine).map_err(|error| {
        anyhow::Error::from(FileConflict::new(directory)).context(format!(
            "could not verify quarantined directory tree {}: {error}",
            directory.display()
        ))
    })?;
    if file_identity(&quarantined.metadata()?) != identity || !binding.is_current()? {
        let restore = rename_noreplace(binding.raw_fd(), &quarantine, binding.target_name());
        return match restore {
            Ok(()) => Err(FileConflict::new(directory).into()),
            Err(error) => Err(
                anyhow::Error::from(FileConflict::new(directory)).context(format!(
                    "uncertain directory generation preserved at {}: {error}",
                    binding.display_path(&quarantine)
                )),
            ),
        };
    }
    require_generation(
        directory,
        &binding,
        binding.target_name(),
        &FileSnapshot::Missing,
    )?;
    remove_directory_contents(&quarantined, directory).with_context(|| {
        format!(
            "partially cleaned directory generation preserved at {}",
            binding.display_path(&quarantine)
        )
    })?;
    run_test_hook(TestHookPoint::RemoveTreeBeforeQuarantineDiscard);
    binding.require_current(directory)?;
    let current = open_directory_entry(binding.raw_fd(), &quarantine).map_err(|error| {
        anyhow::Error::from(FileConflict::new(directory)).context(format!(
            "could not verify quarantined directory tree {} before removal: {error}",
            directory.display()
        ))
    })?;
    if file_identity(&current.metadata()?) != identity {
        return Err(FileConflict::new(directory).into());
    }
    drop(current);
    if unsafe { libc::unlinkat(binding.raw_fd(), quarantine.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("removing directory tree {}", directory.display()));
    }
    binding.require_current(directory)?;
    require_generation(
        directory,
        &binding,
        binding.target_name(),
        &FileSnapshot::Missing,
    )?;
    binding.sync();
    Ok(true)
}

fn remove_directory_contents(directory: &std::fs::File, path: &Path) -> Result<()> {
    for name in directory_entry_names(directory.as_raw_fd())? {
        let child_path = path.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        match open_directory_entry(directory.as_raw_fd(), &name) {
            Ok(child) => {
                let identity = file_identity(&child.metadata()?);
                remove_directory_contents(&child, &child_path)?;
                let current =
                    open_directory_entry(directory.as_raw_fd(), &name).map_err(|error| {
                        anyhow::Error::from(FileConflict::new(&child_path)).context(format!(
                            "could not verify directory {} before removal: {error}",
                            child_path.display()
                        ))
                    })?;
                if file_identity(&current.metadata()?) != identity {
                    return Err(FileConflict::new(&child_path).into());
                }
                drop(current);
                if unsafe {
                    libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    return Err(std::io::Error::last_os_error())
                        .with_context(|| format!("removing directory {}", child_path.display()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ELOOP) | Some(libc::ENOTDIR)
                ) =>
            {
                if let Err(error) = unlink_entry(directory.as_raw_fd(), &name) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        return Err(error)
                            .with_context(|| format!("removing file {}", child_path.display()));
                    }
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("opening directory {}", child_path.display()))
            }
        }
    }
    directory.sync_all()?;
    Ok(())
}

fn open_directory_entry(directory_fd: RawFd, name: &CStr) -> std::io::Result<std::fs::File> {
    let fd = unsafe {
        libc::openat(
            directory_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }
}

fn directory_entry_names(directory_fd: RawFd) -> Result<Vec<CString>> {
    let duplicate = unsafe { libc::dup(directory_fd) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error()).context("duplicating directory descriptor");
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(duplicate);
        }
        return Err(error).context("opening directory stream");
    }
    struct DirectoryStream(*mut libc::DIR);
    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = DirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn directory_chain_is_current(paths: &[PathBuf], identities: &[FileIdentity]) -> Result<bool> {
    for (path, expected) in paths.iter().zip(identities) {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || file_identity(&metadata) != *expected
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn rollback_created_directories(
    directories: &[std::fs::File],
    components: &[CString],
    created: &[bool],
) {
    for index in (0..created.len()).rev() {
        if created[index] {
            let _ = unsafe {
                libc::unlinkat(
                    directories[index].as_raw_fd(),
                    components[index].as_ptr(),
                    libc::AT_REMOVEDIR,
                )
            };
        }
    }
}

#[cfg(unix)]
fn path_c_string(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path contains a null byte: {}", path.display()))
}

#[cfg(target_os = "macos")]
fn reject_extended_acl(file: &std::fs::File, path: &Path) -> Result<()> {
    use std::ffi::{c_int, c_void};

    unsafe extern "C" {
        fn acl_get_fd_np(fd: c_int, acl_type: c_int) -> *mut c_void;
        fn acl_free(value: *mut c_void) -> c_int;
    }

    const ACL_TYPE_EXTENDED: c_int = 0x100;
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
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
fn read_extended_attributes(file: &std::fs::File, path: &Path) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let size = unsafe { list_xattr(file.as_raw_fd(), std::ptr::null_mut(), 0) };
    if size < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("listing extended attributes on {}", path.display()));
    }

    let mut list = vec![0_u8; size as usize];
    if size > 0 {
        let read = unsafe { list_xattr(file.as_raw_fd(), list.as_mut_ptr().cast(), list.len()) };
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
                file.as_raw_fd(),
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
                    file.as_raw_fd(),
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
fn replace_extended_attributes(
    file: &std::fs::File,
    path: &Path,
    attributes: &[(Vec<u8>, Vec<u8>)],
) -> Result<()> {
    for (name, _) in read_extended_attributes(file, path)? {
        let name_string = std::ffi::CString::new(name).expect("xattr name has no null bytes");
        let result = unsafe { remove_xattr(file.as_raw_fd(), name_string.as_ptr()) };
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
                file.as_raw_fd(),
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
unsafe fn list_xattr(fd: RawFd, list: *mut libc::c_char, size: usize) -> isize {
    libc::flistxattr(fd, list, size, 0)
}

#[cfg(target_os = "linux")]
unsafe fn list_xattr(fd: RawFd, list: *mut libc::c_char, size: usize) -> isize {
    libc::flistxattr(fd, list, size)
}

#[cfg(target_os = "macos")]
unsafe fn get_xattr(
    fd: RawFd,
    name: *const libc::c_char,
    value: *mut libc::c_void,
    size: usize,
) -> isize {
    libc::fgetxattr(fd, name, value, size, 0, 0)
}

#[cfg(target_os = "linux")]
unsafe fn get_xattr(
    fd: RawFd,
    name: *const libc::c_char,
    value: *mut libc::c_void,
    size: usize,
) -> isize {
    libc::fgetxattr(fd, name, value, size)
}

#[cfg(target_os = "macos")]
unsafe fn set_xattr(
    fd: RawFd,
    name: *const libc::c_char,
    value: *const libc::c_void,
    size: usize,
) -> libc::c_int {
    libc::fsetxattr(fd, name, value, size, 0, 0)
}

#[cfg(target_os = "linux")]
unsafe fn set_xattr(
    fd: RawFd,
    name: *const libc::c_char,
    value: *const libc::c_void,
    size: usize,
) -> libc::c_int {
    libc::fsetxattr(fd, name, value, size, 0)
}

#[cfg(target_os = "macos")]
unsafe fn remove_xattr(fd: RawFd, name: *const libc::c_char) -> libc::c_int {
    libc::fremovexattr(fd, name, 0)
}

#[cfg(target_os = "linux")]
unsafe fn remove_xattr(fd: RawFd, name: *const libc::c_char) -> libc::c_int {
    libc::fremovexattr(fd, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptors_for_directory(path: &Path) -> usize {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::metadata(path).unwrap();
        (0..4096)
            .filter(|fd| {
                let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                let result = unsafe { libc::fstat(*fd, stat.as_mut_ptr()) };
                if result != 0 {
                    return false;
                }
                let stat = unsafe { stat.assume_init() };
                stat.st_dev as u128 == metadata.dev() as u128
                    && stat.st_ino as u128 == metadata.ino() as u128
            })
            .count()
    }

    fn assert_file_conflict(error: &anyhow::Error) {
        assert!(
            error_has_file_conflict(error),
            "expected FileConflict, got: {error:#}"
        );
    }

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
    fn applied_write_receipts_do_not_retain_directory_descriptors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let before = descriptors_for_directory(&root);
        let mut receipts = Vec::new();

        for index in 0..256 {
            let path = root.join(format!("file-{index}.txt"));
            receipts.push(
                FileSnapshot::Missing
                    .replace_beneath(&root, &path, b"content")
                    .unwrap(),
            );
        }

        let after = descriptors_for_directory(&root);
        assert!(after <= before + 1, "before={before}, after={after}");
        drop(receipts);
    }

    #[test]
    fn read_beneath_rejects_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        std::fs::hard_link(&path, root.join("alias.mdoc")).unwrap();

        let error = read_regular_file_beneath(&root, &path).unwrap_err();

        assert!(error.to_string().contains("hard-linked file"));
    }

    #[test]
    fn read_beneath_rejects_final_entry_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        let replacement = root.join("replacement.mdoc");
        std::fs::write(&replacement, "after").unwrap();
        let hook_path = path.clone();
        set_test_hook(TestHookPoint::ReadAfterContent, move || {
            std::fs::rename(&replacement, hook_path).unwrap();
        });

        let error = read_regular_file_beneath(&root, &path).unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "after");
    }

    #[test]
    fn read_beneath_rejects_in_place_write_after_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        let hook_path = path.clone();
        set_test_hook(TestHookPoint::ReadAfterContent, move || {
            std::fs::write(hook_path, "after").unwrap();
        });

        let error = read_regular_file_beneath(&root, &path).unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "after");
    }

    #[test]
    fn snapshot_batch_captures_existing_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let existing = nested.join("node.mdoc");
        std::fs::write(&existing, "content").unwrap();

        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        let snapshot = batch.capture(&existing).unwrap();
        let missing = batch.capture(&nested.join("missing.mdoc")).unwrap();
        let read = batch.capture_read(&existing).unwrap().unwrap();
        batch.finish().unwrap();

        assert_eq!(snapshot.content(), Some(b"content".as_slice()));
        assert!(matches!(missing, FileSnapshot::Missing));
        assert_eq!(read.content(), b"content");
    }

    #[test]
    fn snapshot_batch_read_treats_a_missing_parent_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("missing/nested/node.mdoc");

        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        let read = batch.capture_read(&path).unwrap();
        batch.finish().unwrap();

        assert!(read.is_none());
    }

    #[test]
    fn full_snapshot_matches_a_lightweight_read_generation() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("node.mdoc");
        std::fs::write(&path, "content").unwrap();
        let snapshot = FileSnapshot::capture(&path).unwrap();

        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        let current = batch.capture_read(&path).unwrap();
        batch.finish().unwrap();
        assert!(snapshot.matches_read(current.as_ref()));

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode ^ 0o100)).unwrap();
        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        let current = batch.capture_read(&path).unwrap();
        batch.finish().unwrap();
        assert!(!snapshot.matches_read(current.as_ref()));
    }

    #[test]
    fn snapshot_batch_read_rejects_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        symlink(outside.path(), root.join("linked")).unwrap();

        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        let error = batch
            .capture_read(&root.join("linked/node.mdoc"))
            .unwrap_err();

        assert_file_conflict(&error);
    }

    #[test]
    fn snapshot_batch_read_rejects_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("node.mdoc");
        std::fs::write(&path, "content").unwrap();
        std::fs::hard_link(&path, root.join("alias.bin")).unwrap();

        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        let error = batch.capture_read(&path).unwrap_err();

        assert!(error.to_string().contains("hard-linked file"));
    }

    #[test]
    fn snapshot_batch_finish_rejects_ancestor_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let parent = root.join("nested");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        batch.capture_read(&path).unwrap();

        std::fs::rename(&parent, root.join("detached")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("node.mdoc"), "after").unwrap();
        let error = batch.finish().unwrap_err();

        assert_file_conflict(&error);
    }

    #[test]
    fn snapshot_batch_read_rejects_final_entry_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("node.mdoc");
        let replacement = root.join("replacement.mdoc");
        std::fs::write(&path, "before").unwrap();
        std::fs::write(&replacement, "after").unwrap();
        let hook_path = path.clone();
        set_test_hook(TestHookPoint::ReadAfterContent, move || {
            std::fs::rename(replacement, hook_path).unwrap();
        });

        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        let error = batch.capture_read(&path).unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "after");
    }

    #[test]
    fn snapshot_batch_bounds_open_directory_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut paths = Vec::new();
        for index in 0..(SNAPSHOT_DIRECTORY_CACHE_SIZE * 3) {
            let parent = root.join(format!("dir-{index}"));
            std::fs::create_dir(&parent).unwrap();
            let path = parent.join("node.mdoc");
            std::fs::write(&path, "content").unwrap();
            paths.push(path);
        }

        let mut batch = FileSnapshotBatch::new(&root).unwrap();
        for path in paths {
            batch.capture_read(&path).unwrap();
            assert!(batch.directories.len() <= SNAPSHOT_DIRECTORY_CACHE_SIZE);
        }
        batch.finish().unwrap();
    }

    #[test]
    fn unchanged_propagates_snapshot_capture_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        let snapshot = FileSnapshot::capture(&path).unwrap();
        std::fs::hard_link(&path, dir.path().join("alias.mdoc")).unwrap();

        let error = snapshot.unchanged(&path).unwrap_err();

        assert!(error.to_string().contains("hard-linked file"));
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

    #[test]
    fn rollback_interleaving_after_initial_verification_preserves_external_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("generated.lean");
        let receipt = FileSnapshot::Missing.replace(&path, b"generated").unwrap();
        let edited_path = path.clone();
        set_test_hook(TestHookPoint::RollbackAfterInitialVerification, move || {
            std::fs::write(edited_path, b"external edit").unwrap();
        });

        let error = receipt.rollback().unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(std::fs::read(&path).unwrap(), b"external edit");
    }

    #[test]
    fn rollback_open_descriptor_interleaving_preserves_external_edit() {
        use std::io::{Seek, SeekFrom};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("generated.lean");
        let receipt = FileSnapshot::Missing.replace(&path, b"generated").unwrap();
        let mut editor = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        set_test_hook(
            TestHookPoint::RollbackAfterQuarantineVerification,
            move || {
                editor.set_len(0).unwrap();
                editor.seek(SeekFrom::Start(0)).unwrap();
                editor.write_all(b"descriptor edit").unwrap();
                editor.sync_all().unwrap();
            },
        );

        let error = receipt.rollback().unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(std::fs::read(&path).unwrap(), b"descriptor edit");
    }

    #[test]
    fn rollback_preserves_quarantine_when_parent_generation_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let parent = root.join("parent");
        let displaced = root.join("displaced");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("generated.lean");
        let receipt = FileSnapshot::Missing
            .replace_beneath(&root, &path, b"generated")
            .unwrap();
        let hook_parent = parent.clone();
        let hook_displaced = displaced.clone();
        set_test_hook(
            TestHookPoint::RollbackAfterQuarantineVerification,
            move || {
                std::fs::rename(&hook_parent, &hook_displaced).unwrap();
                std::fs::create_dir(&hook_parent).unwrap();
            },
        );

        let error = receipt.rollback().unwrap_err();

        assert_file_conflict(&error);
        let preserved = std::fs::read_dir(&displaced)
            .unwrap()
            .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(preserved, vec![b"generated".to_vec()]);
        assert!(std::fs::read_dir(&parent).unwrap().next().is_none());
    }

    #[test]
    fn directory_tree_cleanup_preserves_a_displaced_generation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let tree = root.join("build");
        let displaced = root.join("displaced-build");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("old.vo"), b"old generation").unwrap();
        let hook_tree = tree.clone();
        let hook_displaced = displaced.clone();
        set_test_hook(TestHookPoint::RemoveTreeAfterTargetOpen, move || {
            std::fs::rename(&hook_tree, &hook_displaced).unwrap();
            std::fs::create_dir(&hook_tree).unwrap();
            std::fs::write(hook_tree.join("new.vo"), b"new generation").unwrap();
        });

        let error = remove_directory_tree_beneath(&root, &tree).unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(
            std::fs::read(displaced.join("old.vo")).unwrap(),
            b"old generation"
        );
        assert_eq!(
            std::fs::read(tree.join("new.vo")).unwrap(),
            b"new generation"
        );
    }

    #[test]
    fn absent_directory_cleanup_rejects_a_replaced_parent_generation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let parent = root.join("compiler");
        let displaced = root.join("displaced-compiler");
        let tree = parent.join("build");
        std::fs::create_dir(&parent).unwrap();
        let hook_parent = parent.clone();
        let hook_tree = tree.clone();
        set_test_hook(TestHookPoint::RemoveTreeBeforeTargetOpen, move || {
            std::fs::rename(&hook_parent, displaced).unwrap();
            std::fs::create_dir(&hook_parent).unwrap();
            std::fs::create_dir(&hook_tree).unwrap();
            std::fs::write(hook_tree.join("new.vo"), b"new generation").unwrap();
        });

        let error = remove_directory_tree_beneath(&root, &tree).unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(
            std::fs::read(tree.join("new.vo")).unwrap(),
            b"new generation"
        );
    }

    #[test]
    fn directory_tree_cleanup_rejects_a_replacement_generation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let tree = root.join("build");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("old.vo"), b"old generation").unwrap();
        let hook_tree = tree.clone();
        set_test_hook(
            TestHookPoint::RemoveTreeBeforeQuarantineDiscard,
            move || {
                std::fs::create_dir(&hook_tree).unwrap();
                std::fs::write(hook_tree.join("new.vo"), b"new generation").unwrap();
            },
        );

        let error = remove_directory_tree_beneath(&root, &tree).unwrap_err();

        assert_file_conflict(&error);
        assert_eq!(
            std::fs::read(tree.join("new.vo")).unwrap(),
            b"new generation"
        );
    }

    #[test]
    fn ancestor_replacement_never_redirects_create_or_replace() {
        use std::os::unix::fs::symlink;

        for replacing in [false, true] {
            for point in [
                TestHookPoint::WriteBeforeDirectoryBinding,
                TestHookPoint::WriteAfterInitialVerification,
                TestHookPoint::WriteAfterTempCreation,
                TestHookPoint::WriteBeforePersistence,
                TestHookPoint::WriteAfterPersistence,
            ] {
                let dir = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let root = dir.path().canonicalize().unwrap();
                let ancestor = root.join("ancestor");
                let parent = ancestor.join("parent");
                std::fs::create_dir_all(&parent).unwrap();
                std::fs::create_dir(outside.path().join("parent")).unwrap();
                let path = parent.join("node.mdoc");
                let outside_path = outside.path().join("parent/node.mdoc");
                let displaced = root.join("displaced");
                let expected = if replacing {
                    std::fs::write(&path, b"before").unwrap();
                    FileSnapshot::capture(&path).unwrap()
                } else {
                    FileSnapshot::Missing
                };

                let hook_ancestor = ancestor.clone();
                let hook_outside = outside.path().to_path_buf();
                let hook_displaced = displaced.clone();
                set_test_hook(point, move || {
                    std::fs::rename(&hook_ancestor, &hook_displaced).unwrap();
                    symlink(&hook_outside, &hook_ancestor).unwrap();
                });

                let error = expected
                    .replace_beneath(&root, &path, b"after")
                    .unwrap_err();

                assert_file_conflict(&error);
                assert!(!outside_path.exists(), "outside write at {point:?}");
                let displaced_path = displaced.join("parent/node.mdoc");
                if replacing {
                    assert_eq!(std::fs::read(displaced_path).unwrap(), b"before");
                } else {
                    assert!(!displaced_path.exists());
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extended_attributes_survive_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.mdoc");
        std::fs::write(&path, "before").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut attributes = read_extended_attributes(&file, &path).unwrap();
        attributes.push((b"com.mathdoc.test".to_vec(), b"preserved".to_vec()));
        attributes.sort_by(|left, right| left.0.cmp(&right.0));
        replace_extended_attributes(&file, &path, &attributes).unwrap();
        drop(file);

        let snapshot = FileSnapshot::capture(&path).unwrap();
        atomic_replace(&path, &snapshot, b"after").unwrap();

        let file = std::fs::File::open(&path).unwrap();
        let attributes = read_extended_attributes(&file, &path).unwrap();
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
