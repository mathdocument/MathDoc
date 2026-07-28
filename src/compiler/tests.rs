use super::*;

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

#[cfg(unix)]
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

#[cfg(unix)]
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
    let registry = CompilerRegistry::default_registry();
    let result = registry.resolve("python").unwrap().compile(&req);
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

#[cfg(unix)]
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
    let registry = CompilerRegistry::default_registry();
    let result = registry.resolve("python").unwrap().compile(&req);
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

#[cfg(unix)]
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
    let registry = CompilerRegistry::default_registry();
    let compiler = registry.resolve("latex").unwrap();
    let res = compiler.compile(&req);
    assert!(res.is_success(), "latex compilation failed: {}", res.stderr);
    assert!(mdcroot.join(".mdc/latex/Main.pdf").is_file());
    assert_eq!(
        std::fs::read_to_string(mdcroot.join(".mdc/latex/Lib.tex")).unwrap(),
        "\\input{\"Lib/node.tex\"}\n"
    );
    assert!(mdcroot.join(".mdc/latex/Main.tex").is_file());
}

#[cfg(unix)]
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
    let registry = CompilerRegistry::default_registry();
    let compiler = registry.resolve("lean").unwrap();
    let res = compiler.compile(&req);
    assert!(res.is_success(), "lean compilation failed: {}", res.stderr);
    assert!(mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/node.olean")
        .is_file());
}

#[cfg(unix)]
#[test]
fn test_lean_builds_imports_from_lib_tree() {
    if which::which("lake").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mdcroot = tmp.path();
    let lib = mdcroot.join(".mdc/lean/Lib");
    std::fs::create_dir_all(lib.join("data")).unwrap();
    std::fs::write(lib.join("data/B.lean"), "def answer : Nat := 42\n").unwrap();
    std::fs::write(
        lib.join("data/A.lean"),
        "import Lib.data.B\n#check answer\n",
    )
    .unwrap();
    std::fs::write(lib.join("data/C.lean"), "def independent : Nat := 7\n").unwrap();

    let mut req = make_req(mdcroot, "lean");
    req.source = lib.join("data/A.lean");
    let registry = CompilerRegistry::default_registry();
    let result = registry.resolve("lean").unwrap().compile(&req);

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
    assert!(!mdcroot
        .join(".mdc/lean/.lake/build/lib/lean/Lib/data/C.olean")
        .exists());

    let a_olean = mdcroot.join(".mdc/lean/.lake/build/lib/lean/Lib/data/A.olean");
    let b_olean = mdcroot.join(".mdc/lean/.lake/build/lib/lean/Lib/data/B.olean");
    let a_before = std::fs::metadata(&a_olean).unwrap().modified().unwrap();
    let b_before = std::fs::metadata(&b_olean).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let unchanged = registry.resolve("lean").unwrap().compile(&req);
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
        "import Lib.data.B\n#check answer\n#check Nat\n",
    )
    .unwrap();
    let incremental = registry.resolve("lean").unwrap().compile(&req);
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
    let switched = registry.resolve("lean").unwrap().compile(&req);
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
    let stale_import = registry.resolve("lean").unwrap().compile(&req);
    assert!(
        !stale_import.is_success(),
        "deleted Lean import remained available"
    );
}

#[cfg(unix)]
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
    let registry = CompilerRegistry::default_registry();
    let result = registry.resolve("lean").unwrap().compile(&req);

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

#[cfg(unix)]
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
        "Theorem trivial : True.\nProof. exact I. Qed.\n",
    );
    let req = make_req(mdcroot, "rocq");
    let registry = CompilerRegistry::default_registry();
    let compiler = registry.resolve("rocq").unwrap();
    let res = compiler.compile(&req);
    assert!(res.is_success(), "rocq compilation failed: {}", res.stderr);
    assert!(mdcroot.join(".mdc/rocq/build/node.vo").is_file());
    assert!(!mdcroot.join(".mdc/rocq/Lib/node.vo").exists());
    assert!(!mdcroot.join(".mdc/rocq/Lib/node.glob").exists());
}

#[cfg(unix)]
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
        "From Data Require Import B.\nCheck answer.\n",
    )
    .unwrap();

    let registry = CompilerRegistry::default_registry();
    let compiler = registry.resolve("rocq").unwrap();
    let mut req = make_req(mdcroot, "rocq");
    req.source = lib.join("B.v");
    let dependency = compiler.compile(&req);
    assert!(
        dependency.is_success(),
        "Rocq dependency compilation failed: {}",
        dependency.stderr
    );
    req.source = lib.join("A.v");
    let result = compiler.compile(&req);
    assert!(
        result.is_success(),
        "Rocq compilation failed: {}",
        result.stderr
    );
    assert!(mdcroot.join(".mdc/rocq/build/Data/A.vo").is_file());
    assert!(mdcroot.join(".mdc/rocq/build/Data/B.vo").is_file());

    std::fs::write(lib.join("B.v"), "this is not valid Rocq\n").unwrap();
    let stale_import = compiler.compile(&req);
    assert!(
        !stale_import.is_success(),
        "stale Rocq import remained available"
    );
    assert!(!mdcroot.join(".mdc/rocq/build/Data/B.vo").exists());
}
