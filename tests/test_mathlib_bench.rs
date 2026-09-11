//! Opt-in real-source benchmarks. Runtime data stays in external directories.
use mathdoc::{
    lean::{refresh_editor_sources, Input, LeanService},
    service::Import,
    store::Snapshot,
};
use serde_json::json;
use std::{collections::HashMap, time::Instant};

fn load_snapshot() -> Snapshot {
    let bundle: Import =
        serde_json::from_slice(&std::fs::read(std::env::var("MDC_BENCH_BUNDLE").unwrap()).unwrap())
            .unwrap();
    Snapshot {
        version: "bench".into(),
        nodes: bundle
            .nodes
            .into_iter()
            .map(|n| (n.fnode.clone(), n.into()))
            .collect(),
        project: bundle.project.into(),
        project_key: String::new(),
        modules: Default::default(),
        lean_prefixes: HashMap::new(),
        depths: HashMap::new(),
        lean_keys: HashMap::new(),
        referrers: HashMap::new(),
    }
}

#[tokio::test]
#[ignore = "set MDC_BENCH_BUNDLE and MDC_BENCH_REPORT to external file paths; no compilation"]
async fn mathlib_snapshot_and_editor_preparation() {
    let mut snapshot = load_snapshot();
    let start = Instant::now();
    snapshot.recompute();
    let recompute = start.elapsed().as_secs_f64() * 1000.0;
    let mut timings = vec![];
    let cache = tempfile::tempdir().unwrap();
    let service = LeanService::new(cache.path().to_path_buf()).unwrap();
    for module in [
        "Mathlib.Tactic.Lemma",
        "Mathlib.Data.Nat.Log",
        "Mathlib.Analysis.SpecialFunctions.Log.Basic",
        "Mathlib",
    ] {
        let id = snapshot
            .nodes
            .values()
            .find(|n| n.module == module)
            .unwrap()
            .fnode
            .clone();
        let mut samples = vec![];
        for _ in 0..10 {
            let start = Instant::now();
            std::hint::black_box(Input::capture(&snapshot, &id).unwrap());
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let input = Input::capture(&snapshot, &id).unwrap();
        let start = Instant::now();
        let draft = service.editor_project(&input).await.unwrap();
        let create_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut refresh = vec![];
        let mut sources = input
            .chain
            .iter()
            .map(|(n, _)| (n.fnode.clone(), n.clone()))
            .collect();
        for _ in 0..3 {
            let start = Instant::now();
            refresh_editor_sources(draft.path(), &input, &mut sources)
                .await
                .unwrap();
            refresh.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        timings.push(json!({"module":module,"closure_nodes":input.chain.len(),"capture_ms":samples,"draft_create_ms":create_ms,"unchanged_refresh_ms":refresh}));
    }
    let mut node = snapshot
        .nodes
        .values()
        .find(|n| n.module == "Mathlib.Init")
        .unwrap()
        .as_ref()
        .clone();
    node.blocks
        .iter_mut()
        .find(|b| b.srctype == "lean")
        .unwrap()
        .content
        .push_str("\n-- benchmark\n");
    let start = Instant::now();
    snapshot.apply(vec![node], "edit".into());
    let invalidate = start.elapsed().as_secs_f64() * 1000.0;
    let report = json!({"nodes":snapshot.nodes.len(),"recompute_ms":recompute,"edit_mathlib_init_ms":invalidate,"preparation":timings});
    std::fs::write(
        std::env::var("MDC_BENCH_REPORT").unwrap(),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{report}");
}

/// Compare the same source edits with independent, locally built artifacts.
/// Toolchain installation and pinned dependency source downloads are untimed.
#[tokio::test]
#[ignore = "requires pinned Lean; set MDC_BENCH_BUNDLE and an unused MDC_BENCH_RUN_DIR"]
async fn mathlib_lake_and_mdc_incremental_builds() {
    use mathdoc::lean::{module_path, prepare_project};
    use std::path::{Path, PathBuf};
    async fn lake(root: &Path, args: &[&str]) -> f64 {
        let start = Instant::now();
        eprintln!("Lake {}: {}", root.display(), args.join(" "));
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(1800),
            mathdoc::lean::command(root, args).output(),
        )
        .await
        .expect("native benchmark command timed out")
        .unwrap();
        assert!(
            output.status.success(),
            "Lake failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        start.elapsed().as_secs_f64() * 1000.0
    }
    std::env::set_var("LAKE_NO_CACHE", "true");
    std::env::set_var("MATHLIB_NO_CACHE_ON_UPDATE", "1");
    let root = PathBuf::from(std::env::var("MDC_BENCH_RUN_DIR").unwrap());
    std::fs::create_dir(&root).expect("use a new, empty run directory for cold measurements");
    let mut snapshot = load_snapshot();
    snapshot.recompute();
    let module = std::env::var("MDC_BENCH_MODULE").unwrap_or("Mathlib.Data.Nat.Log".into());
    let mut node = snapshot
        .nodes
        .values()
        .find(|n| n.module == module)
        .unwrap()
        .as_ref()
        .clone();
    let input = Input::capture(&snapshot, &node.fnode).unwrap();
    let native = root.join("lake");
    let cache = root.join("mdc");
    let managed = cache.join("projects").join(&snapshot.project_key);
    for directory in [&native, &managed] {
        prepare_project(directory, &snapshot.project).await.unwrap();
        // Keep the original complete source tree on both sides.
        for node in snapshot.nodes.values() {
            let path = module_path(directory, &node.module).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, node.source("lean").unwrap()).unwrap();
        }
        lake(directory, &["env", "lean", "--version"]).await;
    }
    let service = LeanService::new(cache).unwrap();
    let mut report = json!({"module":module,"closure_nodes":input.chain.len(),"official_cache":false,"lake":{},"mdc":{}});
    let target = format!("+{module}");
    for phase in ["cold", "unchanged", "target_edit", "dependency_edit"] {
        if phase == "target_edit" || phase == "dependency_edit" {
            let changed = if phase == "target_edit" {
                &mut node
            } else {
                &mut snapshot
                    .nodes
                    .values()
                    .find(|n| n.module == "Mathlib.Init")
                    .unwrap()
                    .as_ref()
                    .clone()
            };
            changed
                .blocks
                .iter_mut()
                .find(|b| b.srctype == "lean")
                .unwrap()
                .content
                .push_str("\n-- incremental benchmark edit\n");
            std::fs::write(
                module_path(&native, &changed.module).unwrap(),
                changed.source("lean").unwrap(),
            )
            .unwrap();
            snapshot.apply(vec![changed.clone()], phase.into());
        }
        let native_ms = lake(&native, &["build", &target]).await;
        report["lake"][phase] = json!({"elapsed_ms":native_ms});
        let start = Instant::now();
        let checked = service
            .check(Input::capture(&snapshot, &node.fnode).unwrap(), true)
            .await
            .unwrap();
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        report["mdc"][phase] = json!({"elapsed_ms":elapsed_ms,"result":checked});
        std::fs::write(
            root.join("result.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        assert!(checked.certified && checked.built, "{checked:?}");
        assert_eq!(checked.has_sorry, Some(false));
        assert_eq!(checked.cache_hit, phase == "unchanged");
        eprintln!("{module} {phase}: lake={native_ms:.2} ms mdc={elapsed_ms:.2} ms");
    }
    service.shutdown().await;
}
