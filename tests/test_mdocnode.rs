use std::fs;

use mathdoc::mdocnode::{MdocNode, SrcBlock};

fn write_file(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn write_node(node: &MdocNode) {
    write_file(&node.path, &node.render().unwrap());
}

#[test]
fn test_render_write_load_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("roundtrip.mdoc");

    let mut node = MdocNode::new_at_path(&path, "Roundtrip");
    node.add_dependency("dep-a");
    node.blocks.push(SrcBlock {
        srctype: "text".to_string(),
        content: "hello\nworld\n".to_string(),
        metadata: [("lang".to_string(), "en".to_string())].into(),
    });
    write_node(&node);

    let loaded = MdocNode::load(&path).unwrap();
    assert_eq!(loaded.title, "Roundtrip");
    assert_eq!(loaded.path, path);
    assert_eq!(loaded.fnode, node.fnode);
    assert_eq!(loaded.depens, vec!["dep-a"]);
    assert_eq!(loaded.blocks.len(), 1);
    assert_eq!(loaded.blocks[0].srctype, "text");
    assert_eq!(loaded.blocks[0].content, "hello\nworld\n");
    assert_eq!(loaded.blocks[0].metadata.get("lang").unwrap(), "en");
}

#[test]
fn test_add_dependency_is_unique() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("deps.mdoc");
    let mut node = MdocNode::new_at_path(&path, "Deps");
    node.add_dependency("x");
    node.add_dependency("x");
    write_node(&node);

    let loaded = MdocNode::load(&path).unwrap();
    assert_eq!(loaded.depens, vec!["x"]);
}

#[test]
fn test_load_rejects_missing_required_headers() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("bad.mdoc");
    write_file(&path, "@title: no fnode\n");
    assert!(MdocNode::load(&path).is_err());
}

#[test]
fn test_load_preserves_blank_lines_in_src_blocks() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("blank.mdoc");
    write_file(
        &path,
        "@fnode: blank-node\n\
         @title: Blank Lines\n\
         \n\
         @src: python\n\
         print('line1')\n\
         \n\
         print('line3')\n\
         @end\n",
    );
    let node = MdocNode::load(&path).unwrap();
    assert_eq!(node.blocks.len(), 1);
    assert_eq!(node.blocks[0].content, "print('line1')\n\nprint('line3')\n");
}

#[test]
fn test_load_rejects_noncanonical_dependency() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("dep-token.mdoc");
    write_file(
        &path,
        "@fnode: dep-node\n@title: Dep Token\n\n@dep:\nabc:def\n@end\n",
    );
    assert!(MdocNode::load(&path).is_err());
}

#[test]
fn test_load_rejects_noncanonical_fnodes() {
    let dir = tempfile::TempDir::new().unwrap();
    for (index, fnode) in [
        "Uppercase",
        "数学节点",
        "<unknown>",
        "../node",
        "node.name",
        "-node",
        "node-",
    ]
    .into_iter()
    .enumerate()
    {
        let path = dir.path().join(format!("invalid-{index}.mdoc"));
        write_file(&path, &format!("@fnode: {fnode}\n@title: Invalid\n"));
        assert!(MdocNode::load(&path).is_err(), "{fnode}");
    }
}

#[test]
fn test_load_rejects_controls_in_structural_fields() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("control.mdoc");
    write_file(
        &path,
        "@fnode: safe-node\n@title: unsafe\u{1b}]0;title\u{7}\n",
    );
    assert!(MdocNode::load(&path).is_err());
}

#[test]
fn test_remove_dependency() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("rm.mdoc");
    let mut node = MdocNode::new_at_path(&path, "RM");
    node.add_dependency("a");
    node.add_dependency("b");
    node.add_dependency("c");
    node.remove_dependency("b");
    write_node(&node);

    let loaded = MdocNode::load(&path).unwrap();
    assert_eq!(loaded.depens, vec!["a", "c"]);
}

#[test]
fn test_metadata_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("meta.mdoc");
    let mut node = MdocNode::new_at_path(&path, "Meta");
    node.blocks.push(SrcBlock {
        srctype: "latex".to_string(),
        content: String::new(),
        metadata: [("preamble".to_string(), "/some path with spaces".to_string())].into(),
    });
    write_node(&node);

    let loaded = MdocNode::load(&path).unwrap();
    assert_eq!(
        loaded.blocks[0].metadata.get("preamble").unwrap(),
        "/some path with spaces"
    );
}

#[test]
fn test_source_block_upsert_preserves_metadata_and_create_defaults_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("blocks.mdoc");
    let mut node = MdocNode::new_at_path(&path, "Blocks");

    node.upsert_source_block("latex", "first".to_string())
        .unwrap();
    assert!(node.source_block("latex").unwrap().metadata.is_empty());
    node.blocks[0]
        .metadata
        .insert("preamble".to_string(), "article".to_string());

    node.upsert_source_block("LATEX", "second".to_string())
        .unwrap();
    let block = node.source_block("latex").unwrap();
    assert_eq!(block.content, "second\n");
    assert_eq!(block.metadata.get("preamble").unwrap(), "article");
    assert!(node.remove_source_block("Latex"));
    assert!(!node.remove_source_block("latex"));
    assert!(node.source_block("latex").is_none());
}

#[test]
fn test_metadata_rendering_sorts_keys() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("ordered.mdoc");
    let mut node = MdocNode::new_at_path(&path, "Ordered Metadata");
    node.blocks.push(SrcBlock {
        srctype: "text".to_string(),
        content: String::new(),
        metadata: [
            ("zeta".to_string(), "last".to_string()),
            ("alpha".to_string(), "first".to_string()),
            ("middle".to_string(), "center".to_string()),
        ]
        .into(),
    });

    let rendered = node.render().unwrap();
    assert!(rendered.contains("@src: text alpha=\"first\" middle=\"center\" zeta=\"last\""));
}

#[test]
fn test_new_at_path_creates_unique_fnode() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let a = MdocNode::new_at_path(&root.join("a.mdoc"), "A");
    let b = MdocNode::new_at_path(&root.join("b.mdoc"), "B");
    assert_ne!(a.fnode, b.fnode);
    assert!(!a.fnode.is_empty());
}

#[test]
fn test_render_rejects_structural_injection() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("safe.mdoc");
    let mut node = MdocNode::new_at_path(&path, "Safe");
    node.blocks.push(SrcBlock {
        srctype: "text".to_string(),
        content: "original".to_string(),
        metadata: Default::default(),
    });
    node.title = "Injected\n@dep:\nevil\n@end".to_string();
    let err = node.render().unwrap_err().to_string();
    assert!(err.contains("single line"), "unexpected error: {err}");

    node.title = "Safe".to_string();
    node.blocks[0].content = "before\n  @end  \nafter".to_string();
    let err = node.render().unwrap_err().to_string();
    assert!(err.contains("reserved '@end'"), "unexpected error: {err}");
}

#[test]
fn test_render_rejects_unrepresentable_dependencies_and_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("invalid-fields.mdoc");
    let mut node = MdocNode::new_at_path(&path, "Invalid Fields");

    node.depens.push("@end".to_string());
    assert!(node
        .render()
        .unwrap_err()
        .to_string()
        .contains("reserved '@end'"));
    node.depens.clear();
    node.blocks.push(SrcBlock {
        srctype: "text".to_string(),
        content: String::new(),
        metadata: [("bad key".to_string(), "value".to_string())].into(),
    });
    assert!(node.render().is_err());
}

#[test]
fn test_load_rejects_path_traversing_srctype() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("unsafe.mdoc");
    fs::write(
        &path,
        "@fnode: unsafe\n@title: Unsafe\n\n@src: ../outside\nbody\n@end\n",
    )
    .unwrap();

    assert!(MdocNode::load(&path).is_err());
}

#[test]
fn test_parse_and_render_reject_unknown_srctypes() {
    let dir = tempfile::TempDir::new().unwrap();
    let parsed_path = dir.path().join("unknown.mdoc");
    write_file(
        &parsed_path,
        "@fnode: unknown-src\n@title: Unknown Source\n\n@src: markdown\nbody\n@end\n",
    );

    let parse_error = MdocNode::load(&parsed_path).unwrap_err().to_string();
    assert!(parse_error.contains("unsupported srctype 'markdown'"));

    let saved_path = dir.path().join("save-unknown.mdoc");
    let mut node = MdocNode::new_at_path(&saved_path, "Unknown Save");
    node.blocks.push(SrcBlock {
        srctype: "markdown".to_string(),
        content: String::new(),
        metadata: Default::default(),
    });
    let render_error = node.render().unwrap_err().to_string();
    assert!(render_error.contains("unsupported srctype 'markdown'"));
}

#[test]
fn test_load_canonicalizes_known_srctype_case() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("python.mdoc");
    write_file(
        &path,
        "@fnode: python-node\n@title: Python\n\n@src: Python\nprint('ok')\n@end\n",
    );

    let node = MdocNode::load(&path).unwrap();
    assert_eq!(node.blocks[0].srctype, "python");
}

#[test]
fn test_load_rejects_case_only_duplicate_srctypes() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("duplicate-src.mdoc");
    write_file(
        &path,
        "@fnode: python-node\n@title: Python\n\n@src: Python\none\n@end\n\n@src: python\ntwo\n@end\n",
    );

    assert!(MdocNode::load(&path).is_err());
}
