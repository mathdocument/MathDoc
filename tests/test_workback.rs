use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::{Command, Output};

use mathdoc::config::Config;
use mathdoc::depgraph::workback;
use mathdoc::depgraph::DepGraph;
use mathdoc::mdocnode::{MdocNode, SrcBlock};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_node(root: &Path, title: &str, srctype: &str, content: &str) -> MdocNode {
    fs::create_dir_all(root.join(".mdc")).unwrap();
    let mut node = MdocNode::new_at_path(root, root, title);
    node.path = root.join(format!("{}.mdoc", &node.fnode[..8]));
    node.blocks.push(SrcBlock {
        srctype: srctype.to_string(),
        content: content.to_string(),
        metadata: Default::default(),
    });
    node
}

fn load_config(root: &Path) -> Config {
    Config::load(root).unwrap()
}

fn run_mdc(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn legacy_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// ── merge_work_files tests ───────────────────────────────────────────────────

#[test]
fn test_merge_single_node_latex() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Hello", "latex", "\\section{Hello}\n");
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, 1, &config).unwrap();

    assert!(files.contains_key("latex"));
    let tex = &files["latex"].content;
    assert!(tex.contains("% mdc: preamble"));
    assert!(tex.contains("\\documentclass"));
    assert!(tex.contains(&format!("% mdc: fnode: {}", &src.fnode)));
    assert!(tex.contains("% mdc: title: Hello"));
    assert!(tex.contains("\\section{Hello}"));
    assert!(tex.contains("% mdc: postamble"));
    assert!(tex.contains("\\end{document}"));
}

#[test]
fn test_merge_with_deps_respects_reverse_depens() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep = make_node(root, "Dep", "latex", "dep content\n");
    dep.save().unwrap();

    let mut src = make_node(root, "Src", "latex", "src content\n");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, -1, &config).unwrap();

    let tex = &files["latex"].content;
    // latex default: reverse_depens=true → root first, then deps.
    let src_pos = tex.find(&format!("% mdc: fnode: {}", &src.fnode)).unwrap();
    let dep_pos = tex.find(&format!("% mdc: fnode: {}", &dep.fnode)).unwrap();
    assert!(
        src_pos < dep_pos,
        "root should come before dep with reverse_depens=true"
    );
}

#[test]
fn test_merge_lean_reverse_depens_false() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep = make_node(root, "Dep", "lean", "def foo := 1\n");
    dep.save().unwrap();

    let mut src = make_node(root, "Src", "lean", "def bar := foo\n");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, -1, &config).unwrap();

    let lean = &files["lean"].content;
    // lean default: reverse_depens=false → deps first, then root.
    let src_pos = lean
        .find(&format!("-- mdc: fnode: {}", &src.fnode))
        .unwrap();
    let dep_pos = lean
        .find(&format!("-- mdc: fnode: {}", &dep.fnode))
        .unwrap();
    assert!(
        dep_pos < src_pos,
        "dep should come before root with reverse_depens=false"
    );
}

#[test]
fn test_merge_empty_block_still_included() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Empty", "latex", "");
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, 1, &config).unwrap();

    assert!(files.contains_key("latex"));
    let tex = &files["latex"].content;
    assert!(tex.contains(&format!("% mdc: fnode: {}", &src.fnode)));
}

#[test]
fn test_merge_multiple_srctypes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let mut node = MdocNode::new_at_path(root, root, "Multi");
    node.path = root.join(format!("{}.mdoc", &node.fnode[..8]));
    fs::create_dir_all(root.join(".mdc")).unwrap();
    node.blocks.push(SrcBlock {
        srctype: "latex".to_string(),
        content: "tex content\n".to_string(),
        metadata: Default::default(),
    });
    node.blocks.push(SrcBlock {
        srctype: "lean".to_string(),
        content: "lean content\n".to_string(),
        metadata: Default::default(),
    });
    node.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &node.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, 1, &config).unwrap();

    assert!(files.contains_key("latex"));
    assert!(files.contains_key("lean"));
}

#[test]
fn test_merge_text_uses_hash_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(root, "Note", "text", "some text\n");
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, 1, &config).unwrap();

    let txt = &files["text"].content;
    assert!(txt.contains("# mdc: fnode:"));
    assert!(txt.contains("# mdc: end"));
}

#[test]
fn test_merge_always_emits_preamble_postamble() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    // text srctype has empty default preamble/postamble.
    let src = make_node(root, "Note", "text", "some text\n");
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, 1, &config).unwrap();

    let txt = &files["text"].content;
    // Even with empty defaults, preamble and postamble blocks are present.
    assert!(txt.contains("# mdc: preamble\n# mdc: end"));
    assert!(txt.contains("# mdc: postamble\n# mdc: end"));
}

#[test]
fn test_merge_rocq_uses_block_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let src = make_node(
        root,
        "Proof",
        "rocq",
        "Theorem foo : True. Proof. trivial. Qed.\n",
    );
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, 1, &config).unwrap();

    let v = &files["rocq"].content;
    assert!(v.contains("(* mdc: fnode:"));
    assert!(v.contains("(* mdc: end *)"));
    assert!(v.contains("(* mdc: preamble *)"));
    assert!(v.contains("(* mdc: postamble *)"));
}

#[test]
fn test_extract_rocq_markers() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.v");
    fs::write(
        &path,
        "(* mdc: preamble *)\n(* mdc: end *)\n\n\
         (* mdc: fnode: aabbccdd *)\n(* mdc: title: Proof *)\nTheorem foo : True.\n(* mdc: end *)\n\n\
         (* mdc: postamble *)\n(* mdc: end *)\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "rocq").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].0, "aabbccdd");
    assert_eq!(result.nodes[0].2, "Theorem foo : True.");
    assert!(result.warnings.is_empty());
    assert!(
        result.preamble.is_none(),
        "empty preamble block should normalize to None"
    );
    assert!(
        result.postamble.is_none(),
        "empty postamble block should normalize to None"
    );
}

// ── extract_work_file tests ──────────────────────────────────────────────────

#[test]
fn test_extract_basic_latex() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(
        &path,
        "% mdc: preamble\n\\documentclass{article}\n% mdc: end\n\n\
         % mdc: fnode: aabbccdd\n% mdc: title: Hello\n\\section{Hello}\n% mdc: end\n\n\
         % mdc: postamble\n\\end{document}\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].0, "aabbccdd");
    assert_eq!(result.nodes[0].2, "\\section{Hello}");
    assert!(result.warnings.is_empty());
}

#[test]
fn test_extract_multiple_nodes() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent a\n% mdc: end\n\n\
         % mdc: fnode: bbbbbbbb\n% mdc: title: B\ncontent b\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert_eq!(result.nodes.len(), 2);
    assert_eq!(result.nodes[0].0, "aaaaaaaa");
    assert_eq!(result.nodes[0].2, "content a");
    assert_eq!(result.nodes[1].0, "bbbbbbbb");
    assert_eq!(result.nodes[1].2, "content b");
}

#[test]
fn test_extract_empty_content() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: Empty\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].2, "");
}

#[test]
fn test_extract_warns_on_content_outside_markers() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(
        &path,
        "stray line\n% mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("stray line"));
}

#[test]
fn test_extract_lean_markers() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.lean");
    fs::write(
        &path,
        "-- mdc: preamble\nimport Mathlib\n-- mdc: end\n\n\
         -- mdc: fnode: aabbccdd\n-- mdc: title: Def\ndef foo := 1\n-- mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "lean").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].0, "aabbccdd");
    assert_eq!(result.nodes[0].2, "def foo := 1");
}

#[test]
fn test_extract_text_hash_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.txt");
    fs::write(
        &path,
        "# mdc: fnode: aabbccdd\n# mdc: title: Note\nsome text\n# mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "text").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].2, "some text");
}

// ── roundtrip: merge → extract ───────────────────────────────────────────────

#[test]
fn test_merge_then_extract_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    let dep = make_node(root, "Dep", "latex", "dep body\n");
    dep.save().unwrap();

    let mut src = make_node(root, "Src", "latex", "src body\n");
    src.add_dependency(&dep.fnode);
    src.save().unwrap();

    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, -1, &config).unwrap();

    // Write to a temp file, then extract.
    let work_path = dir.path().join("MdcWork.tex");
    fs::write(&work_path, &files["latex"].content).unwrap();

    let extracted = workback::extract_work_file(&work_path, "latex").unwrap();
    assert!(extracted.warnings.is_empty());

    // Both nodes should be extracted.
    let fnode_map: HashMap<&str, &str> = extracted
        .nodes
        .iter()
        .map(|(f, _t, c)| (f.as_str(), c.as_str()))
        .collect();

    assert!(fnode_map.contains_key(src.fnode.as_str()));
    assert!(fnode_map.contains_key(dep.fnode.as_str()));
    assert_eq!(fnode_map[src.fnode.as_str()], "src body");
    assert_eq!(fnode_map[dep.fnode.as_str()], "dep body");
}

// ── back: hash not updated on warnings ───────────────────────────────────────

#[test]
fn test_extract_unclosed_block_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: Oops\nsome content\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    // Content is still extracted but a warning is emitted.
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].2, "some content");
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("unclosed"));
}

#[test]
fn test_extract_nested_open_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    // Second fnode opened without closing the first.
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent a\n\
         % mdc: fnode: bbbbbbbb\n% mdc: title: B\ncontent b\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert!(
        !result.warnings.is_empty(),
        "should warn about nested open block"
    );
    assert!(result.warnings.iter().any(|w| w.contains("unclosed")));
}

#[test]
fn test_extract_fnode_inside_preamble_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    // fnode marker appears while preamble is still open.
    fs::write(
        &path,
        "% mdc: preamble\n\\documentclass{article}\n\
         % mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert!(
        !result.warnings.is_empty(),
        "should warn about block opened inside preamble"
    );
    assert!(result.warnings.iter().any(|w| w.contains("unclosed")));
}

// ── [P1] stray end marker ────────────────────────────────────────────────────

#[test]
fn test_extract_stray_end_marker_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    // A valid block followed by an extra end marker.
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent\n% mdc: end\n\
         % mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert!(
        !result.warnings.is_empty(),
        "stray end marker should produce a warning"
    );
    assert!(result.warnings.iter().any(|w| w.contains("stray end")));
}

// ── [P1] title marker outside fnode block ────────────────────────────────────

#[test]
fn test_extract_title_outside_fnode_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.py");
    // title marker appears outside any block.
    fs::write(
        &path,
        "# mdc: title: Orphan\n\
         # mdc: fnode: aaaaaaaa\n# mdc: title: A\ncontent\n# mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "python").unwrap();
    assert!(
        !result.warnings.is_empty(),
        "title marker outside fnode block should warn"
    );
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("title marker outside")));
}

#[test]
fn test_extract_title_inside_preamble_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    // title marker inside preamble — not a fnode block.
    fs::write(
        &path,
        "% mdc: preamble\n% mdc: title: Sneaky\n\\documentclass{article}\n% mdc: end\n\
         % mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert!(
        !result.warnings.is_empty(),
        "title marker inside preamble should warn"
    );
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("title marker outside")));
}

#[test]
fn test_extract_duplicate_title_in_fnode_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.py");
    // A second title: line inside the fnode block content.
    fs::write(
        &path,
        "# mdc: fnode: aaaaaaaa\n# mdc: title: Real\n\
         some code\n# mdc: title: NOT_REAL\nmore code\n# mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "python").unwrap();
    assert!(
        !result.warnings.is_empty(),
        "duplicate title marker in fnode block should warn"
    );
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("duplicate title")));
}

#[test]
fn test_extract_duplicate_closed_sections_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(
        &path,
        "% mdc: preamble\nfirst\n% mdc: end\n\
         % mdc: preamble\nsecond\n% mdc: end\n\
         % mdc: fnode: aaaaaaaa\n% mdc: title: A\nfirst\n% mdc: end\n\
         % mdc: fnode: aaaaaaaa\n% mdc: title: A\nsecond\n% mdc: end\n\
         % mdc: postamble\nfirst\n% mdc: end\n\
         % mdc: postamble\nsecond\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("duplicate preamble")));
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("duplicate fnode")));
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("duplicate postamble")));
}

// ── [P2] unclosed preamble/postamble at EOF ──────────────────────────────────

#[test]
fn test_extract_unclosed_preamble_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(&path, "% mdc: preamble\n\\documentclass{article}\n").unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert!(!result.warnings.is_empty(), "unclosed preamble should warn");
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("unclosed preamble")));
}

#[test]
fn test_extract_unclosed_postamble_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("MdcWork.tex");
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent\n% mdc: end\n\
         % mdc: postamble\n\\end{document}\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert!(
        !result.warnings.is_empty(),
        "unclosed postamble should warn"
    );
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("unclosed postamble")));
}

// ── end-to-end: work → edit preamble → back → re-merge picks up change ──────

#[test]
fn test_preamble_roundtrip_work_back() {
    use mathdoc::config::{read_preamble, write_preamble};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();

    // Write an initial custom preamble so merge_work_files includes it.
    write_preamble(root, "latex", "\\documentclass{book}\n\\begin{document}\n").unwrap();

    let src = make_node(root, "Hello", "latex", "\\section{Hello}\n");
    src.save().unwrap();

    // 1) mdc work: generate work file
    let mut graph = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let config = load_config(root);
    let files = workback::merge_work_files(&mut graph, 1, &config).unwrap();
    let tex = &files["latex"].content;
    assert!(
        tex.contains("\\documentclass{book}"),
        "work file should contain custom preamble"
    );

    // 2) User edits the preamble in the work file.
    let work_path = dir.path().join("MdcWork.tex");
    let edited = tex.replace("\\documentclass{book}", "\\documentclass{report}");
    fs::write(&work_path, &edited).unwrap();

    // 3) mdc back: extract and write preamble back.
    let extracted = workback::extract_work_file(&work_path, "latex").unwrap();
    assert!(extracted.warnings.is_empty());
    assert!(extracted.preamble.is_some());
    let pre = extracted.preamble.unwrap();
    assert!(pre.contains("\\documentclass{report}"));
    write_preamble(root, "latex", &format!("{pre}\n")).unwrap();

    // 4) Verify the preamble file reflects the edit.
    let new_pre = read_preamble(root, "latex");
    assert!(new_pre.contains("report"));
    assert!(!new_pre.contains("book"));

    // 5) Re-merge: new work file should use the updated preamble.
    let mut graph2 = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let files2 = workback::merge_work_files(&mut graph2, 1, &config).unwrap();
    let tex2 = &files2["latex"].content;
    assert!(
        tex2.contains("\\documentclass{report}"),
        "re-merged work file should contain updated preamble"
    );
    assert!(
        !tex2.contains("\\documentclass{book}"),
        "old preamble should be gone"
    );
}

#[test]
fn back_rejects_divergent_work_and_source_changes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Conflict", "latex", "baseline body\n");
    src.save().unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let edited_work = fs::read_to_string(&work_path)
        .unwrap()
        .replace("baseline body", "work body");
    fs::write(&work_path, &edited_work).unwrap();

    let mut changed_source = MdocNode::load(root, &src.path).unwrap();
    changed_source.blocks[0].content = "source body\n".to_string();
    changed_source.save().unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("conflict in fnode"));

    let current_source = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(current_source.blocks[0].content, "source body\n");
    assert!(fs::read_to_string(&work_path)
        .unwrap()
        .contains("work body"));
}

#[test]
fn back_conflicts_when_an_empty_source_block_was_deleted() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Empty", "latex", "");
    src.save().unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));
    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let sidecar_path = root.join(".mdc/latex/.MdcWork.hash");
    let sidecar_before = fs::read(&sidecar_path).unwrap();

    let mut changed_source = MdocNode::load(root, &src.path).unwrap();
    changed_source
        .blocks
        .retain(|block| block.srctype != "latex");
    changed_source.save().unwrap();

    let edited_work = fs::read_to_string(&work_path).unwrap().replace(
        "% mdc: title: Empty\n% mdc: end",
        "% mdc: title: Empty\nwork body\n% mdc: end",
    );
    fs::write(&work_path, edited_work).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("conflict in fnode"));
    let current_source = MdocNode::load(root, &src.path).unwrap();
    assert!(current_source
        .blocks
        .iter()
        .all(|block| block.srctype != "latex"));
    assert_eq!(fs::read(&sidecar_path).unwrap(), sidecar_before);
}

#[test]
fn back_title_error_writes_nothing_for_work_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Original Title", "latex", "original body\n");
    src.save().unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let edited = fs::read_to_string(&work_path)
        .unwrap()
        .replace("\\documentclass{article}", "\\documentclass{report}")
        .replace(
            "% mdc: title: Original Title",
            "% mdc: title: Changed Title",
        )
        .replace("original body", "work body");
    fs::write(&work_path, edited).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("title of"));
    assert!(!root.join(".mdc/latex/preamble.tex").exists());

    let current_source = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(current_source.blocks[0].content, "original body\n");
}

#[test]
fn back_unresolved_target_writes_nothing_for_work_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let missing = make_node(root, "Missing", "latex", "dependency body\n");
    missing.save().unwrap();
    let mut src = make_node(root, "Source", "latex", "original body\n");
    src.add_dependency(&missing.fnode);
    src.save().unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let edited = fs::read_to_string(&work_path)
        .unwrap()
        .replace("\\documentclass{article}", "\\documentclass{report}")
        .replace("original body", "work body");
    fs::write(&work_path, edited).unwrap();
    fs::remove_file(&missing.path).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("cannot resolve fnode"));
    assert!(!root.join(".mdc/latex/preamble.tex").exists());

    let current_source = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(current_source.blocks[0].content, "original body\n");
}

#[test]
fn back_duplicate_section_writes_nothing_for_work_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Duplicate", "latex", "original body\n");
    src.save().unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let mut edited = fs::read_to_string(&work_path)
        .unwrap()
        .replace("\\documentclass{article}", "\\documentclass{report}");
    edited.push_str(&format!(
        "\n% mdc: fnode: {}\n% mdc: title: Duplicate\nduplicate body\n% mdc: end\n",
        src.fnode
    ));
    fs::write(&work_path, edited).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("duplicate fnode section"));
    assert!(!root.join(".mdc/latex/preamble.tex").exists());

    let current_source = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(current_source.blocks[0].content, "original body\n");
}

#[test]
fn work_propagates_hash_sidecar_write_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Sidecar", "latex", "body\n");
    src.save().unwrap();
    fs::create_dir_all(root.join(".mdc/latex/.MdcWork.hash")).unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("non-regular file"));
    assert!(!root.join(".mdc/latex/MdcWork.tex").exists());
}

#[test]
fn work_accepts_and_migrates_legacy_hash_sidecar() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Legacy Sidecar", "latex", "body\n");
    src.save().unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));
    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let sidecar_path = root.join(".mdc/latex/.MdcWork.hash");
    let work = fs::read_to_string(&work_path).unwrap();
    fs::write(&sidecar_path, format!("@file={}\n", legacy_hash(&work))).unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));
    let sidecar: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path).unwrap()).unwrap();
    assert_eq!(sidecar["version"], 3);
    assert_eq!(sidecar["algorithm"], "sha256");
    assert_eq!(sidecar["file"].as_str().unwrap().len(), 64);
}

#[test]
fn back_rejects_divergent_preamble_changes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Amble Conflict", "latex", "body\n");
    src.save().unwrap();
    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let edited = fs::read_to_string(&work_path)
        .unwrap()
        .replace("\\documentclass{article}", "\\documentclass{report}");
    fs::write(&work_path, edited).unwrap();
    fs::create_dir_all(root.join(".mdc/latex")).unwrap();
    fs::write(
        root.join(".mdc/latex/preamble.tex"),
        "\\documentclass{book}\n\\begin{document}\n",
    )
    .unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("conflict in preamble"));
    assert!(fs::read_to_string(root.join(".mdc/latex/preamble.tex"))
        .unwrap()
        .contains("book"));
}

#[test]
fn back_rejects_missing_amble_section() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Missing Amble", "latex", "original body\n");
    src.save().unwrap();
    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let work = fs::read_to_string(&work_path).unwrap();
    let preamble_end = work.find("% mdc: end\n\n").unwrap() + "% mdc: end\n\n".len();
    fs::write(&work_path, &work[preamble_end..]).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("exactly one preamble"));
    assert_eq!(
        MdocNode::load(root, &src.path).unwrap().blocks[0].content,
        "original body\n"
    );
}

#[cfg(unix)]
#[test]
fn work_rejects_symlink_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Symlink", "latex", "body\n");
    src.save().unwrap();
    fs::create_dir_all(root.join(".mdc/latex")).unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "outside").unwrap();
    symlink(outside.path(), root.join(".mdc/latex/MdcWork.tex")).unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("refusing to access symlink"));
    assert_eq!(fs::read_to_string(outside.path()).unwrap(), "outside");
}

#[test]
fn back_rejects_missing_node_section() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Missing Node", "latex", "original body\n");
    src.save().unwrap();
    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let work = fs::read_to_string(&work_path).unwrap();
    let node_start = work.find("% mdc: fnode:").unwrap();
    let postamble_start = work.find("% mdc: postamble").unwrap();
    let truncated = format!("{}{}", &work[..node_start], &work[postamble_start..]);
    fs::write(&work_path, truncated).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("node sections do not match"));
    assert_eq!(
        MdocNode::load(root, &src.path).unwrap().blocks[0].content,
        "original body\n"
    );
}

#[test]
fn work_rejects_source_content_that_looks_like_a_marker() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Marker", "latex", "% mdc: end\n");
    src.save().unwrap();

    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("reserved work-file marker"));
}

#[test]
fn work_rejects_amble_content_that_looks_like_a_marker() {
    for kind in ["preamble", "postamble"] {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let src = make_node(root, "Marker", "latex", "body\n");
        src.save().unwrap();
        fs::create_dir_all(root.join(".mdc/latex")).unwrap();
        fs::write(
            root.join(format!(".mdc/latex/{kind}.tex")),
            "% mdc: custom\n",
        )
        .unwrap();

        let output = run_mdc(root, &["work", &src.fnode]);
        assert!(!output.status.success());
        assert!(output_text(&output.stderr).contains("reserved work-file marker"));
        assert!(!root.join(".mdc/latex/MdcWork.tex").exists());
    }
}

#[test]
fn back_rejects_content_that_cannot_be_generated_again() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let src = make_node(root, "Marker", "latex", "original body\n");
    src.save().unwrap();
    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let work_path = root.join(".mdc/latex/MdcWork.tex");
    let edited = fs::read_to_string(&work_path)
        .unwrap()
        .replace("\\documentclass{article}", "% mdc: custom")
        .replace("original body", "edited body\n% mdc: custom node");
    fs::write(&work_path, edited).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("reserved work-file marker"));
    assert!(!root.join(".mdc/latex/preamble.tex").exists());
    assert_eq!(
        MdocNode::load(root, &src.path).unwrap().blocks[0].content,
        "original body\n"
    );
}

#[test]
fn back_combines_multiple_source_type_edits_to_one_node() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let mut src = make_node(root, "Multi", "latex", "latex original\n");
    src.blocks.push(SrcBlock {
        srctype: "python".to_string(),
        content: "python_original = 1\n".to_string(),
        metadata: Default::default(),
    });
    src.save().unwrap();
    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let latex_path = root.join(".mdc/latex/MdcWork.tex");
    let latex = fs::read_to_string(&latex_path)
        .unwrap()
        .replace("latex original", "latex edited");
    fs::write(latex_path, latex).unwrap();
    let python_path = root.join(".mdc/python/MdcWork.py");
    let python = fs::read_to_string(&python_path)
        .unwrap()
        .replace("python_original = 1", "python_edited = 2");
    fs::write(python_path, python).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));
    let node = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(
        node.blocks
            .iter()
            .find(|block| block.srctype == "latex")
            .unwrap()
            .content,
        "latex edited\n"
    );
    assert_eq!(
        node.blocks
            .iter()
            .find(|block| block.srctype == "python")
            .unwrap()
            .content,
        "python_edited = 2\n"
    );
}

#[test]
fn back_validation_failure_leaves_every_source_type_unchanged() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let mut src = make_node(root, "Multi", "latex", "latex original\n");
    src.blocks.push(SrcBlock {
        srctype: "python".to_string(),
        content: "python_original = 1\n".to_string(),
        metadata: Default::default(),
    });
    src.save().unwrap();
    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let latex_path = root.join(".mdc/latex/MdcWork.tex");
    let latex = fs::read_to_string(&latex_path)
        .unwrap()
        .replace("latex original", "latex edited");
    fs::write(&latex_path, latex).unwrap();
    let latex_sidecar = root.join(".mdc/latex/.MdcWork.hash");
    let sidecar_before = fs::read(&latex_sidecar).unwrap();

    let python_path = root.join(".mdc/python/MdcWork.py");
    let python = fs::read_to_string(&python_path)
        .unwrap()
        .replace("# mdc: title: Multi", "# mdc: title: Changed");
    fs::write(python_path, python).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("title of"));
    let node = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(
        node.blocks
            .iter()
            .find(|block| block.srctype == "latex")
            .unwrap()
            .content,
        "latex original\n"
    );
    assert_eq!(
        node.blocks
            .iter()
            .find(|block| block.srctype == "python")
            .unwrap()
            .content,
        "python_original = 1\n"
    );
    assert_eq!(fs::read(latex_sidecar).unwrap(), sidecar_before);
}

#[test]
fn back_apply_failure_rolls_back_every_source_type() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let mut src = make_node(root, "Multi", "latex", "latex original\n");
    src.blocks.push(SrcBlock {
        srctype: "python".to_string(),
        content: "python_original = 1\n".to_string(),
        metadata: Default::default(),
    });
    src.save().unwrap();
    let output = run_mdc(root, &["work", &src.fnode]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let latex_path = root.join(".mdc/latex/MdcWork.tex");
    let latex = fs::read_to_string(&latex_path)
        .unwrap()
        .replace("latex original", "latex edited");
    fs::write(&latex_path, latex).unwrap();
    let latex_sidecar = root.join(".mdc/latex/.MdcWork.hash");
    let sidecar_before = fs::read(&latex_sidecar).unwrap();

    let python_path = root.join(".mdc/python/MdcWork.py");
    let python = fs::read_to_string(&python_path)
        .unwrap()
        .replace("python_original = 1", "python_edited = 2");
    fs::write(python_path, python).unwrap();
    let python_sidecar = root.join(".mdc/python/.MdcWork.hash");
    let mut permissions = fs::metadata(&python_sidecar).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&python_sidecar, permissions).unwrap();

    let output = run_mdc(root, &["back"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("read-only"));
    let node = MdocNode::load(root, &src.path).unwrap();
    assert_eq!(
        node.blocks
            .iter()
            .find(|block| block.srctype == "latex")
            .unwrap()
            .content,
        "latex original\n"
    );
    assert_eq!(
        node.blocks
            .iter()
            .find(|block| block.srctype == "python")
            .unwrap()
            .content,
        "python_original = 1\n"
    );
    assert_eq!(fs::read(latex_sidecar).unwrap(), sidecar_before);
}
