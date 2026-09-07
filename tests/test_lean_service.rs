use mathdoc::{
    lean::{Input, LeanService},
    store::{Block, LeanProject, Node, Snapshot},
};
use std::collections::HashMap;

#[tokio::test]
#[ignore = "requires native Lean v4.33.1 and Lake"]
async fn native_lean_incremental_diagnostics_goals_and_imports() {
    let temp = tempfile::tempdir().unwrap();
    let service = LeanService::new(temp.path().to_path_buf()).unwrap();
    let mut a = Node::new("A".into()).unwrap();
    a.blocks.push(Block {
        srctype: "lean".into(),
        content: "theorem exampleA : True := by\n  trivial\n".into(),
        ..Default::default()
    });
    let mut snapshot = Snapshot {
        version: "test".into(),
        nodes: [(a.fnode.clone(), a.clone())].into(),
        project: LeanProject::default(),
        depths: HashMap::new(),
        lean_keys: HashMap::new(),
        referrers: HashMap::new(),
    };
    snapshot.recompute();
    let check = service
        .check(Input::capture(&snapshot, &a.fnode).unwrap(), false)
        .await
        .unwrap();
    eprintln!("cold check: {} ms", check.elapsed_ms);
    assert!(check.passed && check.certified, "{check:?}");
    let repeat = service
        .check(Input::capture(&snapshot, &a.fnode).unwrap(), false)
        .await
        .unwrap();
    assert!(repeat.cache_hit);
    let goals = service
        .goals(Input::capture(&snapshot, &a.fnode).unwrap(), 1, 2)
        .await
        .unwrap();
    assert!(!goals.is_null(), "expected goals at the tactic");
    a.blocks[0].content = "theorem exampleA : True := by\n  exact 42\n".into();
    snapshot.apply(vec![a.clone()], "bad".into());
    let bad = service
        .check(Input::capture(&snapshot, &a.fnode).unwrap(), false)
        .await
        .unwrap();
    eprintln!("changed proof check: {} ms", bad.elapsed_ms);
    assert!(!bad.passed && !bad.diagnostics.is_empty(), "{bad:?}");
    a.blocks[0].content = "theorem exampleA : True := by\n  constructor\n".into();
    snapshot.apply(vec![a.clone()], "fixed".into());
    let fixed = service
        .check(Input::capture(&snapshot, &a.fnode).unwrap(), true)
        .await
        .unwrap();
    assert!(fixed.certified && fixed.built, "{fixed:?}");
    let mut b = Node::new("B".into()).unwrap();
    b.depens.push(a.fnode.clone());
    b.blocks.push(Block {
        srctype: "lean".into(),
        content: format!("import {}\ntheorem exampleB : True := exampleA\n", a.module),
        ..Default::default()
    });
    snapshot.apply(vec![b.clone()], "dependency".into());
    let checked = service
        .check(Input::capture(&snapshot, &b.fnode).unwrap(), false)
        .await
        .unwrap();
    assert!(checked.certified, "{checked:?}");
    assert!(
        checked
            .imports
            .iter()
            .any(|i| i["module"]["name"] == a.module),
        "Lean Server must report the managed import"
    );
    let stable_key = snapshot.lean_keys[&b.fnode].clone();
    a.title = "Renamed A".into();
    a.blocks.push(Block {
        srctype: "text".into(),
        content: "An explanation".into(),
        ..Default::default()
    });
    snapshot.apply(vec![a.clone()], "text".into());
    assert_eq!(snapshot.lean_keys[&b.fnode], stable_key);
    assert!(
        service
            .check(Input::capture(&snapshot, &b.fnode).unwrap(), false)
            .await
            .unwrap()
            .cache_hit
    );
    a.blocks[0].content = "theorem exampleA : True := by exact 42\n".into();
    snapshot.apply(vec![a.clone()], "upstream".into());
    assert_ne!(snapshot.lean_keys[&b.fnode], stable_key);
    let invalidated = service
        .check(Input::capture(&snapshot, &b.fnode).unwrap(), false)
        .await
        .unwrap();
    assert!(
        !invalidated.certified && !invalidated.cache_hit,
        "{invalidated:?}"
    );
    a.blocks[0].content = "theorem exampleA : True := by trivial\n".into();
    snapshot.apply(vec![a.clone()], "restore".into());
    assert!(
        service
            .check(Input::capture(&snapshot, &b.fnode).unwrap(), false)
            .await
            .unwrap()
            .certified
    );
    // Cached results survive environment switches; goals must reopen the exact document.
    let previous = snapshot.project.clone();
    snapshot
        .project
        .lakefile
        .push_str("\n# another environment\n");
    snapshot.recompute();
    service
        .check(Input::capture(&snapshot, &a.fnode).unwrap(), false)
        .await
        .unwrap();
    snapshot.project = previous;
    snapshot.recompute();
    let goal = service
        .goals(Input::capture(&snapshot, &a.fnode).unwrap(), 0, 31)
        .await
        .unwrap();
    assert!(!goal.is_null());
    b.blocks[0].content = "theorem exampleB : True := by trivial\n".into();
    snapshot.apply(vec![b.clone()], "mismatch".into());
    let mismatch = service
        .check(Input::capture(&snapshot, &b.fnode).unwrap(), false)
        .await
        .unwrap();
    assert!(mismatch.passed && !mismatch.certified, "{mismatch:?}");
    assert!(mismatch
        .dependency_errors
        .iter()
        .any(|e| e.contains("exactly match"))); // Opening many unrelated nodes must retire idle workers without losing valid certificates.
    for i in 0..6 {
        let mut n = Node::new(format!("Extra {i}")).unwrap();
        n.blocks.push(Block {
            srctype: "lean".into(),
            content: format!("theorem extra{i} : True := by trivial\n"),
            ..Default::default()
        });
        snapshot.apply(vec![n.clone()], format!("extra{i}"));
        assert!(
            service
                .check(Input::capture(&snapshot, &n.fnode).unwrap(), false)
                .await
                .unwrap()
                .certified
        );
    }
    assert!(!service
        .goals(Input::capture(&snapshot, &a.fnode).unwrap(), 0, 31)
        .await
        .unwrap()
        .is_null());
    let current = Input::capture(&snapshot, &a.fnode).unwrap();
    assert!(
        LeanService::new(temp.path().to_path_buf()).is_err(),
        "two services must not race over Lake artifacts"
    );
    drop(service);
    let restarted = LeanService::new(temp.path().to_path_buf()).unwrap();
    let restored = restarted.check(current, false).await.unwrap();
    assert!(
        restored.certified && restored.cache_hit,
        "restart must reuse input-matched persisted certification"
    );
}
