use super::*;

fn compile(srctype: &str, req: &CompilerReq) -> CompilerRes {
    compile_receipt(srctype, req).0
}

fn compile_receipt(
    srctype: &str,
    req: &CompilerReq,
) -> (CompilerRes, Option<FormalCompilationReceipt>) {
    let work_lock = crate::workspace::WorkspaceWorkLock::acquire(&req.mdcroot).unwrap();
    compile_with_receipt(&work_lock, srctype, req)
}

fn make_req(mdcroot: &std::path::Path, srctype: &str) -> CompilerReq {
    CompilerReq {
        mdcroot: mdcroot.to_path_buf(),
        source: test_source_path(mdcroot, srctype),
        config: crate::config::default_for_srctype(srctype),
        progress: None,
    }
}

fn test_source_path(mdcroot: &std::path::Path, srctype: &str) -> std::path::PathBuf {
    mdcroot
        .join(".mdc")
        .join(srctype)
        .join("Lib")
        .join("node")
        .with_extension(crate::config::srctype_ext(srctype))
}

fn write_source(mdcroot: &std::path::Path, srctype: &str, content: &str) {
    let path = test_source_path(mdcroot, srctype);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn compiler_workspace_rejects_symlinked_language_directory() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join(".mdc")).unwrap();
    symlink(outside.path(), workspace.path().join(".mdc/latex")).unwrap();
    let req = make_req(workspace.path(), "latex");

    let error = CompilerWorkspace::open(&req, "latex").unwrap_err();

    assert!(error.to_string().contains("directory tree"));
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[test]
fn lib_source_rejects_symlinked_lib_component() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = workspace.path();
    std::fs::create_dir_all(root.join(".mdc/lean")).unwrap();
    std::fs::write(outside.path().join("node.lean"), "#check Nat\n").unwrap();
    symlink(outside.path(), root.join(".mdc/lean/Lib")).unwrap();
    let req = make_req(root, "lean");
    let compiler_workspace = CompilerWorkspace::open(&req, "lean").unwrap();

    let error = compiler_workspace.lib_source(&req).unwrap_err();

    assert!(error.to_string().contains("validating compiler source"));
    assert_eq!(
        std::fs::read_to_string(outside.path().join("node.lean")).unwrap(),
        "#check Nat\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_python_accepts_non_utf8_workspace_path() {
    use std::os::unix::ffi::OsStringExt;

    if which::which("python3").is_err() && which::which("python").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp
        .path()
        .join(std::ffi::OsString::from_vec(b"workspace-\xff".to_vec()));
    std::fs::create_dir_all(root.join(".mdc")).unwrap();
    write_source(
        &root,
        "python",
        "import pathlib\npathlib.Path('cwd-marker').write_text('ok')\n",
    );

    let req = make_req(&root, "python");
    let result = compile("python", &req);
    assert!(
        result.is_success(),
        "Python compilation failed: {}",
        result.stderr
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/python/Lib/cwd-marker")).unwrap(),
        "ok"
    );
}

#[test]
fn test_python_uses_deterministic_working_directory() {
    if which::which("python3").is_err() && which::which("python").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    write_source(
        root,
        "python",
        "import pathlib\npathlib.Path('cwd-marker').write_text('ok')\n",
    );

    let req = make_req(root, "python");
    let result = compile("python", &req);
    assert!(
        result.is_success(),
        "Python compilation failed: {}",
        result.stderr
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/python/Lib/cwd-marker")).unwrap(),
        "ok"
    );
}

#[test]
fn test_python_treats_leading_hyphen_source_as_a_file() {
    if which::which("python3").is_err() && which::which("python").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let lib = root.join(".mdc/python/Lib");
    std::fs::create_dir_all(&lib).unwrap();
    let source = lib.join("-c.py");
    std::fs::write(
        &source,
        "import pathlib\npathlib.Path('hyphen-marker').write_text('ok')\n",
    )
    .unwrap();
    let mut req = make_req(root, "python");
    req.source = source;

    let result = compile("python", &req);

    assert!(result.is_success(), "{}", result.stderr);
    assert_eq!(
        std::fs::read_to_string(lib.join("hyphen-marker")).unwrap(),
        "ok"
    );
}

#[test]
fn test_python_rejects_a_replaced_working_tree_generation() {
    if which::which("python3").is_err() && which::which("python").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let lib = root.join(".mdc/python/Lib");
    let displaced = root.join("displaced-lib");
    let marker = root.join("executed-generation");
    std::fs::create_dir_all(&lib).unwrap();
    let source = lib.join("node.py");
    std::fs::write(
        &source,
        format!(
            "from pathlib import Path\nPath({:?}).write_text('original')\n",
            marker
        ),
    )
    .unwrap();
    let hook_lib = lib.clone();
    let hook_displaced = displaced.clone();
    let replacement_source = source.clone();
    let replacement_marker = marker.clone();
    crate::workspace::set_test_hook(
        crate::workspace::TestHookPoint::ProcessAfterCwdOpen,
        move || {
            std::fs::rename(&hook_lib, &hook_displaced).unwrap();
            std::fs::create_dir(&hook_lib).unwrap();
            std::fs::write(
                replacement_source,
                format!(
                    "from pathlib import Path\nPath({:?}).write_text('replacement')\n",
                    replacement_marker
                ),
            )
            .unwrap();
        },
    );
    let req = make_req(&root, "python");

    let result = compile("python", &req);

    assert!(!result.is_success());
    assert!(result.stderr.contains("source changed during execution"));
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "original");
}

#[test]
fn test_python_preserves_nested_script_import_and_main_semantics() {
    if which::which("python3").is_err() && which::which("python").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let package = root.join(".mdc/python/Lib/pkg");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("helper.py"), "value = 42\n").unwrap();
    let source = package.join("main.py");
    std::fs::write(
        &source,
        "import __main__, helper, pathlib, sys\n\
         value = helper.value\n\
         assert __main__.value == 42\n\
         assert pathlib.Path(__file__).name == 'main.py'\n\
         assert sys.argv == ['pkg/main.py']\n\
         assert sys.orig_argv[1:] == ['-B', '--', 'pkg/main.py']\n\
         assert __loader__.name == '__main__'\n\
         assert pathlib.Path(__loader__.path).is_absolute()\n\
         pathlib.Path('nested-marker').write_text(str(value))\n",
    )
    .unwrap();
    let mut req = make_req(root, "python");
    req.source = source;

    let result = compile("python", &req);

    assert!(result.is_success(), "{}", result.stderr);
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/python/Lib/nested-marker")).unwrap(),
        "42"
    );
}

#[test]
fn timed_compilers_reject_unresolved_configuration_without_panicking() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".mdc")).unwrap();

    for srctype in ["python", "latex", "lean", "rocq"] {
        let req = CompilerReq {
            mdcroot: tmp.path().to_path_buf(),
            source: test_source_path(tmp.path(), srctype),
            config: crate::config::SrcConfig::default(),
            progress: None,
        };

        let result = compile(srctype, &req);

        assert!(!result.is_success());
        assert!(
            result.stderr.contains("missing timeout_sec"),
            "{srctype}: {}",
            result.stderr
        );
    }
}

#[test]
fn test_latex_compiles_hello_world() {
    if which::which("latexmk").is_err() || which::which("xelatex").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    std::fs::create_dir_all(mdcroot.join(".mdc")).unwrap();
    write_source(mdcroot, "latex", "Hello, world!\n");
    let req = make_req(mdcroot, "latex");
    let res = compile("latex", &req);
    assert!(res.is_success(), "latex compilation failed: {}", res.stderr);
    assert!(mdcroot.join(".mdc/latex/Main.pdf").is_file());
    assert_eq!(
        std::fs::read_to_string(mdcroot.join(".mdc/latex/Lib.tex")).unwrap(),
        "\\input{\\detokenize{Lib/node.tex}}\n"
    );
    assert!(mdcroot.join(".mdc/latex/Main.tex").is_file());
}

#[test]
fn test_latex_rejects_source_changed_during_compilation() {
    if which::which("latexmk").is_err() || which::which("xelatex").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    std::fs::create_dir_all(mdcroot.join(".mdc")).unwrap();
    write_source(mdcroot, "latex", "Original source.\n");
    let source = test_source_path(mdcroot, "latex");
    crate::workspace::set_test_hook(
        crate::workspace::TestHookPoint::ProcessAfterCwdOpen,
        move || std::fs::write(source, "Replacement source.\n").unwrap(),
    );

    let result = compile("latex", &make_req(mdcroot, "latex"));

    assert!(!result.is_success());
    assert!(result
        .stderr
        .contains("LaTeX source changed during compilation"));
}

#[test]
fn test_latex_compiles_tex_active_source_path() {
    if which::which("latexmk").is_err() || which::which("xelatex").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    let source = mdcroot.join(".mdc/latex/Lib/active_&$^~.tex");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "Active path.\n").unwrap();
    let mut request = make_req(mdcroot, "latex");
    request.source = source;

    let result = compile("latex", &request);

    assert!(result.is_success(), "{}", result.stderr);
}

#[test]
fn test_lean_compiles_hello_world() {
    if which::which("lake").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    std::fs::create_dir_all(mdcroot.join(".mdc")).unwrap();
    write_source(mdcroot, "lean", "#check Nat\n");
    let req = make_req(mdcroot, "lean");
    let res = compile("lean", &req);
    assert!(res.is_success(), "lean compilation failed: {}", res.stderr);
    assert!(mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/node.olean")
        .is_file());
}

#[test]
fn test_lean_builds_imports_from_lib_tree() {
    if which::which("lake").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    let lib = mdcroot.join(".mdc/lean/Lib");
    std::fs::create_dir_all(lib.join("data")).unwrap();
    std::fs::write(
        lib.join("data/B.lean"),
        "module\npublic def answer : Nat := 42\n",
    )
    .unwrap();
    std::fs::write(
        lib.join("data/A.lean"),
        "module\nimport Lib.data.B\npublic import Lib.data.B\n-- import Lib.data.C\n#check answer\n",
    )
    .unwrap();
    std::fs::write(
        lib.join("data/C.lean"),
        "module\npublic def independent : Nat := 7\n",
    )
    .unwrap();

    let mut req = make_req(mdcroot, "lean");
    req.source = lib.join("data/A.lean");
    let (result, formal_receipt) = compile_receipt("lean", &req);

    assert!(
        result.is_success(),
        "lean compilation failed: {}",
        result.stderr
    );
    assert!(mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/data/A.olean")
        .is_file());
    assert!(mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/data/B.olean")
        .is_file());
    assert_eq!(
        formal_receipt
            .as_ref()
            .unwrap()
            .direct_dependencies
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["data/B"]
    );
    assert!(!mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/data/C.olean")
        .exists());

    let a_olean = mdcroot.join(".mdc/lean/.lake/build/lib/lean/Lib/data/A.olean");
    let b_olean = mdcroot.join(".mdc/lean/.lake/build/lib/lean/Lib/data/B.olean");
    let a_before = std::fs::metadata(&a_olean).unwrap().modified().unwrap();
    let b_before = std::fs::metadata(&b_olean).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let unchanged = compile("lean", &req);
    assert!(
        unchanged.is_success(),
        "unchanged Lean build failed: {}",
        unchanged.stderr
    );
    assert_eq!(
        std::fs::metadata(&a_olean).unwrap().modified().unwrap(),
        a_before
    );
    assert_eq!(
        std::fs::metadata(&b_olean).unwrap().modified().unwrap(),
        b_before
    );

    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(
        lib.join("data/A.lean"),
        "module\nimport Lib.data.B\npublic import Lib.data.B\n-- import Lib.data.C\n#check answer\n#check Nat\n",
    )
    .unwrap();
    let incremental = compile("lean", &req);
    assert!(
        incremental.is_success(),
        "incremental Lean build failed: {}",
        incremental.stderr
    );
    assert!(std::fs::metadata(&a_olean).unwrap().modified().unwrap() > a_before);
    assert_eq!(
        std::fs::metadata(&b_olean).unwrap().modified().unwrap(),
        b_before
    );

    req.source = lib.join("data/C.lean");
    let switched = compile("lean", &req);
    assert!(
        switched.is_success(),
        "switched Lean build failed: {}",
        switched.stderr
    );
    assert!(mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/data/C.olean")
        .is_file());
    assert_eq!(
        std::fs::read_to_string(mdcroot.join(".mdc/lean/Lib.lean")).unwrap(),
        "import Lib.«data».«C»\n"
    );

    req.source = lib.join("data/A.lean");
    std::fs::remove_file(lib.join("data/B.lean")).unwrap();
    let stale_import = compile("lean", &req);
    assert!(
        !stale_import.is_success(),
        "deleted Lean import remained available"
    );
}

#[test]
fn test_lean_driver_compiles_quoted_module_path() {
    if which::which("lake").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    let source = mdcroot.join(".mdc/lean/Lib/EGA/1-1.1.2.lean");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "#check Nat\n").unwrap();

    let mut req = make_req(mdcroot, "lean");
    req.source = source;
    let result = compile("lean", &req);

    assert!(
        result.is_success(),
        "Lean compilation failed: {}",
        result.stderr
    );
    assert_eq!(
        std::fs::read_to_string(mdcroot.join(".mdc/lean/Lib.lean")).unwrap(),
        "import Lib.«EGA».«1-1.1.2»\n"
    );
    assert!(mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/EGA/1-1.1.2.olean")
        .is_file());
}

#[test]
fn test_rocq_compiles_hello_world() {
    if which::which("rocq").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    std::fs::create_dir_all(mdcroot.join(".mdc")).unwrap();
    write_source(
        mdcroot,
        "rocq",
        "Require Import Corelib.Init.Logic.\nTheorem trivial : True.\nProof. exact I. Qed.\n",
    );
    let req = make_req(mdcroot, "rocq");
    let (res, receipt) = compile_receipt("rocq", &req);
    assert!(res.is_success(), "rocq compilation failed: {}", res.stderr);
    assert!(receipt
        .as_ref()
        .unwrap()
        .external_dependencies
        .keys()
        .any(|path| path.ends_with("/theories/Init/Prelude.vo")));
    assert!(receipt
        .unwrap()
        .external_dependencies
        .keys()
        .any(|path| path.ends_with("/theories/Init/Logic.vo")));
    assert!(mdcroot.join(".mdc/rocq/build/node.vo").is_file());
    assert!(!mdcroot.join(".mdc/rocq/Lib/node.vo").exists());
    assert!(!mdcroot.join(".mdc/rocq/Lib/node.glob").exists());
}

#[test]
fn test_rocq_imports_previous_lib_build_artifacts() {
    if which::which("rocq").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    let lib = mdcroot.join(".mdc/rocq/Lib/Data");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("B.v"), "Definition answer : nat := 42.\n").unwrap();
    std::fs::write(
        lib.join("A.v"),
        "Require Data.B.\nFrom Data Require Import B.\nCheck Data.B.answer.\n",
    )
    .unwrap();

    let mut req = make_req(mdcroot, "rocq");
    req.source = lib.join("B.v");
    let dependency = compile("rocq", &req);
    assert!(
        dependency.is_success(),
        "Rocq dependency compilation failed: {}",
        dependency.stderr
    );
    req.source = lib.join("A.v");
    let (result, formal_receipt) = compile_receipt("rocq", &req);
    assert!(
        result.is_success(),
        "Rocq compilation failed: {}",
        result.stderr
    );
    assert!(mdcroot.join(".mdc/rocq/build/Data/A.vo").is_file());
    assert!(mdcroot.join(".mdc/rocq/build/Data/B.vo").is_file());
    assert_eq!(
        formal_receipt
            .as_ref()
            .unwrap()
            .direct_dependencies
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["Data/B"]
    );

    std::fs::write(lib.join("B.v"), "this is not valid Rocq\n").unwrap();
    let stale_import = compile("rocq", &req);
    assert!(
        !stale_import.is_success(),
        "stale Rocq import remained available"
    );
    assert!(!mdcroot.join(".mdc/rocq/build/Data/B.vo").exists());
}

#[test]
fn test_rocq_rejects_load_dependencies() {
    if which::which("rocq").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    let lib = mdcroot.join(".mdc/rocq/Lib/Data");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("B.v"), "Definition answer : nat := 42.\n").unwrap();
    std::fs::write(lib.join("A.v"), "Load \"Lib/Data/B\".\nCheck answer.\n").unwrap();

    let mut req = make_req(mdcroot, "rocq");
    req.source = lib.join("A.v");
    let result = compile("rocq", &req);
    assert!(!result.is_success());
    assert!(result.stderr.contains("Load dependencies are unsupported"));
}
