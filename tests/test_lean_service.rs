use mathdoc::{
    lean::{Input, LeanService},
    store::{Block, LeanProject, Node, Snapshot},
};
use std::collections::HashMap;

#[tokio::test]
#[ignore = "requires native Lean and Git; uses a local pinned external library"]
async fn pinned_external_library_builds_and_reuses_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    std::fs::create_dir(&library).unwrap();
    std::fs::write(
        library.join("lakefile.toml"),
        "name = \"fixture\"\n[[lean_lib]]\nname = \"ExternalFixture\"\n",
    )
    .unwrap();
    std::fs::write(
        library.join("ExternalFixture.lean"),
        "theorem externalTruth : True := by trivial\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&library)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "--quiet"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "test: add pinned library fixture",
    ]);
    let revision = git(&["rev-parse", "HEAD"]);
    let url = mathdoc::lean::file_uri(&library).unwrap();
    let mut project = LeanProject::default();
    project.lakefile.push_str(&format!(
        "\n[[require]]\nname = \"fixture\"\ngit = \"{url}\"\nrev = \"{revision}\"\n"
    ));
    project.manifest=Some(serde_json::json!({"version":"1.1.0","name":"MathDoc","lakeDir":".lake","packagesDir":".lake/packages","packages":[{"name":"fixture","type":"git","url":url,"rev":revision,"inputRev":revision,"scope":"","subDir":null,"manifestFile":"lake-manifest.json","configFile":"lakefile.toml","inherited":false}]}).to_string());
    project.validate().unwrap();
    let mut node = Node::new("External library user".into()).unwrap();
    node.blocks.push(Block {
        srctype: "lean".into(),
        content: "import ExternalFixture\ntheorem usesLibrary : True := externalTruth\n".into(),
        ..Default::default()
    });
    let mut snapshot = Snapshot {
        version: "library".into(),
        nodes: [(node.fnode.clone(), node.clone())].into(),
        project,
        depths: HashMap::new(),
        lean_keys: HashMap::new(),
        referrers: HashMap::new(),
    };
    snapshot.recompute();
    let cache = temp.path().join("cache");
    let service = LeanService::new(cache.clone()).unwrap();
    let result = service
        .check(Input::capture(&snapshot, &node.fnode).unwrap(), true)
        .await
        .unwrap();
    assert!(result.certified && result.built, "{result:?}");
    let artifact = cache
        .join("projects")
        .join(snapshot.project.key())
        .join(".lake/packages/fixture/.lake/build/lib/lean/ExternalFixture.olean");
    let modified = std::fs::metadata(&artifact).unwrap().modified().unwrap();
    node.blocks[0].content.push_str("\n-- local proof edit\n");
    snapshot.apply(vec![node.clone()], "edit".into());
    assert!(
        service
            .check(Input::capture(&snapshot, &node.fnode).unwrap(), true)
            .await
            .unwrap()
            .certified
    );
    assert_eq!(
        std::fs::metadata(artifact).unwrap().modified().unwrap(),
        modified,
        "unchanged external library should not rebuild"
    );
}

#[tokio::test]
#[ignore = "requires native Lean v4.33.1 and Lake"]
async fn native_lean_incremental_diagnostics_goals_and_imports() {
    let temp = tempfile::tempdir().unwrap();
    let service = LeanService::new(temp.path().to_path_buf()).unwrap();
    let mut a = Node::new("A".into()).unwrap();
    a.module = "Lib.EGA.«1-1.7.1»".into();
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
    a.module = "Lib.EGA.«1-1.7.1_Moved»".into();
    snapshot.apply(vec![a.clone()], "module moved".into());
    assert!(
        service
            .check(Input::capture(&snapshot, &a.fnode).unwrap(), false)
            .await
            .unwrap()
            .certified
    );
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
