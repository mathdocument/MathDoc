mod latex;
mod lean;
mod process;
mod python;
mod rocq;
#[cfg(test)]
mod tests;

use anyhow::{bail, Context, Result};
use process::{
    ensure_complete_machine_output, process_error_result, require_tool, run_process,
    run_process_with_inherited_fd,
};
use std::path::{Path, PathBuf};

use crate::formal::FormalCompilationReceipt;

pub(crate) const ROCQ_CLEAN_MARKER_CONTENT: &[u8] = b"Lib tree changed\n";

// ── Compiler contract ─────────────────────────────────────────────────────────

pub(crate) type ProgressCallback = Box<dyn Fn(&str)>;

pub(crate) struct CompilerReq {
    pub(crate) mdcroot: PathBuf,
    pub(crate) source: PathBuf,
    /// Validated compiler settings with built-in defaults already applied.
    pub(crate) config: crate::config::SrcConfig,
    pub(crate) progress: Option<ProgressCallback>,
}

impl CompilerReq {
    fn timeout_sec(&self) -> Result<u64> {
        self.config
            .timeout_sec()
            .ok_or_else(|| anyhow::anyhow!("compiler request is missing timeout_sec"))
    }

    fn setup_timeout_sec(&self) -> Result<u64> {
        self.config
            .setup_timeout_sec()
            .ok_or_else(|| anyhow::anyhow!("compiler request is missing setup_timeout_sec"))
    }

    fn emit_progress(&self, message: &str) {
        if let Some(progress) = &self.progress {
            progress(message);
        }
    }
}

pub(crate) struct CompilerRes {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) rtcode: i32,
    pub(crate) interrupted: bool,
}

impl CompilerRes {
    pub(crate) fn err(stderr: impl Into<String>) -> Self {
        CompilerRes {
            stdout: String::new(),
            stderr: stderr.into(),
            rtcode: 1,
            interrupted: false,
        }
    }

    fn err_code(stderr: impl Into<String>, rtcode: i32) -> Self {
        CompilerRes {
            stdout: String::new(),
            stderr: stderr.into(),
            rtcode,
            interrupted: false,
        }
    }

    fn ok(stdout: impl Into<String>) -> Self {
        CompilerRes {
            stdout: stdout.into(),
            stderr: String::new(),
            rtcode: 0,
            interrupted: false,
        }
    }

    pub(crate) fn is_success(&self) -> bool {
        self.rtcode == 0
    }
}

pub(crate) fn compile_with_receipt(
    work_lock: &crate::workspace::WorkspaceWorkLock,
    srctype: &str,
    req: &CompilerReq,
) -> (CompilerRes, Option<FormalCompilationReceipt>) {
    if let Err(error) = work_lock.validate_root(&req.mdcroot) {
        return (CompilerRes::err(format!("{error:#}")), None);
    }
    let result = match crate::config::canonical_srctype(srctype) {
        "text" => (CompilerRes::ok(""), None),
        "python" => (python::compile(req), None),
        "latex" => (latex::compile(req), None),
        "lean" => lean::compile(req),
        "rocq" => rocq::compile(req),
        _ => (
            CompilerRes::err(format!("unknown srctype: {srctype}")),
            None,
        ),
    };
    if let Err(error) = work_lock.require_current() {
        return (CompilerRes::err(format!("{error:#}")), None);
    }
    result
}

// ── Shared helpers ────────────────────────────────────────────────────────────

#[derive(Debug)]
struct CompilerWorkspace {
    mdcroot: PathBuf,
    root: PathBuf,
    srctype: String,
    root_generation: crate::workspace::DirectoryGeneration,
}

impl CompilerWorkspace {
    fn open(req: &CompilerReq, srctype: &str) -> Result<Self> {
        let mdcroot = crate::workspace::validate_mdcroot(&req.mdcroot)?;
        let root = mdcroot.join(".mdc").join(srctype);
        crate::workspace::ensure_regular_directory_tree(&mdcroot, &root)?;
        let root_generation = crate::workspace::DirectoryGeneration::open_beneath(&mdcroot, &root)?;
        Ok(Self {
            mdcroot,
            root,
            srctype: srctype.to_string(),
            root_generation,
        })
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn mdcroot(&self) -> &Path {
        &self.mdcroot
    }

    fn process_cwd(&self) -> Result<&crate::workspace::DirectoryGeneration> {
        self.root_generation.require_current()?;
        Ok(&self.root_generation)
    }

    fn process_cwd_beneath(&self, path: &Path) -> Result<crate::workspace::DirectoryGeneration> {
        self.require_workspace_path(path)?;
        self.root_generation.open_descendant(&self.mdcroot, path)
    }

    fn lib_root(&self) -> PathBuf {
        self.root.join("Lib")
    }

    fn snapshot(&self, path: &Path) -> Result<crate::workspace::FileSnapshot> {
        self.require_workspace_path(path)?;
        self.root_generation.require_current()?;
        let snapshot = crate::workspace::FileSnapshot::capture_beneath(&self.mdcroot, path)?;
        self.root_generation.require_current()?;
        Ok(snapshot)
    }

    fn snapshot_unchanged(
        &self,
        snapshot: &crate::workspace::FileSnapshot,
        path: &Path,
    ) -> Result<bool> {
        self.require_workspace_path(path)?;
        self.root_generation.require_current()?;
        let unchanged = snapshot.unchanged_beneath(&self.mdcroot, path)?;
        self.root_generation.require_current()?;
        Ok(unchanged)
    }

    fn file_digest(&self, path: &Path) -> Result<String> {
        let snapshot = self.snapshot(path)?;
        let content = snapshot.content().ok_or_else(|| {
            anyhow::anyhow!("formal compiler output is missing: {}", path.display())
        })?;
        let digest = crate::formal::status::content_digest(content);
        if !self.snapshot_unchanged(&snapshot, path)? {
            bail!(
                "formal compiler output changed while reading: {}",
                path.display()
            );
        }
        Ok(digest)
    }

    fn ensure_directory_tree(&self, directory: &Path) -> Result<bool> {
        self.require_workspace_path(directory)?;
        self.root_generation.require_current()?;
        let changed = crate::workspace::ensure_regular_directory_tree(&self.mdcroot, directory)?;
        self.root_generation.require_current()?;
        Ok(changed)
    }

    fn remove_directory_tree(&self, directory: &Path) -> Result<bool> {
        self.require_workspace_path(directory)?;
        self.root_generation.require_current()?;
        let changed = crate::workspace::remove_directory_tree_beneath(&self.mdcroot, directory)?;
        self.root_generation.require_current()?;
        Ok(changed)
    }

    fn remove_file(
        &self,
        path: &Path,
        snapshot: &crate::workspace::FileSnapshot,
    ) -> Result<Option<crate::workspace::AppliedWrite>> {
        self.require_workspace_path(path)?;
        self.root_generation.require_current()?;
        let applied = snapshot.remove_beneath(&self.mdcroot, path)?;
        self.root_generation.require_current()?;
        Ok(applied)
    }

    fn lib_source(&self, req: &CompilerReq) -> Result<(PathBuf, PathBuf)> {
        self.root_generation.require_current()?;
        let lib_root = self.lib_root();
        let source = absolute_path(&req.source)?;
        let requested_root = absolute_path(&req.mdcroot)?;
        let requested_lib_root = requested_root.join(".mdc").join(&self.srctype).join("Lib");
        let source = if source.starts_with(&lib_root) {
            source
        } else if let Ok(relative) = source.strip_prefix(&requested_lib_root) {
            lib_root.join(relative)
        } else {
            source
        };
        let relative = source
            .strip_prefix(&lib_root)
            .with_context(|| format!("source is outside {}", lib_root.display()))?
            .to_path_buf();
        if relative.as_os_str().is_empty() {
            bail!("compiler source cannot be the Lib source directory");
        }
        crate::workspace::ensure_regular_file_beneath(&self.mdcroot, &source)
            .with_context(|| format!("validating compiler source {}", source.display()))?;
        self.root_generation.require_current()?;
        Ok((lib_root, relative))
    }

    fn replace_generated(
        &self,
        path: &Path,
        snapshot: &crate::workspace::FileSnapshot,
        content: &[u8],
    ) -> Result<crate::workspace::AppliedWrite> {
        self.require_workspace_path(path)?;
        self.root_generation.require_current()?;
        let applied = snapshot.replace_beneath(&self.mdcroot, path, content)?;
        self.root_generation.require_current()?;
        Ok(applied)
    }

    fn require_workspace_path(&self, path: &Path) -> Result<()> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            anyhow::anyhow!(
                "compiler path {} is outside workspace {}",
                path.display(),
                self.root.display()
            )
        })?;
        if relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            bail!("refusing non-normalized compiler path {}", path.display());
        }
        Ok(())
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
