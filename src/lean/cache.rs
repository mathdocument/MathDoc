//! Database-scoped native Lake objects. Workspaces retain only mutable state.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

pub struct Pool {
    pub root: PathBuf,
    // ponytail: GC requires stopped branches; use per-closure pins if live GC becomes necessary.
    _lease: Lease,
}

pub struct Lease(Option<fs::File>);
impl Drop for Lease {
    fn drop(&mut self) {
        // Release explicitly: another thread may have forked just before close.
        if let Some(file) = &self.0 {
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

pub fn lease(root: &Path, exclusive: bool) -> Result<Lease> {
    fs::create_dir_all(root)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("lease.lock"))?;
    let mode = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error).context("lock Lean shared cache");
        }
        bail!("Lean shared cache is in use; stop this database's branches before cleaning it");
    }
    Ok(Lease(Some(file)))
}

impl Pool {
    pub fn open(root: PathBuf) -> Result<Self> {
        let lease = lease(&root, false)?;
        Ok(Self {
            root,
            _lease: lease,
        })
    }

    pub fn project(&self, key: &str) -> PathBuf {
        self.root
            .join("v1")
            .join(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .join(key)
    }

    pub async fn attach(&self, workspace: &Path, project_key: &str) -> Result<()> {
        let destination = self.project(project_key).join("lake");
        tokio::fs::create_dir_all(&destination).await?;
        let destination = tokio::fs::canonicalize(destination).await?;
        let local = workspace.join(".lake/cache");
        tokio::fs::create_dir_all(local.parent().unwrap()).await?;
        let _lock = lock(local.with_extension("lock")).await?;
        if local.is_symlink() {
            if tokio::fs::canonicalize(&local).await? == destination {
                return Ok(());
            }
            bail!(
                "workspace points to a different Lean cache: {}",
                local.display()
            );
        }
        // Upgrade a stopped branch's old native cache once, without copying object bytes.
        if local.is_dir() {
            let from = local.clone();
            let to = destination.clone();
            tokio::task::spawn_blocking(move || import_cache(&from, &to)).await??;
            tokio::fs::remove_dir_all(&local).await?;
        }
        tokio::fs::symlink(destination, local).await?;
        Ok(())
    }
}

/// A native child retains its own lease even if mdc crashes or cancels its task.
/// Only clear CLOEXEC in the child, so unrelated concurrent spawns cannot inherit it.
pub(super) fn inherit_lease(command: &mut tokio::process::Command, workspace: &Path) -> Result<()> {
    let local = workspace.join(".lake/cache");
    if !local.is_symlink() {
        return Ok(());
    }
    let target = fs::canonicalize(local)?;
    let ancestors: Vec<_> = target.ancestors().take(5).collect();
    if ancestors.len() != 5
        || target.file_name().is_none_or(|n| n != "lake")
        || ancestors[3].file_name().is_none_or(|n| n != "v1")
    {
        return Ok(());
    }
    // This particular file description must stay locked until the final native
    // child closes it, rather than explicitly unlocking when Command is dropped.
    let lease = lease(ancestors[4], false)?.0.take().unwrap();
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(lease.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

/// Cancellable, cross-process single-flight without blocking a Tokio worker.
pub async fn lock(path: PathBuf) -> Result<fs::File> {
    tokio::fs::create_dir_all(path.parent().context("lock needs a parent")?).await?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn import_cache(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            import_cache(&entry.path(), &target)?;
        } else if entry.file_type()?.is_file() && !target.exists() {
            // The source is an inactive, native cache object, never a mutable source file.
            match fs::hard_link(entry.path(), &target) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(_) => {
                    let tmp = tempfile::NamedTempFile::new_in(to)?;
                    fs::copy(entry.path(), tmp.path())?;
                    match tmp.persist_noclobber(&target) {
                        Ok(_) => (),
                        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => (),
                        Err(e) => return Err(e.error.into()),
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Artifacts {
    pub parts: Vec<String>,
    pub ilean: String,
    pub files: Vec<String>,
}
impl Artifacts {
    pub fn from_trace(root: &Path, module: &str) -> Option<Self> {
        let trace = root
            .join(".lake/build/lib/lean")
            .join(crate::store::module_file(module, "trace").ok()?);
        let data: Value = serde_json::from_slice(&fs::read(trace).ok()?).ok()?;
        Self::from_outputs(&data["outputs"])
    }
    fn from_outputs(data: &Value) -> Option<Self> {
        let parts: Vec<String> = serde_json::from_value(data["o"].clone()).ok()?;
        let ilean = data["i"].as_str()?.to_owned();
        let mut files = vec![];
        object_names(data, &mut files);
        if parts.is_empty() || files.iter().any(|s| !valid_object(s)) {
            return None;
        }
        Some(Self {
            parts,
            ilean,
            files,
        })
    }
    pub fn complete(&self, cache: &Path) -> bool {
        !self.parts.is_empty()
            && !self.files.is_empty()
            && self
                .parts
                .iter()
                .chain(std::iter::once(&self.ilean))
                .all(|s| valid_object(s) && self.files.contains(s))
            && self
                .files
                .iter()
                .all(|s| valid_object(s) && cache.join("artifacts").join(s).is_file())
    }
    pub fn parts(&self, cache: &Path) -> Vec<PathBuf> {
        self.parts
            .iter()
            .map(|s| cache.join("artifacts").join(s))
            .collect()
    }
}
pub(super) fn object_names(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::String(s) => paths.push(s.clone()),
        Value::Array(values) => values.iter().for_each(|v| object_names(v, paths)),
        Value::Object(values) => values.values().for_each(|v| object_names(v, paths)),
        _ => (),
    }
}
pub(super) fn valid_object(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shared_objects_survive_workspace_removal_and_protect_gc() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join(".shared");
        let a = Pool::open(shared.clone()).unwrap();
        let b = Pool::open(shared.clone()).unwrap();
        let workspace = tmp.path().join("a");
        fs::create_dir_all(workspace.join(".lake/cache/artifacts")).unwrap();
        fs::write(
            workspace.join(".lake/cache/artifacts/123.olean"),
            b"native object",
        )
        .unwrap();
        a.attach(&workspace, "project").await.unwrap();
        b.attach(&tmp.path().join("b"), "project").await.unwrap();
        fs::remove_dir_all(workspace).unwrap();
        assert_eq!(
            fs::read(tmp.path().join("b/.lake/cache/artifacts/123.olean")).unwrap(),
            b"native object"
        );
        assert!(lease(&shared, true).is_err());
        drop(a);
        drop(b);
        assert!(lease(&shared, true).is_ok());
        assert!(
            Artifacts::from_outputs(&serde_json::json!({"o":["../outside"],"i":"a.ilean"}))
                .is_none()
        );
    }

    #[tokio::test]
    async fn native_child_keeps_gc_blocked_after_the_service_lease_is_dropped() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("shared");
        let pool = Pool::open(root.clone()).unwrap();
        let workspace = tmp.path().join("workspace");
        pool.attach(&workspace, "project").await.unwrap();
        let mut command = tokio::process::Command::new("sh");
        command
            .args(["-c", "echo ready; read stop"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        inherit_lease(&mut command, &workspace).unwrap();
        let mut child = command.spawn().unwrap();
        drop(command);
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .await
            .unwrap();
        assert_eq!(line.trim(), "ready");
        drop(pool);
        assert!(
            lease(&root, true).is_err(),
            "the child, not mdc, must retain the lock"
        );
        drop(child.stdin.take());
        child.wait().await.unwrap();
        assert!(lease(&root, true).is_ok());
    }
}
