use std::fs;

use mathdoc::mdocnode::{MdocNode, SrcBlock};

fn write_file(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn test_create_save_load_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("roundtrip.mdoc");

    let mut node = MdocNode::new_at_path(root, &path, "Roundtrip");
    node.add_dependency("dep-a");
    node.blocks.push(SrcBlock {
        srctype: "text".to_string(),
        content: "hello\nworld\n".to_string(),
        metadata: [("lang".to_string(), "en".to_string())].into(),
    });
    node.save().unwrap();

    let loaded = MdocNode::load(root, &path).unwrap();
    assert_eq!(loaded.title, "Roundtrip");
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
    let mut node = MdocNode::new_at_path(root, &path, "Deps");
    node.add_dependency("x");
    node.add_dependency("x");
    node.save().unwrap();

    let loaded = MdocNode::load(root, &path).unwrap();
    assert_eq!(loaded.depens, vec!["x"]);
}

#[test]
fn test_load_rejects_missing_required_headers() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("bad.mdoc");
    write_file(&path, "@title: no fnode\n");
    assert!(MdocNode::load(dir.path(), &path).is_err());
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
    let node = MdocNode::load(root, &path).unwrap();
    assert_eq!(node.blocks.len(), 1);
    assert_eq!(node.blocks[0].content, "print('line1')\n\nprint('line3')\n");
}

#[test]
fn test_load_dependency_keeps_full_token() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("dep-token.mdoc");
    write_file(
        &path,
        "@fnode: dep-node\n@title: Dep Token\n\n@dep:\nabc:def\n@end\n",
    );
    let node = MdocNode::load(root, &path).unwrap();
    assert_eq!(node.depens, vec!["abc:def"]);
}

#[test]
fn test_remove_dependency() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("rm.mdoc");
    let mut node = MdocNode::new_at_path(root, &path, "RM");
    node.add_dependency("a");
    node.add_dependency("b");
    node.add_dependency("c");
    node.remove_dependency("b");
    node.save().unwrap();

    let loaded = MdocNode::load(root, &path).unwrap();
    assert_eq!(loaded.depens, vec!["a", "c"]);
}

#[test]
fn test_metadata_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("meta.mdoc");
    let mut node = MdocNode::new_at_path(root, &path, "Meta");
    node.blocks.push(SrcBlock {
        srctype: "latex".to_string(),
        content: String::new(),
        metadata: [("preamble".to_string(), "/some path with spaces".to_string())].into(),
    });
    node.save().unwrap();

    let loaded = MdocNode::load(root, &path).unwrap();
    assert_eq!(
        loaded.blocks[0].metadata.get("preamble").unwrap(),
        "/some path with spaces"
    );
}

#[test]
fn test_new_at_path_creates_unique_fnode() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let a = MdocNode::new_at_path(root, &root.join("a.mdoc"), "A");
    let b = MdocNode::new_at_path(root, &root.join("b.mdoc"), "B");
    assert_ne!(a.fnode, b.fnode);
    assert!(!a.fnode.is_empty());
}
