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
fn test_load_rejects_noncanonical_dependency() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("dep-token.mdoc");
    write_file(
        &path,
        "@fnode: dep-node\n@title: Dep Token\n\n@dep:\nabc:def\n@end\n",
    );
    assert!(MdocNode::load(root, &path).is_err());
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
        assert!(MdocNode::load(dir.path(), &path).is_err(), "{fnode}");
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
    assert!(MdocNode::load(dir.path(), &path).is_err());
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

#[test]
fn test_save_rejects_structural_injection_without_overwriting_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("safe.mdoc");
    let mut node = MdocNode::new_at_path(root, &path, "Safe");
    node.blocks.push(SrcBlock {
        srctype: "text".to_string(),
        content: "original".to_string(),
        metadata: Default::default(),
    });
    node.save().unwrap();
    let original = fs::read_to_string(&path).unwrap();

    node.title = "Injected\n@dep:\nevil\n@end".to_string();
    let err = node.save().unwrap_err().to_string();
    assert!(err.contains("single line"), "unexpected error: {err}");
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    node.title = "Safe".to_string();
    node.blocks[0].content = "before\n  @end  \nafter".to_string();
    let err = node.save().unwrap_err().to_string();
    assert!(err.contains("reserved '@end'"), "unexpected error: {err}");
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn test_save_rejects_unrepresentable_dependencies_and_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let path = root.join("invalid-fields.mdoc");
    let mut node = MdocNode::new_at_path(root, &path, "Invalid Fields");

    node.depens.push("@end".to_string());
    assert!(node
        .save()
        .unwrap_err()
        .to_string()
        .contains("reserved '@end'"));
    assert!(!path.exists());

    node.depens.clear();
    node.blocks.push(SrcBlock {
        srctype: "text".to_string(),
        content: String::new(),
        metadata: [("bad key".to_string(), "value".to_string())].into(),
    });
    assert!(node.save().is_err());
    assert!(!path.exists());
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

    assert!(MdocNode::load(dir.path(), &path).is_err());
}

#[test]
fn test_load_canonicalizes_known_srctype_case() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("python.mdoc");
    write_file(
        &path,
        "@fnode: python-node\n@title: Python\n\n@src: Python\nprint('ok')\n@end\n",
    );

    let node = MdocNode::load(dir.path(), &path).unwrap();
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

    assert!(MdocNode::load(dir.path(), &path).is_err());
}

#[test]
fn test_save_new_never_replaces_an_existing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("existing.mdoc");
    fs::write(&path, "victim").unwrap();
    let node = MdocNode::new_at_path(dir.path(), &path, "New");

    assert!(node.save_new().is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "victim");
}

#[cfg(unix)]
#[test]
fn test_save_replaces_final_symlink_without_following_it() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let outside = tempfile::TempDir::new().unwrap();
    let outside_path = outside.path().join("outside.mdoc");
    fs::write(&outside_path, "outside victim").unwrap();
    let path = dir.path().join("link.mdoc");
    symlink(&outside_path, &path).unwrap();
    let node = MdocNode::new_at_path(dir.path(), &path, "Safe Replacement");

    node.save().unwrap();
    assert_eq!(fs::read_to_string(&outside_path).unwrap(), "outside victim");
    assert!(!fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        MdocNode::load(dir.path(), &path).unwrap().title,
        "Safe Replacement"
    );
}

#[cfg(unix)]
#[test]
fn test_save_does_not_bypass_read_only_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("readonly.mdoc");
    let mut node = MdocNode::new_at_path(dir.path(), &path, "Original");
    node.save().unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    node.title = "Changed".to_string();

    assert!(node.save().is_err());
    assert_eq!(MdocNode::load(dir.path(), &path).unwrap().title, "Original");
}
