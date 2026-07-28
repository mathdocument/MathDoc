use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use super::{
    process_error_result, require_tool, run_process, CompilerReq, CompilerRes, CompilerWorkspace,
    SrcCompiler,
};

pub(super) struct CompilerRocq;

const INVENTORY_FILE: &str = ".mdc-module-inventory";

impl SrcCompiler for CompilerRocq {
    fn srctype(&self) -> &str {
        "rocq"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec = req.timeout_sec();

        let rocq = match require_tool("rocq") {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };

        let workspace = match CompilerWorkspace::open(req, "rocq") {
            Ok(workspace) => workspace,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let (_, relative) = match workspace.lib_source(req) {
            Ok(source) => source,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let (inventory, inventory_snapshot) = match ensure_workspace(&workspace) {
            Ok(inventory) => inventory,
            Err(e) => return CompilerRes::err(e.to_string()),
        };
        let source = Path::new("Lib").join(relative);
        let output = Path::new("build")
            .join(source.strip_prefix("Lib").unwrap())
            .with_extension("vo");
        if let Err(error) = ensure_build_parent(&workspace, &output) {
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
            Some(workspace.root()),
        ) {
            Ok((rtcode, stdout, stderr)) => {
                if rtcode == 0 {
                    if let Err(error) =
                        record_module_inventory(&workspace, &inventory, &inventory_snapshot)
                    {
                        return CompilerRes::err(error.to_string());
                    }
                }
                CompilerRes {
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

fn ensure_workspace(
    workspace: &CompilerWorkspace,
) -> anyhow::Result<(Vec<u8>, crate::workspace::FileSnapshot)> {
    let root = workspace.root();
    let project_path = root.join("_CoqProject");
    let snapshot = workspace.snapshot(&project_path)?;
    if matches!(snapshot, crate::workspace::FileSnapshot::Missing)
        || snapshot.content() == Some(b"")
    {
        workspace.replace_generated(&project_path, &snapshot, b"-Q build \"\"\n-Q Lib \"\"\n")?;
    }
    let clean_marker = root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME);
    let clean_snapshot = workspace.snapshot(&clean_marker)?;
    let inventory = source_tree_digest(&root.join("Lib"), "v")?;
    let inventory_path = root.join(INVENTORY_FILE);
    let inventory_snapshot = workspace.snapshot(&inventory_path)?;
    let build = root.join("build");
    let build_exists = match std::fs::symlink_metadata(&build) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let inventory_changed =
        build_exists && inventory_snapshot.content() != Some(inventory.as_slice());
    if !matches!(clean_snapshot, crate::workspace::FileSnapshot::Missing) || inventory_changed {
        workspace.remove_directory_tree(&build)?;
        let _ = workspace.remove_file(&clean_marker, &clean_snapshot)?;
    }
    Ok((inventory, inventory_snapshot))
}

fn record_module_inventory(
    workspace: &CompilerWorkspace,
    expected: &[u8],
    snapshot: &crate::workspace::FileSnapshot,
) -> anyhow::Result<()> {
    let root = workspace.root();
    let current = source_tree_digest(&root.join("Lib"), "v")?;
    if current != expected {
        let marker = root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME);
        let marker_snapshot = workspace.snapshot(&marker)?;
        if marker_snapshot.content() != Some(crate::compiler::ROCQ_CLEAN_MARKER_CONTENT) {
            workspace.replace_generated(
                &marker,
                &marker_snapshot,
                crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
            )?;
        }
        anyhow::bail!("Rocq Lib source tree changed during compilation; retry the build");
    }
    let path = root.join(INVENTORY_FILE);
    if snapshot.content() != Some(expected) {
        workspace.replace_generated(&path, snapshot, expected)?;
    }
    Ok(())
}

fn ensure_build_parent(workspace: &CompilerWorkspace, output: &Path) -> anyhow::Result<()> {
    let mut current = workspace.root().to_path_buf();
    let Some(parent) = output.parent() else {
        return Ok(());
    };
    for component in parent.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("invalid Rocq build output path");
        };
        current.push(name);
    }
    workspace.ensure_directory_tree(&current)?;
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

    fn test_workspace(directory: &tempfile::TempDir) -> CompilerWorkspace {
        let mdcroot = directory.path().canonicalize().unwrap();
        let root = mdcroot.join(".mdc/rocq");
        std::fs::create_dir_all(&root).unwrap();
        CompilerWorkspace {
            mdcroot,
            root,
            srctype: "rocq".to_string(),
        }
    }

    #[test]
    fn clean_marker_removes_stale_build_artifacts() {
        let workspace = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::create_dir_all(root.join("build/Data")).unwrap();
        std::fs::write(root.join("build/Data/Stale.vo"), "stale").unwrap();
        std::fs::write(
            root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        ensure_workspace(&compiler_workspace).unwrap();

        assert!(!root.join("build").exists());
        assert!(!root
            .join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME)
            .exists());
    }

    #[test]
    fn inventory_change_removes_stale_build_artifacts_without_sync_marker() {
        let workspace = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::create_dir_all(root.join("Lib")).unwrap();
        std::fs::write(root.join("Lib/Data.v"), "Definition value := 1.\n").unwrap();
        let inventory = source_tree_digest(&root.join("Lib"), "v").unwrap();
        std::fs::write(root.join(INVENTORY_FILE), inventory).unwrap();
        std::fs::create_dir_all(root.join("build/Data")).unwrap();
        std::fs::write(root.join("build/Data/Stale.vo"), "stale").unwrap();
        std::fs::write(root.join("Lib/Data.v"), "Definition value := 2.\n").unwrap();

        ensure_workspace(&compiler_workspace).unwrap();

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
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        let target = outside.path().join("created-outside");
        symlink(&target, root.join("_CoqProject")).unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();
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
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        symlink(outside.path(), root.join("_CoqProject")).unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert_eq!(std::fs::read_to_string(outside.path()).unwrap(), "outside");
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_rejects_symlinked_build_tree() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::write(outside.path().join("keep.vo"), "outside").unwrap();
        symlink(outside.path(), root.join("build")).unwrap();
        std::fs::write(
            root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();

        assert!(error.to_string().contains("non-directory tree"));
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep.vo")).unwrap(),
            "outside"
        );
    }

    #[test]
    fn cleanup_rejects_regular_file_build_path() {
        let workspace = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::write(root.join("build"), "not a directory").unwrap();
        std::fs::write(
            root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        let error = ensure_workspace(&compiler_workspace).unwrap_err();

        assert!(error.to_string().contains("non-directory tree"));
        assert_eq!(
            std::fs::read_to_string(root.join("build")).unwrap(),
            "not a directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_unlinks_nested_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let compiler_workspace = test_workspace(&workspace);
        let root = compiler_workspace.root();
        std::fs::create_dir_all(root.join("build/Data")).unwrap();
        std::fs::write(outside.path().join("keep.vo"), "outside").unwrap();
        symlink(outside.path(), root.join("build/Data/link")).unwrap();
        std::fs::write(
            root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME),
            crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
        )
        .unwrap();

        ensure_workspace(&compiler_workspace).unwrap();

        assert!(!root.join("build").exists());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep.vo")).unwrap(),
            "outside"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_cannot_follow_replaced_workspace_ancestor() {
        use std::os::unix::fs::symlink;

        for point in [
            crate::workspace::TestHookPoint::RemoveTreeBeforeDirectoryBinding,
            crate::workspace::TestHookPoint::RemoveTreeAfterDirectoryBinding,
        ] {
            let workspace = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let compiler_workspace = test_workspace(&workspace);
            let root = compiler_workspace.root().to_path_buf();
            std::fs::create_dir_all(root.join("build/Data")).unwrap();
            std::fs::write(root.join("build/Data/stale.vo"), "stale").unwrap();
            std::fs::write(
                root.join(crate::compiler::ROCQ_CLEAN_MARKER_FILENAME),
                crate::compiler::ROCQ_CLEAN_MARKER_CONTENT,
            )
            .unwrap();
            std::fs::create_dir_all(outside.path().join("build/Data")).unwrap();
            std::fs::write(outside.path().join("build/Data/outside.vo"), "outside").unwrap();
            let displaced = compiler_workspace.mdcroot.join(".mdc/displaced-rocq");
            let hook_root = root.clone();
            let hook_displaced = displaced.clone();
            let hook_outside = outside.path().to_path_buf();
            crate::workspace::set_test_hook(point, move || {
                std::fs::rename(hook_root, hook_displaced).unwrap();
                symlink(hook_outside, root).unwrap();
            });

            let error = ensure_workspace(&compiler_workspace).unwrap_err();

            assert!(crate::workspace::error_has_file_conflict(&error));
            assert_eq!(
                std::fs::read_to_string(outside.path().join("build/Data/outside.vo")).unwrap(),
                "outside",
                "outside tree changed at {point:?}"
            );
            assert_eq!(
                std::fs::read_to_string(displaced.join("build/Data/stale.vo")).unwrap(),
                "stale",
                "displaced tree changed at {point:?}"
            );
        }
    }
}
