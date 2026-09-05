use mathdoc::application::nodes::{create_nodes, edit_nodes, NewNode, NodeChange, NodeEdit};
use mathdoc::indcache::WorkspaceStore;

fn fixture() -> (tempfile::TempDir, WorkspaceStore) {
    let dir = tempfile::tempdir().unwrap();
    mathdoc::workspace::initialize(dir.path()).unwrap();
    let mut store = WorkspaceStore::open(dir.path().to_path_buf()).unwrap();
    create_nodes(&mut store, &[seed("a"), seed("b"), seed("c")]).unwrap();
    (dir, store)
}
fn seed(name: &str) -> NewNode {
    NewNode {
        file: name.into(),
        title: name.into(),
        fnode: Some(name.into()),
    }
}
fn edit(name: &str, change: NodeChange) -> NodeEdit {
    NodeEdit {
        reference: name.into(),
        expected_revision: None,
        changes: vec![change],
    }
}

#[test]
fn batch_links_share_a_graph_and_update_depths_together() {
    let (_dir, mut store) = fixture();
    edit_nodes(
        &mut store,
        &[
            edit("a", NodeChange::AddDependencies(vec!["b".into()])),
            edit("b", NodeChange::AddDependencies(vec!["c".into()])),
            edit("a", NodeChange::SetTitle("New A".into())),
        ],
        None,
    )
    .unwrap();
    assert_eq!(store.node_summary("a").unwrap().depth, 2);
    assert_eq!(store.node_summary("b").unwrap().depth, 1);
    assert_eq!(store.node_summary("a").unwrap().title, "New A");
}

#[test]
fn cycle_created_only_by_combining_batch_edges_is_rejected_before_writes() {
    let (dir, mut store) = fixture();
    let before = std::fs::read(dir.path().join("a.mdoc")).unwrap();
    let error = edit_nodes(
        &mut store,
        &[
            edit("a", NodeChange::AddDependencies(vec!["b".into()])),
            edit("b", NodeChange::AddDependencies(vec!["a".into()])),
        ],
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("cycle"));
    assert!(store.all_valid_edges().unwrap().is_empty());
    assert_eq!(std::fs::read(dir.path().join("a.mdoc")).unwrap(), before);
}

#[test]
fn stale_revision_or_duplicate_output_rejects_the_whole_batch() {
    let (dir, mut store) = fixture();
    let mut stale = edit("b", NodeChange::SetTitle("B changed".into()));
    stale.expected_revision = Some("stale".into());
    assert!(edit_nodes(
        &mut store,
        &[edit("a", NodeChange::SetTitle("A changed".into())), stale,],
        None
    )
    .is_err());
    assert_eq!(store.node_summary("a").unwrap().title, "a");
    let mut duplicate = seed("new-2");
    duplicate.file = "new-1".into();
    assert!(create_nodes(&mut store, &[seed("new-1"), duplicate]).is_err());
    assert!(!dir.path().join("new-1.mdoc").exists());
    assert_eq!(store.count().unwrap(), 3);
}

#[test]
fn no_op_preserves_noncanonical_bytes_and_missing_dependencies_can_be_removed() {
    let (dir, mut store) = fixture();
    let path = dir.path().join("a.mdoc");
    let bytes = "@fnode: a\n@title: a\n\n\n@dep:\nmissing\n@end\n";
    std::fs::write(&path, bytes).unwrap();
    edit_nodes(
        &mut store,
        &[edit("a", NodeChange::SetTitle("a".into()))],
        None,
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    edit_nodes(
        &mut store,
        &[edit(
            "a",
            NodeChange::RemoveDependencies(vec!["missing".into()]),
        )],
        None,
    )
    .unwrap();
    assert!(store.all_valid_edges().unwrap().is_empty());
}

#[test]
fn index_failure_restores_every_file_in_the_batch() {
    let (dir, mut store) = fixture();
    let connection = rusqlite::Connection::open(dir.path().join(".mdc/index.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_batch_title BEFORE UPDATE OF title ON mdocs
        WHEN NEW.title LIKE 'Changed%' BEGIN SELECT RAISE(ABORT, 'injected index failure'); END;",
        )
        .unwrap();
    assert!(edit_nodes(
        &mut store,
        &[
            edit("a", NodeChange::SetTitle("Changed A".into())),
            edit("b", NodeChange::SetTitle("Changed B".into())),
        ],
        None
    )
    .is_err());
    for name in ["a", "b"] {
        assert_eq!(store.node_summary(name).unwrap().title, name);
        let node =
            mathdoc::mdocnode::MdocNode::load(&dir.path().join(format!("{name}.mdoc"))).unwrap();
        assert_eq!(node.title, name);
    }
}
