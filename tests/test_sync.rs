use mathdoc::mdocnode::{MdocNode, SrcBlock};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Output};

fn run_mdc(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn make_node(root: &Path, relative: &str) -> MdocNode {
    let path = root.join(relative);
    let mut node = MdocNode::new_at_path(&path, "Source Mirror");
    node.blocks.push(SrcBlock {
        srctype: "lean".to_string(),
        content: "#check Nat\n".to_string(),
        metadata: HashMap::new(),
    });
    node.blocks[0]
        .metadata
        .insert("module".to_string(), "A".to_string());
    node.blocks.push(SrcBlock {
        srctype: "latex".to_string(),
        content: "\\section{A}\n".to_string(),
        metadata: HashMap::new(),
    });
    node
}

fn write_node(node: &MdocNode) {
    if let Some(parent) = node.path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&node.path, node.render().unwrap()).unwrap();
}

#[test]
fn sync_exports_present_blocks_and_back_imports_mirror_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);

    let output = run_mdc(root, &["sync"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("source files from 1 valid mdocs"));
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/lean/Lib/data/A.lean")).unwrap(),
        "#check Nat\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/latex/Lib/data/A.tex")).unwrap(),
        "\\section{A}\n"
    );
    for path in [
        ".mdc/text/Lib/data/A.txt",
        ".mdc/python/Lib/data/A.py",
        ".mdc/rocq/Lib/data/A.v",
    ] {
        assert!(!root.join(path).exists());
    }
    assert!(root.join(".mdc/source-blocks.json").is_file());

    std::fs::write(root.join(".mdc/lean/Lib/data/A.lean"), "direct edit\n").unwrap();
    let output = run_mdc(root, &["sync"]);
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/lean/Lib/data/A.lean")).unwrap(),
        "direct edit\n"
    );
    let output = run_mdc(root, &["back"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = MdocNode::load(&node.path).unwrap();
    assert_eq!(
        saved
            .blocks
            .iter()
            .find(|block| block.srctype == "lean")
            .unwrap()
            .content,
        "direct edit\n"
    );
    assert_eq!(
        saved
            .blocks
            .iter()
            .find(|block| block.srctype == "lean")
            .unwrap()
            .metadata
            .get("module")
            .unwrap(),
        "A"
    );
}

#[test]
fn sync_and_back_preserve_present_empty_and_absent_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let mut node = make_node(root, "data/A.mdoc");
    node.upsert_source_block("text", String::new()).unwrap();
    write_node(&node);

    assert!(run_mdc(root, &["sync"]).status.success());
    assert!(root.join(".mdc/text/Lib/data/A.txt").is_file());
    assert!(!root.join(".mdc/python/Lib/data/A.py").exists());
    let output = run_mdc(root, &["back"]);
    assert!(output.status.success());
    let saved = MdocNode::load(&node.path).unwrap();
    assert!(saved.source_block("text").is_some());
    assert!(saved.source_block("python").is_none());

    let mut changed = saved;
    assert!(changed.remove_source_block("text"));
    changed
        .upsert_source_block("python", String::new())
        .unwrap();
    write_node(&changed);

    let output = run_mdc(root, &["sync"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("(1 updated, 1 removed)"));
    assert!(!root.join(".mdc/text/Lib/data/A.txt").exists());
    assert_eq!(
        std::fs::read(root.join(".mdc/python/Lib/data/A.py")).unwrap(),
        b""
    );

    let output = run_mdc(root, &["back"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = MdocNode::load(&node.path).unwrap();
    assert!(saved.source_block("text").is_none());
    assert!(saved.source_block("python").is_some());
}

#[test]
fn sync_removes_outputs_for_renamed_sources() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);
    assert!(run_mdc(root, &["sync"]).status.success());
    let unrelated = root.join(".mdc/lean/Lib/data/unrelated.lean");
    std::fs::write(&unrelated, "not generated\n").unwrap();

    std::fs::create_dir_all(root.join("renamed")).unwrap();
    std::fs::rename(root.join("data/A.mdoc"), root.join("renamed/B.mdoc")).unwrap();
    let output = run_mdc(root, &["sync"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    for (srctype, extension) in [("latex", "tex"), ("lean", "lean")] {
        assert!(!root
            .join(format!(".mdc/{srctype}/Lib/data/A.{extension}"))
            .exists());
        assert!(root
            .join(format!(".mdc/{srctype}/Lib/renamed/B.{extension}"))
            .is_file());
    }
    for path in [
        ".mdc/text/Lib/renamed/B.txt",
        ".mdc/python/Lib/renamed/B.py",
        ".mdc/rocq/Lib/renamed/B.v",
    ] {
        assert!(!root.join(path).exists());
    }
    assert_eq!(
        std::fs::read_to_string(unrelated).unwrap(),
        "not generated\n"
    );
}

#[test]
fn sync_warns_for_invalid_mdocs_and_preserves_previous_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    let source_path = node.path.clone();
    write_node(&node);
    assert!(run_mdc(root, &["sync"]).status.success());
    let generated_path = root.join(".mdc/lean/Lib/data/A.lean");
    let generated_before = std::fs::read(&generated_path).unwrap();

    std::fs::write(&source_path, "@title: invalid without fnode\n").unwrap();
    let output = run_mdc(root, &["sync"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("data/A.mdoc"));
    assert_eq!(std::fs::read(generated_path).unwrap(), generated_before);
}

#[test]
fn sync_preserves_old_mirrors_when_a_renamed_source_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);
    assert!(run_mdc(root, &["sync"]).status.success());

    std::fs::create_dir(root.join("renamed")).unwrap();
    std::fs::rename(root.join("data/A.mdoc"), root.join("renamed/B.mdoc")).unwrap();
    std::fs::write(root.join("renamed/B.mdoc"), "@title: invalid\n").unwrap();

    let output = run_mdc(root, &["sync"]);
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/lean/Lib/data/A.lean")).unwrap(),
        "#check Nat\n"
    );
    assert!(!root.join(".mdc/lean/Lib/renamed/B.lean").exists());
}

#[test]
fn sync_preserves_orphaned_v1_mirrors_without_a_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".mdc/lean/Lib/data")).unwrap();
    let mirror = root.join(".mdc/lean/Lib/data/A.lean");
    std::fs::write(&mirror, "uncommitted legacy edit\n").unwrap();
    std::fs::write(
        root.join(".mdc/source-blocks.json"),
        r#"{"version":1,"sources":["646174612f412e6d646f63"]}"#,
    )
    .unwrap();

    let output = run_mdc(root, &["sync"]);
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(mirror).unwrap(),
        "uncommitted legacy edit\n"
    );
}

#[test]
fn sync_migrates_sparse_mirrors_without_losing_absent_block_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".mdc/python/Lib/data")).unwrap();
    std::fs::create_dir_all(root.join(".mdc/rocq/Lib/data")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);
    let empty_digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    std::fs::write(root.join(".mdc/python/Lib/data/A.py"), "").unwrap();
    std::fs::write(root.join(".mdc/rocq/Lib/data/A.v"), "Check nat.\n").unwrap();
    std::fs::write(
        root.join(".mdc/source-blocks.json"),
        format!(
            r#"{{"version":2,"sources":{{"646174612f412e6d646f63":{{"blocks":{{"python":{{"digest":"{empty_digest}","present":false}},"rocq":{{"digest":"{empty_digest}","present":false}}}}}}}}}}"#
        ),
    )
    .unwrap();

    let output = run_mdc(root, &["sync"]);
    assert!(!output.status.success());
    assert!(!root.join(".mdc/python/Lib/data/A.py").exists());
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/rocq/Lib/data/A.v")).unwrap(),
        "Check nat.\n"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".mdc/source-blocks.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["version"], 3);

    let output = run_mdc(root, &["back"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        MdocNode::load(&node.path)
            .unwrap()
            .source_block("rocq")
            .unwrap()
            .content,
        "Check nat.\n"
    );

    let python = root.join(".mdc/python/Lib/data/A.py");
    std::fs::create_dir_all(python.parent().unwrap()).unwrap();
    std::fs::write(&python, "").unwrap();
    assert!(run_mdc(root, &["back"]).status.success());
    assert_eq!(
        MdocNode::load(&node.path)
            .unwrap()
            .source_block("python")
            .unwrap()
            .content,
        ""
    );
}

#[test]
fn sync_rejects_unknown_manifest_source_types() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    std::fs::write(
        root.join(".mdc/source-blocks.json"),
        r#"{"version":2,"sources":{"412e6d646f63":{"blocks":{"..":{"digest":"0000000000000000000000000000000000000000000000000000000000000000","present":true}}}}}"#,
    )
    .unwrap();

    let output = run_mdc(root, &["sync"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid source type"));
}

#[test]
fn sync_and_back_preserve_both_sides_on_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);
    assert!(run_mdc(root, &["sync"]).status.success());

    let mirror_path = root.join(".mdc/lean/Lib/data/A.lean");
    std::fs::write(&mirror_path, "mirror edit\n").unwrap();
    let mut changed = MdocNode::load(&node.path).unwrap();
    changed
        .blocks
        .iter_mut()
        .find(|block| block.srctype == "lean")
        .unwrap()
        .content = "mdoc edit\n".to_string();
    write_node(&changed);

    assert!(!run_mdc(root, &["sync"]).status.success());
    assert!(!run_mdc(root, &["back"]).status.success());
    assert_eq!(
        std::fs::read_to_string(mirror_path).unwrap(),
        "mirror edit\n"
    );
    assert_eq!(
        MdocNode::load(&node.path)
            .unwrap()
            .blocks
            .iter()
            .find(|block| block.srctype == "lean")
            .unwrap()
            .content,
        "mdoc edit\n"
    );
}

#[test]
fn back_imports_clean_blocks_while_preserving_other_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);
    assert!(run_mdc(root, &["sync"]).status.success());

    let lean_mirror = root.join(".mdc/lean/Lib/data/A.lean");
    std::fs::write(&lean_mirror, "accepted mirror edit\n").unwrap();
    let latex_mirror = root.join(".mdc/latex/Lib/data/A.tex");
    std::fs::write(&latex_mirror, "conflicting mirror edit\n").unwrap();
    let mut changed = MdocNode::load(&node.path).unwrap();
    changed
        .blocks
        .iter_mut()
        .find(|block| block.srctype == "latex")
        .unwrap()
        .content = "conflicting mdoc edit\n".to_string();
    write_node(&changed);

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    let saved = MdocNode::load(&node.path).unwrap();
    assert_eq!(
        saved
            .blocks
            .iter()
            .find(|block| block.srctype == "lean")
            .unwrap()
            .content,
        "accepted mirror edit\n"
    );
    assert_eq!(
        saved
            .source_block("lean")
            .unwrap()
            .metadata
            .get("module")
            .unwrap(),
        "A"
    );
    assert_eq!(
        saved
            .blocks
            .iter()
            .find(|block| block.srctype == "latex")
            .unwrap()
            .content,
        "conflicting mdoc edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(latex_mirror).unwrap(),
        "conflicting mirror edit\n"
    );
}

#[test]
fn back_creates_and_deletes_blocks_from_mirrors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);
    assert!(run_mdc(root, &["sync"]).status.success());

    let python = root.join(".mdc/python/Lib/data/A.py");
    std::fs::create_dir_all(python.parent().unwrap()).unwrap();
    std::fs::write(&python, "print('created')\n").unwrap();
    let lean = root.join(".mdc/lean/Lib/data/A.lean");
    std::fs::remove_file(&lean).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = MdocNode::load(&node.path).unwrap();
    assert_eq!(
        saved
            .blocks
            .iter()
            .find(|block| block.srctype == "python")
            .unwrap()
            .content,
        "print('created')\n"
    );
    assert!(saved.blocks.iter().all(|block| block.srctype != "lean"));
    assert!(!lean.exists());
    assert!(run_mdc(root, &["sync"]).status.success());
}

#[test]
fn sync_lib_directory_does_not_overwrite_compiler_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".mdc/lean")).unwrap();
    let compiler_path = root.join(".mdc/lean/lakefile.lean");
    std::fs::write(&compiler_path, "compiler file\n").unwrap();
    let node = make_node(root, "lakefile.mdoc");
    write_node(&node);

    let output = run_mdc(root, &["sync"]);

    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(compiler_path).unwrap(),
        "compiler file\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/lean/Lib/lakefile.lean")).unwrap(),
        "#check Nat\n"
    );
}

#[test]
fn sync_handles_case_only_source_renames_without_deleting_the_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let node = make_node(root, "data/A.mdoc");
    write_node(&node);
    assert!(run_mdc(root, &["sync"]).status.success());

    std::fs::rename(root.join("data/A.mdoc"), root.join("data/a.mdoc")).unwrap();
    let output = run_mdc(root, &["sync"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".mdc/lean/Lib/data/a.lean")).unwrap(),
        "#check Nat\n"
    );
    let names: Vec<_> = std::fs::read_dir(root.join(".mdc/lean/Lib/data"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("a.lean")]);
}
