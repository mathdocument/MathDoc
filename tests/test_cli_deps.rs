use std::path::Path;
use std::process::{Command, Output};

use mathdoc::mdocnode::MdocNode;

fn init_workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".mdc")).unwrap();
    std::fs::write(root.join(".mdc/config.toml"), "# test\n").unwrap();
}

fn write_node(root: &Path, rel_path: &str, fnode: &str, title: &str, deps: &[&str]) {
    let path = root.join(rel_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut body = format!("@fnode: {fnode}\n@title: {title}\n");
    if !deps.is_empty() {
        body.push_str("\n@dep:\n");
        for dep in deps {
            body.push_str(dep);
            body.push('\n');
        }
        body.push_str("@end\n");
    }
    std::fs::write(path, body).unwrap();
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

#[test]
fn graph_output_escapes_workspace_terminal_controls() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    init_workspace(root);
    write_node(
        root,
        "unsafe.mdoc",
        "safe-node",
        "unsafe\u{1b}]0;title\u{7}",
        &[],
    );

    let output = run_mdc(root, &["graph", "roots"]);
    let stdout = output_text(&output.stdout);

    assert!(!stdout.contains("\u{1b}]"));
    assert!(!stdout.contains('\u{7}'));
}

#[test]
fn dep_add_target_accepts_paths_and_canonicalizes_prefixes() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    init_workspace(root);
    write_node(root, "source.mdoc", "source-node-0001", "Source", &[]);
    write_node(root, "notes/target.mdoc", "target-node-0001", "Target", &[]);

    let output = run_mdc(
        root,
        &["dep", "add", "source.mdoc", "--target", "notes/target.mdoc"],
    );
    assert!(output.status.success(), "{}", output_text(&output.stderr));
    let source = MdocNode::load(root, &root.join("source.mdoc")).unwrap();
    assert_eq!(source.depens, vec!["target-node-0001"]);

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "target-node"]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));
    assert!(output_text(&output.stdout).contains("already a dependency"));
    let source = MdocNode::load(root, &root.join("source.mdoc")).unwrap();
    assert_eq!(source.depens, vec!["target-node-0001"]);

    let output = run_mdc(
        root,
        &[
            "dep",
            "add",
            "source.mdoc",
            "Target",
            "--target",
            "target-node-0001",
        ],
    );
    assert!(!output.status.success());
}

#[test]
fn dep_add_target_rejects_self_missing_ambiguous_and_cyclic_targets() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    init_workspace(root);
    write_node(root, "source.mdoc", "source-node-0001", "Source", &[]);
    write_node(
        root,
        "cycle.mdoc",
        "cycle-node-0001",
        "Cycle",
        &["source-node-0001"],
    );
    write_node(root, "shared-a.mdoc", "shared-alpha", "Shared A", &[]);
    write_node(root, "shared-b.mdoc", "shared-beta", "Shared B", &[]);
    write_node(root, "dup-a.mdoc", "duplicate-target", "Dup A", &[]);
    write_node(root, "dup-b.mdoc", "duplicate-target", "Dup B", &[]);
    write_node(root, "not-mdoc.txt", "text-target", "Text Target", &[]);
    std::fs::write(
        root.join("invalid.mdoc"),
        "@fnode: invalid-target\n@title: Invalid\n@unknown: value\n",
    )
    .unwrap();

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "source-node"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("own dependency"));

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "does-not-exist"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("no mdoc matched"));

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "shared-"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("ambiguous"));

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "dup-a.mdoc"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("duplicate fnode"));

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "./not-mdoc.txt"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains(".mdoc"));

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "cycle.mdoc"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("cycle"));

    let output = run_mdc(root, &["dep", "add", "source.mdoc", "-t", "invalid.mdoc"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("must be valid"));

    let source = MdocNode::load(root, &root.join("source.mdoc")).unwrap();
    assert!(source.depens.is_empty());
}

#[test]
fn dep_rm_target_handles_paths_missing_targets_and_ambiguity() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    init_workspace(root);
    write_node(
        root,
        "source.mdoc",
        "source-node-0001",
        "Source",
        &["target-node-0001", "missing-one-0001", "missing-two-0002"],
    );
    write_node(root, "notes/target.mdoc", "target-node-0001", "Target", &[]);
    write_node(root, "other.mdoc", "other-node-0001", "Other", &[]);

    let output = run_mdc(root, &["dep", "rm", "source.mdoc", "-t", "missing-"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("ambiguous direct dependency"));

    let output = run_mdc(
        root,
        &["dep", "rm", "source.mdoc", "--target", "notes/target.mdoc"],
    );
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    // A dangling dependency can still be removed by its unique prefix even
    // though no target file exists to resolve.
    let output = run_mdc(root, &["dep", "rm", "source.mdoc", "-t", "missing-one"]);
    assert!(output.status.success(), "{}", output_text(&output.stderr));

    let output = run_mdc(root, &["dep", "rm", "source.mdoc", "-t", "other.mdoc"]);
    assert!(!output.status.success());
    assert!(output_text(&output.stderr).contains("not a direct dependency"));

    let source = MdocNode::load(root, &root.join("source.mdoc")).unwrap();
    assert_eq!(source.depens, vec!["missing-two-0002"]);
}
