use mathdoc::mdocnode::MdocNode;
use std::process::Command;

#[test]
fn edit_propagates_nonzero_editor_status() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join(".mdc")).unwrap();
    let path = root.join("node.mdoc");
    let node = MdocNode::new_at_path(&path, "Editor Failure");
    std::fs::write(&path, node.render().unwrap()).unwrap();

    let editor = which::which("false").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mdc"))
        .current_dir(root)
        .env("EDITOR", editor)
        .args(["edit", &node.fnode])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("editor exited"));
}
