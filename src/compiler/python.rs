use super::{
    process_error_result, require_tool, run_process_with_inherited_fd, CompilerReq, CompilerRes,
    CompilerWorkspace, SrcCompiler,
};
use std::io::Write;
use std::os::fd::AsRawFd;

pub(super) struct CompilerPython;

const EXEC_CAPTURED_SOURCE: &str = r#"import importlib.machinery, os, sys
source_name = sys.argv[1]
source_fd = int(sys.argv[2])
source_path = os.path.abspath(source_name)
sys.argv = [source_name]
sys.orig_argv = [sys.executable, "-B", "--", source_name]
sys.path[0] = os.path.dirname(source_path)
os.lseek(source_fd, 0, os.SEEK_SET)
with os.fdopen(source_fd, "rb", closefd=True) as source:
    code = source.read()
main = sys.modules["__main__"]
main.__file__ = source_path
main.__cached__ = None
main.__loader__ = importlib.machinery.SourceFileLoader("__main__", source_path)
main.__package__ = None
main.__spec__ = None
exec(compile(code, source_path, "exec"), main.__dict__, main.__dict__)
"#;

impl SrcCompiler for CompilerPython {
    fn srctype(&self) -> &str {
        "python"
    }

    fn compile(&self, req: &CompilerReq) -> CompilerRes {
        let timeout_sec = match req.timeout_sec() {
            Ok(timeout) => timeout,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let python = match require_tool("python3").or_else(|_| require_tool("python")) {
            Ok(p) => p,
            Err(e) => return CompilerRes::err_code(e.to_string(), 127),
        };
        let workspace = match CompilerWorkspace::open(req, "python") {
            Ok(workspace) => workspace,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let (lib_root, relative) = match workspace.lib_source(req) {
            Ok(source) => source,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let cwd_directory = match workspace.process_cwd_beneath(&lib_root) {
            Ok(directory) => directory,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let source_path = lib_root.join(&relative);
        let source_snapshot = match workspace.snapshot(&source_path) {
            Ok(snapshot) if snapshot.content().is_some() => snapshot,
            Ok(_) => return CompilerRes::err("Python source disappeared before execution"),
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        if let Err(error) = cwd_directory.require_current() {
            return CompilerRes::err(format!(
                "Python working tree changed before execution: {error}"
            ));
        }
        let mut captured_source = match tempfile::NamedTempFile::new() {
            Ok(file) => file,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        if let Err(error) =
            captured_source.write_all(source_snapshot.content().expect("source presence checked"))
        {
            return CompilerRes::err(format!("preparing captured Python source: {error}"));
        }
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let expected = match captured_source.as_file().metadata() {
            Ok(metadata) => metadata,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let execution_source = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(captured_source.path())
        {
            Ok(file) => file,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        let opened = match execution_source.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return CompilerRes::err(error.to_string()),
        };
        if !opened.is_file() || (expected.dev(), expected.ino()) != (opened.dev(), opened.ino()) {
            return CompilerRes::err("captured Python source changed while it was opened");
        }
        if let Err(error) = captured_source.close() {
            return CompilerRes::err(format!("unlinking captured Python source: {error}"));
        }
        let source_fd = execution_source.as_raw_fd().to_string();
        let result = match run_process_with_inherited_fd(
            &python,
            [
                std::ffi::OsStr::new("-B"),
                std::ffi::OsStr::new("-c"),
                std::ffi::OsStr::new(EXEC_CAPTURED_SOURCE),
                relative.as_os_str(),
                std::ffi::OsStr::new(&source_fd),
            ],
            "python",
            timeout_sec,
            &cwd_directory,
            execution_source.as_raw_fd(),
        ) {
            Ok((rtcode, stdout, stderr)) => CompilerRes {
                stdout,
                stderr,
                rtcode,
                interrupted: false,
            },
            Err(e) => process_error_result(e, 127),
        };
        match workspace.snapshot_unchanged(&source_snapshot, &source_path) {
            Ok(true) => result,
            Ok(false) => generation_error(result, "Python source changed during execution"),
            Err(error) => generation_error(result, &error.to_string()),
        }
    }
}

fn generation_error(mut result: CompilerRes, message: &str) -> CompilerRes {
    if result.is_success() {
        return CompilerRes::err(message);
    }
    if !result.stderr.is_empty() {
        result.stderr.push('\n');
    }
    result.stderr.push_str(message);
    result
}
