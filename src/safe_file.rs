use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

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

#[derive(Clone)]
pub(crate) enum FileSnapshot {
    Missing,
    File {
        content: Vec<u8>,
        permissions: std::fs::Permissions,
        identity: FileIdentity,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    len: u64,
}

fn file_identity(meta: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            modified: meta.modified().ok(),
            len: meta.len(),
        }
    }
}

fn same_permissions(left: &std::fs::Permissions, right: &std::fs::Permissions) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        left.mode() == right.mode()
    }
    #[cfg(not(unix))]
    {
        left.readonly() == right.readonly()
    }
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
                permissions: meta.permissions(),
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
                    permissions,
                    identity,
                },
                Self::File {
                    content: current,
                    permissions: current_permissions,
                    identity: current_identity,
                },
            ) => {
                *content == current
                    && *identity == current_identity
                    && same_permissions(permissions, &current_permissions)
            }
            _ => false,
        })
    }
}

pub(crate) struct AppliedWrite {
    path: PathBuf,
    before: FileSnapshot,
    after: FileSnapshot,
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
                atomic_replace(&self.path, &current, &content)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn atomic_replace(
    path: &Path,
    expected: &FileSnapshot,
    content: &[u8],
) -> Result<AppliedWrite> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_regular_directory(parent)?;
    if !expected.unchanged(path)? {
        return Err(FileConflict::new(path).into());
    }
    if matches!(
        expected,
        FileSnapshot::File { permissions, .. } if permissions.readonly()
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
    if let FileSnapshot::File { permissions, .. } = expected {
        temp.as_file_mut().set_permissions(permissions.clone())?;
    }
    temp.write_all(content)
        .with_context(|| format!("writing temporary file for {}", path.display()))?;
    temp.as_file_mut()
        .sync_all()
        .with_context(|| format!("syncing temporary file for {}", path.display()))?;

    if !expected.unchanged(path)? {
        return Err(FileConflict::new(path).into());
    }
    match expected {
        FileSnapshot::Missing => temp.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::Error::from(FileConflict::new(path))
            } else {
                anyhow::Error::from(error.error)
            }
        })?,
        FileSnapshot::File { .. } => temp
            .persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("replacing {}", path.display()))?,
    };
    sync_parent(path);

    let after = FileSnapshot::capture(path)?;
    Ok(AppliedWrite {
        path: path.to_path_buf(),
        before: expected.clone(),
        after,
    })
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
