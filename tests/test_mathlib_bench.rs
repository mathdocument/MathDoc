//! Opt-in real-source microbenchmarks. No database or Lean compilation required.
use mathdoc::{
    lean::{refresh_editor_sources, Input, LeanService},
    service::Import,
    store::Snapshot,
};
use serde_json::json;
use std::{collections::HashMap, time::Instant};

#[tokio::test]
#[ignore = "set MDC_BENCH_BUNDLE and MDC_BENCH_REPORT to external file paths"]
async fn mathlib_snapshot_and_editor_preparation() {
    let bundle: Import =
        serde_json::from_slice(&std::fs::read(std::env::var("MDC_BENCH_BUNDLE").unwrap()).unwrap())
            .unwrap();
    let mut snapshot = Snapshot {
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
    };
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
