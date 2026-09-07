use mathdoc::store::{Database, Node};

#[tokio::test]
#[ignore = "requires local TerminusDB and MDC_TERMINUS_PASSWORD"]
async fn terminus_atomic_revisions_and_persistence() {
    let name = format!("mdctest{}", uuid::Uuid::new_v4().simple());
    let database = Database::from_env(name, "main".into()).unwrap();
    database.initialize().await.unwrap();
    let mut snapshot = database.load().await.unwrap();
    let a = Node::new("A".into()).unwrap();
    let mut b = Node::new("B".into()).unwrap();
    b.depens.push(a.fnode.clone());
    snapshot.validate_changes(&[a.clone(), b.clone()]).unwrap();
    let old_version = snapshot.version.clone();
    let version = database
        .put(&[a.clone(), b.clone()], &old_version, "Create linked nodes")
        .await
        .unwrap();
    snapshot.apply(vec![a.clone(), b.clone()], version);
    assert_eq!(database.load().await.unwrap().nodes[&b.fnode], b);
    let mut changed = a.clone();
    changed.title = "Changed".into();
    assert!(database
        .put(&[changed], &old_version, "Stale write")
        .await
        .is_err());
    assert_eq!(database.load().await.unwrap().nodes[&a.fnode], a);
    assert_eq!(database.version().await.unwrap(), snapshot.version);
    database.create_branch("agent").await.unwrap();
    assert!(database.history().await.unwrap().as_array().unwrap().len() >= 3);
}
