use std::collections::HashMap;
use std::fs;
use std::path::Path;

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
    let tex = &files["latex"];
    assert!(tex.contains("% mdc: preamble"));
    assert!(tex.contains("\\documentclass"));
    assert!(tex.contains(&format!("% mdc: fnode: {}", &src.fnode[..8])));
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

    let tex = &files["latex"];
    // latex default: reverse_depens=true → root first, then deps.
    let src_pos = tex
        .find(&format!("% mdc: fnode: {}", &src.fnode[..8]))
        .unwrap();
    let dep_pos = tex
        .find(&format!("% mdc: fnode: {}", &dep.fnode[..8]))
        .unwrap();
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

    let lean = &files["lean"];
    // lean default: reverse_depens=false → deps first, then root.
    let src_pos = lean
        .find(&format!("-- mdc: fnode: {}", &src.fnode[..8]))
        .unwrap();
    let dep_pos = lean
        .find(&format!("-- mdc: fnode: {}", &dep.fnode[..8]))
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
    let tex = &files["latex"];
    assert!(tex.contains(&format!("% mdc: fnode: {}", &src.fnode[..8])));
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

    let txt = &files["text"];
    assert!(txt.contains("# mdc: fnode:"));
    assert!(txt.contains("# mdc: end"));
}

// ── extract_work_file tests ──────────────────────────────────────────────────

#[test]
fn test_extract_basic_latex() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.tex");
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
    assert_eq!(result.nodes[0].1, "\\section{Hello}");
    assert!(result.warnings.is_empty());
}

#[test]
fn test_extract_multiple_nodes() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.tex");
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: A\ncontent a\n% mdc: end\n\n\
         % mdc: fnode: bbbbbbbb\n% mdc: title: B\ncontent b\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert_eq!(result.nodes.len(), 2);
    assert_eq!(result.nodes[0].0, "aaaaaaaa");
    assert_eq!(result.nodes[0].1, "content a");
    assert_eq!(result.nodes[1].0, "bbbbbbbb");
    assert_eq!(result.nodes[1].1, "content b");
}

#[test]
fn test_extract_empty_content() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.tex");
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: Empty\n% mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].1, "");
}

#[test]
fn test_extract_warns_on_content_outside_markers() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.tex");
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
    let path = dir.path().join("mdc-work.lean");
    fs::write(
        &path,
        "-- mdc: preamble\nimport Mathlib\n-- mdc: end\n\n\
         -- mdc: fnode: aabbccdd\n-- mdc: title: Def\ndef foo := 1\n-- mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "lean").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].0, "aabbccdd");
    assert_eq!(result.nodes[0].1, "def foo := 1");
}

#[test]
fn test_extract_text_hash_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.txt");
    fs::write(
        &path,
        "# mdc: fnode: aabbccdd\n# mdc: title: Note\nsome text\n# mdc: end\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "text").unwrap();
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].1, "some text");
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
    let work_path = dir.path().join("mdc-work.tex");
    fs::write(&work_path, &files["latex"]).unwrap();

    let extracted = workback::extract_work_file(&work_path, "latex").unwrap();
    assert!(extracted.warnings.is_empty());

    // Both nodes should be extracted.
    let fnode_map: HashMap<&str, &str> = extracted
        .nodes
        .iter()
        .map(|(f, c)| (f.as_str(), c.as_str()))
        .collect();

    assert!(fnode_map.contains_key(&src.fnode[..8]));
    assert!(fnode_map.contains_key(&dep.fnode[..8]));
    assert_eq!(fnode_map[&src.fnode[..8]], "src body");
    assert_eq!(fnode_map[&dep.fnode[..8]], "dep body");
}

// ── back: hash not updated on warnings ───────────────────────────────────────

#[test]
fn test_extract_unclosed_block_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.tex");
    fs::write(
        &path,
        "% mdc: fnode: aaaaaaaa\n% mdc: title: Oops\nsome content\n",
    )
    .unwrap();

    let result = workback::extract_work_file(&path, "latex").unwrap();
    // Content is still extracted but a warning is emitted.
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].1, "some content");
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("unclosed"));
}

#[test]
fn test_extract_nested_open_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.tex");
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
    let path = dir.path().join("mdc-work.tex");
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
    let path = dir.path().join("mdc-work.tex");
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
    let path = dir.path().join("mdc-work.py");
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
    let path = dir.path().join("mdc-work.tex");
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
    let path = dir.path().join("mdc-work.py");
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

// ── [P2] unclosed preamble/postamble at EOF ──────────────────────────────────

#[test]
fn test_extract_unclosed_preamble_warns() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mdc-work.tex");
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
    let path = dir.path().join("mdc-work.tex");
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

// ── end-to-end: work → edit preamble → back → eval picks up change ──────────

#[test]
fn test_preamble_roundtrip_work_back_eval() {
    use mathdoc::compiler::CompilerRegistry;
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
    let tex = &files["latex"];
    assert!(
        tex.contains("\\documentclass{book}"),
        "work file should contain custom preamble"
    );

    // 2) User edits the preamble in the work file.
    let work_path = dir.path().join("mdc-work.tex");
    let edited = tex.replace("\\documentclass{book}", "\\documentclass{report}");
    fs::write(&work_path, &edited).unwrap();

    // 3) mdc back: extract and write preamble back.
    let extracted = workback::extract_work_file(&work_path, "latex").unwrap();
    assert!(extracted.warnings.is_empty());
    assert!(extracted.preamble.is_some());
    let pre = extracted.preamble.unwrap();
    assert!(pre.contains("\\documentclass{report}"));
    write_preamble(root, "latex", &format!("{pre}\n")).unwrap();

    // 4) Verify the file now reflects the edit.
    let new_pre = read_preamble(root, "latex");
    assert!(new_pre.contains("report"));
    assert!(!new_pre.contains("book"));

    // 5) mdc eval: compiler should receive the updated preamble.
    let mut graph2 = DepGraph::new(root.to_path_buf(), &src.fnode).unwrap();
    let results = graph2
        .eval_blocks(
            1,
            &CompilerRegistry::default_registry(),
            &config,
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    // latex compiler needs latexmk — it will fail with tool-not-found, but we can
    // verify the error is NOT about a missing preamble config key (which would mean
    // the new preamble wasn't fed to the compiler).
    // If latexmk IS available, it would compile with the new preamble.
    let res = &results[0].res;
    if !res.result {
        // Acceptable: tool not found. NOT acceptable: "config key 'preamble' is required".
        assert!(
            !res.stderr.contains("preamble"),
            "compiler should not complain about missing preamble; got: {}",
            res.stderr
        );
    }
}
