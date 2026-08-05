use std::collections::HashMap;
use std::fs;
use std::path::Path;

use mathdoc::core::{FormalCodeStatus, FormalizationStatus};
use mathdoc::indcache::IndCache;
use mathdoc::mdocnode::{MdocNode, SrcBlock};

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn formal_node(root: &Path, relative: &str) -> MdocNode {
    let mut node = MdocNode::new_at_path(&root.join(relative), "Formal status");
    for srctype in ["lean", "rocq"] {
        node.blocks.push(SrcBlock {
            srctype: srctype.to_string(),
            content: "formal source\n".to_string(),
            metadata: HashMap::new(),
        });
    }
    write(&node.path, &node.render().unwrap());
    node
}

#[test]
fn cache_reports_no_code_without_formal_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join(".mdc")).unwrap();
    let node = MdocNode::new_at_path(&root.join("plain.mdoc"), "Plain node");
    write(&node.path, &node.render().unwrap());

    let cache = IndCache::open(root.to_path_buf()).unwrap();

    assert_eq!(
        cache.formalization_status(&node.fnode).unwrap(),
        FormalizationStatus::default()
    );
}

#[test]
fn artifacts_without_work_attestations_stay_unverified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join(".mdc")).unwrap();
    let node = formal_node(root, "notes/card.mdoc");
    let lean_source = root.join(".mdc/lean/Lib/notes/card.lean");
    let rocq_source = root.join(".mdc/rocq/Lib/notes/card.v");
    write(&lean_source, "formal source\n");
    write(&rocq_source, "formal source\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(
        cache.formalization_status(&node.fnode).unwrap(),
        FormalizationStatus {
            lean: FormalCodeStatus::Unverified,
            rocq: FormalCodeStatus::Unverified,
        }
    );

    let lean_artifact = root.join(".mdc/lean/.lake/build/lib/lean/Lib/notes/card.olean");
    let rocq_artifact = root.join(".mdc/rocq/build/notes/card.vo");
    write(&lean_artifact, "olean");
    write(&rocq_artifact, "vo");
    cache.refresh_all().unwrap();
    assert_eq!(
        cache.formalization_status(&node.fnode).unwrap(),
        FormalizationStatus {
            lean: FormalCodeStatus::Unverified,
            rocq: FormalCodeStatus::Unverified,
        }
    );
}

#[test]
fn focused_upsert_never_promotes_unattested_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join(".mdc")).unwrap();
    let node = formal_node(root, "card.mdoc");
    let lean_source = root.join(".mdc/lean/Lib/card.lean");
    write(&lean_source, "formal source\n");

    let mut cache = IndCache::open(root.to_path_buf()).unwrap();
    assert_eq!(
        cache.formalization_status(&node.fnode).unwrap().lean,
        FormalCodeStatus::Unverified
    );

    write(
        &root.join(".mdc/lean/.lake/build/lib/lean/Lib/card.olean"),
        "olean",
    );
    cache.upsert_path(&node.path).unwrap();

    assert_eq!(
        cache.formalization_status(&node.fnode).unwrap().lean,
        FormalCodeStatus::Unverified
    );

    let mut edited = MdocNode::load(&node.path).unwrap();
    edited
        .blocks
        .iter_mut()
        .find(|block| block.srctype == "lean")
        .unwrap()
        .content = "changed source\n".to_string();
    write(&edited.path, &edited.render().unwrap());
    cache.upsert_path(&edited.path).unwrap();
    assert_eq!(
        cache.formalization_status(&node.fnode).unwrap().lean,
        FormalCodeStatus::Unverified
    );

    edited
        .blocks
        .iter_mut()
        .find(|block| block.srctype == "lean")
        .unwrap()
        .content = "formal source\n".to_string();
    write(&edited.path, &edited.render().unwrap());
    cache.upsert_path(&edited.path).unwrap();
    assert_eq!(
        cache.formalization_status(&node.fnode).unwrap().lean,
        FormalCodeStatus::Unverified
    );
}
