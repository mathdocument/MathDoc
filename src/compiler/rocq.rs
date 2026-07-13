use std::path::Path;

use super::{
    cfg_positive_int, process_error_result, require_tool, run_process, CompilerReq, CompilerRes,
    SrcCompiler,
};

pub(super) struct CompilerRocq;

const SOURCE_FILE: &str = "MdcWork.v";

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
        if let Err(e) = ensure_workspace(&ws_root) {
            return CompilerRes::err(e.to_string());
        }

        match run_process(
            &rocq,
            ["compile", SOURCE_FILE],
            &format!("rocq compile {SOURCE_FILE}"),
            timeout_sec,
            Some(&ws_root),
        ) {
            Ok((rtcode, stdout, stderr)) => CompilerRes {
                result: rtcode == 0,
                stdout: stdout.trim().to_string(),
                stderr: stderr.trim().to_string(),
                rtcode,
                interrupted: false,
            },
            Err(e) => process_error_result(e, 1),
        }
    }
}

fn ensure_workspace(root: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(root)?;
    let project_path = root.join("_CoqProject");
    let snapshot = crate::workspace::FileSnapshot::capture(&project_path)?;
    if matches!(snapshot, crate::workspace::FileSnapshot::Missing) {
        crate::workspace::atomic_replace(&project_path, &snapshot, b"")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
