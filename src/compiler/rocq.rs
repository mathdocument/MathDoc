use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use super::{
    cfg_positive_int, lib_source, process_error_result, require_tool, run_process, CompilerReq,
    CompilerRes, SrcCompiler,
};

pub(super) struct CompilerRocq;

const INVENTORY_FILE: &str = ".mdc-module-inventory";

impl SrcCompiler for CompilerRocq {
    fn srctype(&self) -> &str {
        "rocq"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec =
            match cfg_positive_int(&req.compcfg, "timeout_sec", "src.rocq.timeout_sec") {
                Ok(v) => v,
                Err(e) => return CompilerRes::err(e.to_string()),
            };

        let rocq = match require_tool("rocq") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };

        let ws_root = req.mdcroot.join(".mdc").join("rocq");
        let (inventory, inventory_snapshot) = match ensure_workspace(&ws_root) {
            Ok(inventory) => inventory,
            Err(e) => return CompilerRes::err(e.to_string()),
        };

        let (_, relative) = match lib_source(req, "rocq") {
            Ok(source) => source,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let source = Path::new("Lib").join(relative);
        let output = Path::new("build")
            .join(source.strip_prefix("Lib").unwrap())
            .with_extension("vo");
        if let Err(error) = ensure_build_parent(&ws_root, &output) {
            return CompilerRes::err(error.to_string());
        }
        let args = vec![
            OsStr::new("compile").to_os_string(),
            OsStr::new("-Q").to_os_string(),
            OsStr::new("build").to_os_string(),
            std::ffi::OsString::new(),
            OsStr::new("-Q").to_os_string(),
            OsStr::new("Lib").to_os_string(),
            std::ffi::OsString::new(),
            OsStr::new("-noglob").to_os_string(),
            OsStr::new("-o").to_os_string(),
            output.as_os_str().to_os_string(),
            source.as_os_str().to_os_string(),
        ];

        match run_process(
            &rocq,
            args,
            &format!("rocq compile {}", source.display()),
            timeout_sec,
            Some(&ws_root),
        ) {
            Ok((rtcode, stdout, stderr)) => {
                if rtcode == 0 {
                    if let Err(error) =
                        record_module_inventory(&ws_root, &inventory, &inventory_snapshot)
                    {
                        return CompilerRes::err(error.to_string());
                    }
                }
                CompilerRes {
                    result: rtcode == 0,
                    stdout: stdout.trim().to_string(),
                    stderr: stderr.trim().to_string(),
                    rtcode,
                    interrupted: false,
                }
            }
            Err(e) => process_error_result(e, 1),
        }
    }
}

fn ensure_workspace(root: &Path) -> anyhow::Result<(Vec<u8>, crate::workspace::FileSnapshot)> {
    std::fs::create_dir_all(root)?;
    let project_path = root.join("_CoqProject");
    let snapshot = crate::workspace::FileSnapshot::capture(&project_path)?;
    if matches!(snapshot, crate::workspace::FileSnapshot::Missing)
        || snapshot.content() == Some(b"")
    {
        snapshot.replace(&project_path, b"-Q build \"\"\n-Q Lib \"\"\n")?;
    }
    let clean_marker = root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME);
    let clean_snapshot = crate::workspace::FileSnapshot::capture(&clean_marker)?;
    let inventory = source_tree_digest(&root.join("Lib"), "v")?;
    let inventory_path = root.join(INVENTORY_FILE);
    let inventory_snapshot = crate::workspace::FileSnapshot::capture(&inventory_path)?;
    let build = root.join("build");
    let inventory_changed =
        build.exists() && inventory_snapshot.content() != Some(inventory.as_slice());
    if !matches!(clean_snapshot, crate::workspace::FileSnapshot::Missing) || inventory_changed {
        remove_build_tree(&build)?;
        let _ = clean_snapshot.remove(&clean_marker)?;
    }
    Ok((inventory, inventory_snapshot))
}

fn remove_build_tree(build: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(build) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            anyhow::bail!(
                "refusing to clean non-directory Rocq build path {}",
                build.display()
            )
        }
        Ok(_) => std::fs::remove_dir_all(build)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn record_module_inventory(
    root: &Path,
    expected: &[u8],
    snapshot: &crate::workspace::FileSnapshot,
) -> anyhow::Result<()> {
    let current = source_tree_digest(&root.join("Lib"), "v")?;
    if current != expected {
        let marker = root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME);
        let marker_snapshot = crate::workspace::FileSnapshot::capture(&marker)?;
        if marker_snapshot.content() != Some(crate::compiler::ROCQ_CLEAN_MARKER_CONTENT) {
            marker_snapshot.replace(&marker, crate::compiler::ROCQ_CLEAN_MARKER_CONTENT)?;
        }
        anyhow::bail!("Rocq Lib source tree changed during compilation; retry the build");
    }
    let path = root.join(INVENTORY_FILE);
    if snapshot.content() != Some(expected) {
        snapshot.replace(&path, expected)?;
    }
    Ok(())
}

fn ensure_build_parent(root: &Path, output: &Path) -> anyhow::Result<()> {
    let mut current = root.to_path_buf();
    let Some(parent) = output.parent() else {
        return Ok(());
    };
    for component in parent.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("invalid Rocq build output path");
        };
        current.push(name);
        crate::workspace::ensure_regular_directory_exists(&current)?;
    }
    Ok(())
}

fn source_tree_digest(root: &Path, extension: &str) -> anyhow::Result<Vec<u8>> {
    let mut files = Vec::new();
    if !root.is_dir() {
        return Ok(format!("{:x}\n", Sha256::digest([])).into_bytes());
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!(
                "refusing symlink in compiler source tree: {}",
                entry.path().display()
            );
        }
        if !entry.file_type().is_file() || entry.path().extension() != Some(OsStr::new(extension)) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .expect("walk entry belongs to source root");
        files.push((
            relative.as_os_str().as_bytes().to_vec(),
            entry.path().to_path_buf(),
        ));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut digest = Sha256::new();
    for (relative, path) in files {
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(&relative);
        let content = std::fs::read(&path)
            .with_context(|| format!("reading compiler source {}", path.display()))?;
        digest.update((content.len() as u64).to_le_bytes());
        digest.update(content);
    }
    Ok(format!("{:x}\n", digest.finalize()).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lib_tree_change_cleans_stale_build_artifacts() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("rocq");
        std::fs::create_dir_all(root.join("build/Data")).unwrap();
        std::fs::write(root.join("build/Data/Stale.vo"), "stale").unwrap();
        std::fs::write(
            root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        ensure_workspace(&root).unwrap();

        assert!(!root.join("build").exists());
        assert!(!root
            .join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME)
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_coqproject_symlink_without_creating_target() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = workspace.path().join("rocq");
        std::fs::create_dir(&root).unwrap();
        let target = outside.path().join("created-outside");
        symlink(&target, root.join("_CoqProject")).unwrap();

        let error = ensure_workspace(&root).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert!(!target.exists());
        assert!(std::fs::symlink_metadata(root.join("_CoqProject"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_coqproject_symlink_to_existing_file() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "outside").unwrap();
        let root = workspace.path().join("rocq");
        std::fs::create_dir(&root).unwrap();
        symlink(outside.path(), root.join("_CoqProject")).unwrap();

        let error = ensure_workspace(&root).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert_eq!(std::fs::read_to_string(outside.path()).unwrap(), "outside");
    }
}
