use mathdoc::{
    lean::{Input, LeanService},
    store::{Block, LeanProject, Node, Snapshot},
};
use std::collections::HashMap;

#[tokio::test]
#[ignore = "requires native Lean and Lake"]
async fn native_project_keeps_module_names_configuration_and_dependency_guards() {
    let temp = tempfile::tempdir().unwrap();
    let project = LeanProject {
        lakefile_name: Some("lakefile.lean".into()),
        module_root: Some("Mathlib".into()),
        lakefile: "import Lake\nopen Lake DSL\npackage fixture where\n  restoreAllArtifacts := true\nlean_lib Mathlib\nlean_lib Support\n".into(),
        manifest: Some("{\"version\":\"1.1.0\",\"name\":\"fixture\",\"lakeDir\":\".lake\",\"packagesDir\":\".lake/packages\",\"packages\":[]}".into()),
        files: [("Support.lean".into(), "def supportValue : Nat := 7\n".into())].into(),
        ..Default::default()
    };
    let mut a = Node::new("A".into()).unwrap();
    a.module = "Mathlib.A".into();
    let executions = temp.path().join("dependency-executions");
    a.blocks.push(Block {
        srctype: "lean".into(),
        content: format!("import Lean\nimport Support\nrun_cmd Lean.Elab.Command.liftIO <| do\n  let file ← IO.FS.Handle.mk {} .append\n  file.putStr \"compiled\\n\"\ntheorem base : supportValue = 7 := rfl\n", serde_json::to_string(&executions.to_string_lossy()).unwrap()),
        ..Default::default()
    });
    let mut b = Node::new("B".into()).unwrap();
    b.module = "Mathlib.B".into();
    b.depens.push(a.fnode.clone());
    b.blocks.push(Block {
        srctype: "lean".into(),
        content: "import Mathlib.A\ntheorem derived : supportValue = 7 := base\n".into(),
        ..Default::default()
    });
    let deleted_id = a.fnode.clone();
    let mut snapshot = Snapshot {
        version: "native".into(),
        nodes: [a, b.clone()]
            .into_iter()
            .map(|n| (n.fnode.clone(), n.into()))
            .collect(),
        project: project.into(),
        latex_project: Default::default(),
        project_key: String::new(),
        latex_project_key: String::new(),
        modules: Default::default(),
        lean_prefixes: HashMap::new(),
        depths: HashMap::new(),
        lean_keys: HashMap::new(),
        referrers: HashMap::new(),
    };
    snapshot.recompute();
    let service = LeanService::new(temp.path().to_path_buf()).unwrap();
    let checked = service
        .check(Input::capture(&snapshot, &b.fnode).unwrap(), true)
        .await
        .unwrap();
    assert!(checked.certified && checked.built, "{checked:?}");
    assert_eq!(
        std::fs::read_to_string(&executions).unwrap(),
        "compiled\n",
        "CLI must use the dependency compiled by Lake without elaborating it in another LSP worker"
    );
    let root = temp.path().join("projects").join(snapshot.project.key());
    assert_eq!(
        std::fs::read_to_string(root.join("lakefile.lean")).unwrap(),
        snapshot.project.lakefile
    );
    assert!(!root.join("lakefile.toml").exists());
    b.depens.clear();
    snapshot.apply(vec![b.clone()], "missing edge".into());
    let checked = service
        .check(Input::capture(&snapshot, &b.fnode).unwrap(), false)
        .await
        .unwrap();
    assert!(checked.passed && !checked.certified, "missing managed edge must be detected even when a previous build left the imported module on disk: {checked:?}");
    snapshot.remove(&deleted_id, "delete dependency".into());
    service.shutdown().await;
    drop(service);
    // Restart with the old on-disk artifacts still present. They must never
    // turn a deleted node into an implicitly trusted external dependency.
    let service = LeanService::new(temp.path().to_path_buf()).unwrap();
    let checked = service
        .check(Input::capture(&snapshot, &b.fnode).unwrap(), false)
        .await
        .unwrap();
    assert!(!checked.certified, "{checked:?}");
    assert!(
        checked
            .dependency_errors
            .iter()
            .any(|e| e.contains("no longer a graph node")),
        "{checked:?}"
    );
    service.shutdown().await;
}

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
        nodes: [(node.fnode.clone(), node.clone().into())].into(),
        project: project.into(),
        latex_project: Default::default(),
        project_key: String::new(),
        latex_project_key: String::new(),
        modules: Default::default(),
        lean_prefixes: HashMap::new(),
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
        std::fs::metadata(&artifact).unwrap().modified().unwrap(),
        modified,
        "unchanged external library should not rebuild"
    );
    let draft = service
        .editor_project(&Input::capture(&snapshot, &node.fnode).unwrap())
        .await
        .unwrap();
    assert!(draft.path().join(".lake/packages").is_symlink());
    let mut lean = mathdoc::lean::spawn(
        draft.path(),
        &[
            "env",
            "lean",
            &mathdoc::store::module_file(&node.module, "lean")
                .unwrap()
                .to_string_lossy(),
        ],
    )
    .unwrap();
    assert!(lean.child.wait().await.unwrap().success());
    drop(draft);
    assert_eq!(
        std::fs::metadata(&artifact).unwrap().modified().unwrap(),
        modified
    );
    service.shutdown().await;
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
        nodes: [(a.fnode.clone(), a.clone().into())].into(),
        project: LeanProject::default().into(),
        latex_project: Default::default(),
        project_key: String::new(),
        latex_project_key: String::new(),
        modules: Default::default(),
        lean_prefixes: HashMap::new(),
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
    let artifact = temp
        .path()
        .join("projects")
        .join(snapshot.project.key())
        .join(".lake/build/lib/lean")
        .join(mathdoc::store::module_file(&a.module, "olean").unwrap());
    std::fs::remove_file(&artifact).unwrap();
    assert!(
        service
            .cached_or_load(
                &a,
                &snapshot.lean_keys[&a.fnode],
                &snapshot.project_key,
                true
            )
            .await
            .is_some(),
        "local outputs are disposable when shared objects survive"
    );
    let shared_object = temp
        .path()
        .join("projects")
        .join(&snapshot.project_key)
        .join(".lake/cache/artifacts")
        .join(&fixed.artifacts.as_ref().unwrap().parts[0]);
    std::fs::remove_file(shared_object).unwrap();
    assert!(service
        .cached_or_load(
            &a,
            &snapshot.lean_keys[&a.fnode],
            &snapshot.project_key,
            true
        )
        .await
        .is_none());
    let rebuilt = service
        .check(Input::capture(&snapshot, &a.fnode).unwrap(), true)
        .await
        .unwrap();
    assert!(rebuilt.built && !rebuilt.cache_hit && artifact.is_file());

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
    std::sync::Arc::make_mut(&mut snapshot.project)
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
    service.shutdown().await;
    drop(service);
    let restarted = LeanService::new(temp.path().to_path_buf()).unwrap();
    let restored = restarted.check(current, false).await.unwrap();
    assert!(
        restored.certified && restored.cache_hit,
        "restart must reuse input-matched persisted certification"
    );
}

#[tokio::test]
#[ignore = "requires native Lean and Lake"]
async fn branches_reuse_native_objects_after_the_producer_is_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join(".shared");
    let a = LeanService::with_shared_cache(tmp.path().join("a"), shared.clone()).unwrap();
    let b = LeanService::with_shared_cache(tmp.path().join("b"), shared.clone()).unwrap();
    let marker = tmp.path().join("compiled");
    let mut dep = Node::new("Shared dependency".into()).unwrap();
    dep.blocks.push(Block { srctype: "lean".into(), content: format!(
        "module\npublic import Lean\nrun_cmd Lean.Elab.Command.liftIO <| IO.FS.writeFile {} \"compiled\"\npublic theorem commonTruth : True := by trivial\n",
        serde_json::to_string(&marker.to_string_lossy()).unwrap()), ..Default::default() });
    let mut node = Node::new("Shared target".into()).unwrap();
    node.depens.push(dep.fnode.clone());
    node.blocks.push(Block {
        srctype: "lean".into(),
        content: format!(
            "import {}\ntheorem targetTruth : True := commonTruth\n",
            dep.module
        ),
        ..Default::default()
    });
    let mut snapshot = Snapshot {
        version: "fork".into(),
        nodes: [dep.clone(), node.clone()]
            .into_iter()
            .map(|n| (n.fnode.clone(), n.into()))
            .collect(),
        project: LeanProject::default().into(),
        latex_project: Default::default(),
        project_key: String::new(),
        latex_project_key: String::new(),
        modules: Default::default(),
        lean_prefixes: HashMap::new(),
        depths: HashMap::new(),
        lean_keys: HashMap::new(),
        referrers: HashMap::new(),
    };
    snapshot.recompute();
    let input = Input::capture(&snapshot, &node.fnode).unwrap();
    assert!(a.check(input.clone(), true).await.unwrap().built);
    assert!(marker.exists());
    std::fs::remove_file(&marker).unwrap();
    let result = b.check(input.clone(), true).await.unwrap();
    assert!(result.built && result.certified, "{result:?}");
    assert!(
        !marker.exists(),
        "a fork must reuse the compiled dependency"
    );
    let workspace = tmp.path().join("b/projects").join(&snapshot.project_key);
    assert!(!workspace
        .join(".lake/build/lib/lean")
        .join(mathdoc::store::module_file(&dep.module, "olean").unwrap())
        .exists());
    assert_eq!(
        b.formal_status(&dep, &snapshot.lean_keys[&dep.fnode]),
        "verified"
    );
    a.shutdown().await;
    drop(a);
    std::fs::remove_dir_all(tmp.path().join("a")).unwrap();
    assert!(b.check(input.clone(), true).await.unwrap().cache_hit);
    let draft = b.editor_project(&input).await.unwrap();
    assert!(
        !draft.path().join(".lake/build").exists(),
        "never copy a whole build tree"
    );
    assert!(draft.path().join(".lake/cache").is_symlink());
    b.shutdown().await;
}
