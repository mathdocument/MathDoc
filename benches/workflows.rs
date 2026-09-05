//! End-to-end application batches and contended API latency, on disposable workspaces.
use anyhow::{bail, Result};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
    Router,
};
use mathdoc::{
    application::nodes::{create_nodes, edit_nodes, NewNode, NodeChange, NodeEdit},
    indcache::WorkspaceStore,
    web::{server::router, AppState},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::Instant};
use tower::ServiceExt;

const NODES: usize = 1_000;
const CHANGES: usize = 25;
const SAMPLES: usize = 3;
const READERS: usize = 16;

#[derive(Serialize, Deserialize)]
struct Report {
    schema: u32,
    fixtures: [usize; 4],
    metrics_ms: BTreeMap<String, f64>,
    samples_ms: BTreeMap<String, Vec<f64>>,
}

fn fixture() -> Result<(tempfile::TempDir, WorkspaceStore)> {
    let dir = tempfile::tempdir()?;
    mathdoc::workspace::initialize(dir.path())?;
    let mut store = WorkspaceStore::open(dir.path().to_path_buf())?;
    let nodes = (0..NODES)
        .map(|i| seed(&format!("node-{i}")))
        .collect::<Vec<_>>();
    create_nodes(&mut store, &nodes)?;
    Ok((dir, store))
}

fn seed(name: &str) -> NewNode {
    NewNode {
        file: name.into(),
        title: name.into(),
        fnode: Some(name.into()),
    }
}

fn creation(batch: bool) -> Result<f64> {
    let (_dir, mut store) = fixture()?;
    let nodes = (0..CHANGES)
        .map(|i| seed(&format!("created-{i}")))
        .collect::<Vec<_>>();
    let start = Instant::now();
    if batch {
        create_nodes(&mut store, &nodes)?;
    } else {
        for node in &nodes {
            create_nodes(&mut store, std::slice::from_ref(node))?;
        }
    }
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    if store.count()? as usize != NODES + CHANGES {
        bail!("incorrect create count");
    }
    Ok(elapsed)
}

fn dependencies(batch: bool) -> Result<f64> {
    let (_dir, mut store) = fixture()?;
    let edits = (0..CHANGES)
        .map(|i| NodeEdit {
            reference: format!("node-{i}"),
            expected_revision: None,
            changes: vec![NodeChange::AddDependencies(vec![format!("node-{}", i + 1)])],
        })
        .collect::<Vec<_>>();
    let start = Instant::now();
    if batch {
        edit_nodes(&mut store, &edits, None)?;
    } else {
        for edit in &edits {
            edit_nodes(&mut store, std::slice::from_ref(edit), None)?;
        }
    }
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    if store.node_summary("node-0")?.depth as usize != CHANGES {
        bail!("incorrect batch graph depth");
    }
    Ok(elapsed)
}

async fn request(app: Router, request: Request<Body>) -> Result<serde_json::Value> {
    let response = app.oneshot(request).await?;
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024 * 1024).await?;
    if !status.is_success() {
        bail!(
            "API request failed: {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(serde_json::from_slice(&body)?)
}

async fn mixed_api() -> Result<f64> {
    let (_dir, store) = fixture()?;
    let app = router(AppState::new(store));
    let view = request(
        app.clone(),
        Request::builder()
            .uri("/api/node/node-0/view")
            .header("host", "localhost")
            .body(Body::empty())?,
    )
    .await?;
    let revision = view["node"]["revision"].as_str().unwrap();
    let mut jobs = Vec::new();
    for i in 0..=READERS {
        let builder = Request::builder().header("host", "localhost");
        let req = if i == READERS {
            builder
                .method(Method::PUT)
                .uri("/api/node/node-0/title")
                .header("content-type", "application/json")
                .header("if-match", format!("\"{revision}\""))
                .body(Body::from(r#"{"title":"Updated root"}"#))?
        } else {
            builder
                .uri(if i % 2 == 0 {
                    "/api/search?q=node&n=200"
                } else {
                    "/api/graph/full"
                })
                .body(Body::empty())?
        };
        let app = app.clone();
        jobs.push(tokio::spawn(async move {
            let start = Instant::now();
            request(app, req).await?;
            Ok::<_, anyhow::Error>(start.elapsed().as_secs_f64() * 1_000.0)
        }));
    }
    let mut latencies = Vec::new();
    for job in jobs {
        latencies.push(job.await??);
    }
    latencies.sort_by(f64::total_cmp);
    let p95 = latencies[(latencies.len() as f64 * 0.95).ceil() as usize - 1];
    let checked = request(
        app,
        Request::builder()
            .uri("/api/graph/check")
            .header("host", "localhost")
            .body(Body::empty())?,
    )
    .await?;
    if checked["nodes"] != NODES || checked["cycles"] != serde_json::json!([]) {
        bail!("mixed requests corrupted graph results");
    }
    Ok(p95)
}

fn main() -> Result<()> {
    let mut output = PathBuf::from("perf/workflows-latest.json");
    let mut compare = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bench" => {}
            "--output" => {
                output = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing output path"))?
                    .into()
            }
            "--compare" => {
                compare = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("missing comparison path"))?,
                ))
            }
            _ => bail!("unknown argument: {arg}"),
        }
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    for _ in 0..SAMPLES {
        for (name, value) in [
            ("create.sequential", creation(false)?),
            ("create.batch", creation(true)?),
            ("dependencies.sequential", dependencies(false)?),
            ("dependencies.batch", dependencies(true)?),
            ("api.mixedP95", runtime.block_on(mixed_api())?),
        ] {
            samples.entry(name.into()).or_default().push(value);
        }
    }
    let metrics_ms = samples
        .iter()
        .map(|(name, values)| {
            let mut sorted = values.clone();
            sorted.sort_by(f64::total_cmp);
            (name.clone(), sorted[sorted.len() / 2])
        })
        .collect();
    let report = Report {
        schema: 1,
        fixtures: [NODES, CHANGES, SAMPLES, READERS],
        metrics_ms,
        samples_ms: samples,
    };
    std::fs::write(&output, serde_json::to_string_pretty(&report)? + "\n")?;
    for (name, value) in &report.metrics_ms {
        println!("{name}: {value:.3} ms");
    }
    if let Some(path) = compare {
        let before: Report = serde_json::from_slice(&std::fs::read(path)?)?;
        if before.schema != report.schema
            || before.fixtures != report.fixtures
            || !before.metrics_ms.keys().eq(report.metrics_ms.keys())
        {
            bail!("incompatible workflow reports");
        }
        for (name, value) in &report.metrics_ms {
            let baseline = before.metrics_ms[name];
            let limit = (baseline * 1.20).max(baseline + 30.0);
            if *value > limit {
                bail!("{name}: {value:.3} ms exceeds {limit:.3} ms");
            }
        }
    }
    Ok(())
}
