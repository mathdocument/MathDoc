use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mathdoc::core::FormalCodeStatus;
use mathdoc::indcache::IndCache;
use mathdoc::mdocnode::{MdocNode, SrcBlock};

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn init_workspace(root: &Path) -> PathBuf {
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let bin = root.join("test-bin");
    std::fs::create_dir(&bin).unwrap();
    let lean = bin.join("bin/lean");
    write(&lean, "#!/bin/sh\nexit 0\n");
    let mut permissions = std::fs::metadata(&lean).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&lean, permissions).unwrap();
    let lake = bin.join("lake");
    write(
        &lake,
        r#"#!/bin/sh
if [ "$1" = "init" ]; then
  printf 'name = "Lib"\nversion = "0.1.0"\ndefaultTargets = ["Lib"]\n\n[[lean_lib]]\nname = "Lib"\n' > lakefile.toml
  printf 'leanprover/lean4:stable\n' > lean-toolchain
  exit 0
fi
if [ "$1" = "env" ] && [ "$3" = "--print-prefix" ]; then
  /usr/bin/dirname "$0"
  exit 0
fi
if [ "$1" = "env" ] && [ "$3" = "--src-deps" ]; then
  /usr/bin/awk -v root="$PWD/Lib" '/^[[:space:]]*(public[[:space:]]+|meta[[:space:]]+)?import[[:space:]]+Lib\./ { module=$NF; sub(/^Lib\./, "", module); print root "/" module ".lean" }' "$4"
  exit 0
fi
if [ "$1" = "env" ] && [ "$3" = "--deps" ]; then
  /usr/bin/awk -v root="$PWD/.lake/build/lib/lean/Lib" '/^[[:space:]]*(public[[:space:]]+|meta[[:space:]]+)?import[[:space:]]+Lib\./ { module=$NF; sub(/^Lib\./, "", module); print root "/" module ".olean" }' "$4"
  exit 0
fi
if [ "$1" = "env" ]; then
  exit 0
fi
if [ "${MDC_TEST_LAKE_FAIL:-0}" = "1" ]; then
  exit 7
fi
if [ "$1" = "--quiet" ]; then
  if [ "${MDC_TEST_LAKE_TAMPER_DRIVER:-0}" = "1" ]; then
    printf 'import Lib.tampered\n' > Lib.lean
  fi
  mkdir -p .lake/build/lib/lean/Lib
  for source in Lib/*.lean; do
    [ -f "$source" ] || continue
    module="$(basename "$source" .lean)"
    [ -f ".lake/build/lib/lean/Lib/$module.olean" ] || \
      printf 'artifact for %s\n' "$module" > ".lake/build/lib/lean/Lib/$module.olean"
  done
  exit 0
fi
exit 1
"#,
    );
    let mut permissions = std::fs::metadata(&lake).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&lake, permissions).unwrap();
    let rocq = bin.join("rocq");
    write(
        &bin.join("coqlib/theories/Init/Prelude.vo"),
        "fake Rocq Prelude\n",
    );
    std::fs::create_dir_all(bin.join("coqlib/user-contrib")).unwrap();
    write(
        &rocq,
        r#"#!/bin/sh
if [ "${MDC_TEST_ROCQ_FAIL:-0}" = "1" ]; then
  exit 8
fi
if [ "$1" = "compile" ] && [ "$2" = "-where" ]; then
  printf '%s/coqlib\n' "$(/usr/bin/dirname "$0")"
  exit 0
fi
if [ "$1" = "compile" ]; then
  shift
  output=""
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "-o" ]; then
      output="$2"
      shift 2
    else
      shift
    fi
  done
  mkdir -p "$(dirname "$output")"
  printf 'rocq artifact\n' > "$output"
  exit 0
fi
if [ "$1" = "dep" ]; then
  if [ "${MDC_TEST_ROCQ_MALFORMED_DEP:-0}" = "1" ]; then
    printf 'malformed dependency output\n'
    exit 0
  fi
  source=""
  for value in "$@"; do
    case "$value" in Lib/*.v) source="$value" ;; esac
  done
  printf '%s: %s\n' "${source%.v}.vo" "$source"
  exit 0
fi
exit 1
"#,
    );
    let mut permissions = std::fs::metadata(&rocq).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&rocq, permissions).unwrap();
    bin
}

fn formal_node(
    root: &Path,
    file: &str,
    title: &str,
    dependencies: &[String],
    lean: &str,
    python: Option<&str>,
) -> MdocNode {
    let mut node = MdocNode::new_at_path(&root.join(file), title);
    node.depens = dependencies.to_vec();
    node.blocks.push(SrcBlock {
        srctype: "lean".to_string(),
        content: lean.to_string(),
        metadata: HashMap::new(),
    });
    if let Some(content) = python {
        node.blocks.push(SrcBlock {
            srctype: "python".to_string(),
            content: content.to_string(),
            metadata: HashMap::new(),
        });
    }
    write(&node.path, &node.render().unwrap());
    node
}

fn run_mdc(root: &Path, bin: &Path, args: &[&str], lake_fails: bool) -> Output {
    run_mdc_with_options(root, bin, args, lake_fails, false, false, false)
}

fn run_mdc_with_options(
    root: &Path,
    bin: &Path,
    args: &[&str],
    lake_fails: bool,
    rocq_fails: bool,
    malformed_rocq_dep: bool,
    tamper_driver: bool,
) -> Output {
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(&original_path));
    Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(args)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("MDC_TEST_LAKE_FAIL", if lake_fails { "1" } else { "0" })
        .env("MDC_TEST_ROCQ_FAIL", if rocq_fails { "1" } else { "0" })
        .env(
            "MDC_TEST_ROCQ_MALFORMED_DEP",
            if malformed_rocq_dep { "1" } else { "0" },
        )
        .env(
            "MDC_TEST_LAKE_TAMPER_DRIVER",
            if tamper_driver { "1" } else { "0" },
        )
        .output()
        .unwrap()
}

fn lean_status(root: &Path, fnode: &str) -> FormalCodeStatus {
    formal_status(root, fnode, "lean")
}

fn formal_status(root: &Path, fnode: &str, language: &str) -> FormalCodeStatus {
    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    cache.refresh_all().unwrap();
    let status = cache.formalization_status(fnode).unwrap();
    match language {
        "lean" => status.lean,
        "rocq" => status.rocq,
        _ => panic!("unsupported formal language"),
    }
}

#[test]
fn work_attestations_follow_strict_dependencies_and_propagate_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let dependency = formal_node(
        root,
        "dep.mdoc",
        "Dependency",
        &[],
        "def value : Nat := 1\n",
        None,
    );
    let parent = formal_node(
        root,
        "parent.mdoc",
        "Parent",
        std::slice::from_ref(&dependency.fnode),
        "import Lib.dep\n#check value\n",
        None,
    );

    assert_eq!(
        lean_status(root, &dependency.fnode),
        FormalCodeStatus::Unverified
    );
    let output = run_mdc(root, &bin, &["work", "dep.mdoc"], false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(run_mdc(root, &bin, &["work", "parent.mdoc"], false)
        .status
        .success());
    assert_eq!(lean_status(root, &parent.fnode), FormalCodeStatus::Verified);

    let mut edited = MdocNode::load(&dependency.path).unwrap();
    edited.blocks[0].content = "def value : Nat := 2\n".to_string();
    write(&edited.path, &edited.render().unwrap());
    assert_eq!(
        lean_status(root, &dependency.fnode),
        FormalCodeStatus::Unverified
    );
    assert_eq!(
        lean_status(root, &parent.fnode),
        FormalCodeStatus::Unverified
    );

    assert!(run_mdc(root, &bin, &["work", "dep.mdoc"], false)
        .status
        .success());
    assert_eq!(
        lean_status(root, &dependency.fnode),
        FormalCodeStatus::Verified
    );
    assert_eq!(
        lean_status(root, &parent.fnode),
        FormalCodeStatus::Unverified
    );
    assert!(run_mdc(root, &bin, &["work", "parent.mdoc"], false)
        .status
        .success());
    assert_eq!(lean_status(root, &parent.fnode), FormalCodeStatus::Verified);
}

#[test]
fn claimant_reconciliation_downgrades_attested_referrers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let dependency = formal_node(
        root,
        "dep.mdoc",
        "Dependency",
        &[],
        "def value : Nat := 1\n",
        None,
    );
    let parent = formal_node(
        root,
        "parent.mdoc",
        "Parent",
        std::slice::from_ref(&dependency.fnode),
        "import Lib.dep\n#check value\n",
        None,
    );
    assert!(run_mdc(root, &bin, &["work", "dep.mdoc"], false)
        .status
        .success());
    assert!(run_mdc(root, &bin, &["work", "parent.mdoc"], false)
        .status
        .success());

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(
        cache.formalization_status(&parent.fnode).unwrap().lean,
        FormalCodeStatus::Verified
    );
    std::fs::remove_file(&dependency.path).unwrap();

    assert!(cache
        .reconcile_fnode_paths(&dependency.fnode)
        .unwrap()
        .is_empty());
    assert_eq!(
        cache.formalization_status(&parent.fnode).unwrap().lean,
        FormalCodeStatus::Unverified
    );
}

#[test]
fn work_rejects_import_mismatches_and_invalidates_failed_retries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let dependency = formal_node(
        root,
        "dep.mdoc",
        "Dependency",
        &[],
        "def dep : Nat := 1\n",
        None,
    );
    let extra = formal_node(
        root,
        "extra.mdoc",
        "Extra",
        &[],
        "def extra : Nat := 2\n",
        None,
    );
    let parent = formal_node(
        root,
        "parent.mdoc",
        "Parent",
        std::slice::from_ref(&dependency.fnode),
        "import Lib.dep\nimport Lib.extra\n#check dep\n",
        None,
    );
    assert!(run_mdc(root, &bin, &["work", "dep.mdoc"], false)
        .status
        .success());
    assert!(run_mdc(root, &bin, &["work", "extra.mdoc"], false)
        .status
        .success());

    let mismatch = run_mdc(root, &bin, &["work", "parent.mdoc"], false);
    assert_eq!(mismatch.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("imports must exactly match"));
    assert_eq!(
        lean_status(root, &parent.fnode),
        FormalCodeStatus::Unverified
    );

    let failed_retry = run_mdc(root, &bin, &["work", "dep.mdoc"], true);
    assert_eq!(failed_retry.status.code(), Some(7));
    assert_eq!(
        lean_status(root, &dependency.fnode),
        FormalCodeStatus::Unverified
    );
    let _ = extra;
}

#[test]
fn formal_languages_publish_independently_of_other_target_failures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let node = formal_node(
        root,
        "mixed.mdoc",
        "Mixed",
        &[],
        "#check Nat\n",
        Some("raise RuntimeError('expected failure')\n"),
    );

    let output = run_mdc(root, &bin, &["work", "mixed.mdoc"], false);
    assert!(!output.status.success());
    assert_eq!(lean_status(root, &node.fnode), FormalCodeStatus::Verified);
}

#[test]
fn successful_rocq_work_publishes_an_attestation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let mut node = MdocNode::new_at_path(&root.join("rocq.mdoc"), "Rocq");
    node.blocks.push(SrcBlock {
        srctype: "rocq".to_string(),
        content: "Theorem trivial : True. Proof. exact I. Qed.\n".to_string(),
        metadata: HashMap::new(),
    });
    write(&node.path, &node.render().unwrap());

    let output = run_mdc(root, &bin, &["work", "rocq.mdoc"], false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        formal_status(root, &node.fnode, "rocq"),
        FormalCodeStatus::Verified
    );
}

#[test]
fn rocq_work_rejects_malformed_dependency_output() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let mut node = MdocNode::new_at_path(&root.join("rocq.mdoc"), "Rocq");
    node.blocks.push(SrcBlock {
        srctype: "rocq".to_string(),
        content: "Theorem trivial : True. Proof. exact I. Qed.\n".to_string(),
        metadata: HashMap::new(),
    });
    write(&node.path, &node.render().unwrap());

    let output = run_mdc_with_options(
        root,
        &bin,
        &["work", "rocq.mdoc"],
        false,
        false,
        true,
        false,
    );

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("target separator"));
    assert_eq!(
        formal_status(root, &node.fnode, "rocq"),
        FormalCodeStatus::Unverified
    );
}

#[test]
fn formal_languages_publish_independently() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let mut node = MdocNode::new_at_path(&root.join("mixed-formal.mdoc"), "Mixed formal");
    node.blocks.push(SrcBlock {
        srctype: "lean".to_string(),
        content: "#check Nat\n".to_string(),
        metadata: HashMap::new(),
    });
    node.blocks.push(SrcBlock {
        srctype: "rocq".to_string(),
        content: "Check nat.\n".to_string(),
        metadata: HashMap::new(),
    });
    write(&node.path, &node.render().unwrap());

    let output = run_mdc_with_options(
        root,
        &bin,
        &["work", "mixed-formal.mdoc"],
        false,
        true,
        false,
        false,
    );
    assert!(!output.status.success());
    assert_eq!(
        formal_status(root, &node.fnode, "lean"),
        FormalCodeStatus::Verified
    );
    assert_eq!(
        formal_status(root, &node.fnode, "rocq"),
        FormalCodeStatus::Unverified
    );
}

#[test]
fn work_revokes_attestations_for_removed_formal_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let node = formal_node(
        root,
        "removed.mdoc",
        "Removed formal block",
        &[],
        "#check Nat\n",
        None,
    );
    let original = node.render().unwrap();
    assert!(run_mdc(root, &bin, &["work", "removed.mdoc"], false)
        .status
        .success());
    assert_eq!(lean_status(root, &node.fnode), FormalCodeStatus::Verified);

    let mut without_formal = MdocNode::load(&node.path).unwrap();
    without_formal.blocks.clear();
    write(&node.path, &without_formal.render().unwrap());
    assert!(run_mdc(root, &bin, &["work", "removed.mdoc"], false)
        .status
        .success());

    write(&node.path, &original);
    assert!(run_mdc(root, &bin, &["sync"], false).status.success());
    assert_eq!(lean_status(root, &node.fnode), FormalCodeStatus::Unverified);
}

#[test]
fn work_rejects_a_driver_changed_during_lean_compilation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = init_workspace(root);
    let node = formal_node(
        root,
        "driver-race.mdoc",
        "Driver race",
        &[],
        "#check Nat\n",
        None,
    );

    let output = run_mdc_with_options(
        root,
        &bin,
        &["work", "driver-race.mdoc"],
        false,
        false,
        false,
        true,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("build driver changed"));
    assert_eq!(lean_status(root, &node.fnode), FormalCodeStatus::Unverified);
}
