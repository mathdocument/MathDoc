//! Contended API latency through the production router, without TCP or browser costs.
use anyhow::{bail, Result};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
    Router,
};
use mathdoc::{
    indcache::WorkspaceStore,
    web::{server::router, AppState},
};
use std::process::Command;
use std::{collections::BTreeMap, fs, sync::Arc, time::Instant};
use tower::ServiceExt;
const NODES: usize = 1_000;
const READERS: usize = 16;
const LARGE_NODES: usize = 10_000;
const LARGE_EDGES: usize = (LARGE_NODES - 1) + (LARGE_NODES - 6);
fn fixture(large: bool) -> Result<(tempfile::TempDir, WorkspaceStore)> {
    let dir = tempfile::tempdir()?;
    mathdoc::workspace::initialize(dir.path())?;
    let count = if large { LARGE_NODES } else { NODES };
    for i in 0..count {
        let mut text = format!("@fnode: node-{i}\n@title: node-{i}\n");
        if large && i > 0 {
            text.push_str(&format!("\n@dep:\nnode-{}\n", (i - 1) / 2));
            if i >= 6 {
                text.push_str(&format!("node-{}\n", (i - 3) / 3));
            }
            text.push_str("@end\n");
        }
        fs::write(dir.path().join(format!("node-{i}.mdoc")), text)?;
    }
    let store = WorkspaceStore::open(dir.path().to_path_buf())?;
    if store.count()? as usize != count {
        bail!("incorrect fixture count");
    }
    Ok((dir, store))
}
async fn request(app: Router, req: Request<Body>) -> Result<serde_json::Value> {
    let response = app.oneshot(req).await?;
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024 * 1024).await?;
    if !status.is_success() {
        bail!(
            "request failed: {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(serde_json::from_slice(&body)?)
}
fn get(path: &str) -> Result<Request<Body>> {
    Ok(Request::builder()
        .uri(path)
        .header("host", "localhost")
        .body(Body::empty())?)
}
async fn burst(write: bool, large: bool) -> Result<f64> {
    let (_dir, store) = fixture(large)?;
    let nodes = if large { LARGE_NODES } else { NODES };
    let edges = if large { LARGE_EDGES } else { 0 };
    let app = router(AppState::new(store));
    let view = request(app.clone(), get("/api/node/node-0/view")?).await?;
    let revision = view["node"]["revision"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing revision"))?;
    // Both routes execute once before the timed burst, warming query and serialization paths.
    let search = request(app.clone(), get("/api/search?q=node&n=200")?).await?;
    let graph = request(app.clone(), get("/api/graph/full")?).await?;
    if search.as_array().map(Vec::len) != Some(200)
        || graph["nodes"].as_array().map(Vec::len) != Some(nodes)
        || graph["edges"].as_array().map(Vec::len) != Some(edges)
    {
        bail!("incorrect warmed response");
    }
    let count = READERS + usize::from(write);
    let barrier = Arc::new(tokio::sync::Barrier::new(count));
    let mut jobs = Vec::new();
    for i in 0..count {
        let req = if i == READERS {
            Request::builder()
                .method(Method::PUT)
                .uri("/api/node/node-0/title")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("if-match", format!("\"{revision}\""))
                .body(Body::from(r#"{"title":"Updated root"}"#))?
        } else {
            get(if !large && i % 2 == 0 {
                "/api/search?q=node&n=200"
            } else {
                "/api/graph/full"
            })?
        };
        let app = app.clone();
        let barrier = barrier.clone();
        jobs.push(tokio::spawn(async move {
            barrier.wait().await;
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
    let checked = request(app.clone(), get("/api/graph/check")?).await?;
    if checked["nodes"] != nodes
        || checked["edges"] != edges
        || checked["cycles"] != serde_json::json!([])
    {
        bail!("incorrect graph");
    }
    if write {
        let view = request(app, get("/api/node/node-0/view")?).await?;
        if view["node"]["title"] != "Updated root" {
            bail!("write was not applied");
        }
    }
    Ok(latencies[(latencies.len() as f64 * 0.95).ceil() as usize - 1])
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema: u32,
    environment: BTreeMap<String, String>,
    fixtures: BTreeMap<String, usize>,
    metrics_ms: BTreeMap<String, f64>,
    samples_ms: BTreeMap<String, Vec<f64>>,
}

#[derive(serde::Deserialize)]
struct Budgets {
    schema: u32,
    metrics: BTreeMap<String, Budget>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Budget {
    max_increase_ratio: f64,
    noise: f64,
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn compare(before: &Report, after: &Report, budgets: &Budgets) -> Result<()> {
    if before.schema != after.schema
        || budgets.schema != after.schema
        || before.fixtures != after.fixtures
        || before.environment != after.environment
        || !before.metrics_ms.keys().eq(after.metrics_ms.keys())
        || !after.metrics_ms.keys().eq(budgets.metrics.keys())
    {
        bail!("incompatible API reports or budgets (schema, environment, fixtures, or metrics differ)");
    }
    let mut failures = Vec::new();
    for (name, value) in &after.metrics_ms {
        let base = before.metrics_ms[name];
        let budget = &budgets.metrics[name];
        let limit = (base * (1.0 + budget.max_increase_ratio)).max(base + budget.noise);
        println!("{name}: baseline {base:.3}, current {value:.3}, limit {limit:.3} ms");
        if !value.is_finite() || *value > limit {
            failures.push(name.as_str());
        }
    }
    if !failures.is_empty() {
        bail!("API performance regression: {}", failures.join(", "));
    }
    Ok(())
}

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut output = root.join("perf/api-latest.json");
    let mut comparison = None;
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
                comparison = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("missing comparison path"))?,
                )
            }
            _ => bail!("unknown argument: {arg}"),
        }
    }
    let sample_count = std::env::var("MDC_PERF_SAMPLES").map_or(Ok(5), |s| s.parse::<usize>())?;
    if sample_count < 3 {
        bail!("MDC_PERF_SAMPLES must be at least 3");
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let scenarios = [
        ("api.readBurstP95", false, false),
        ("api.mixedBurstP95", true, false),
        ("api.largeGraphBurstP95", false, true),
    ];
    for (_, write, large) in scenarios {
        runtime.block_on(burst(write, large))?;
    }
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    for _ in 0..sample_count {
        for (name, write, large) in scenarios {
            samples
                .entry(name.into())
                .or_default()
                .push(runtime.block_on(burst(write, large))?);
        }
    }
    let report = Report {
        schema: 1,
        environment: BTreeMap::from([
            ("os".into(), std::env::consts::OS.into()),
            ("arch".into(), std::env::consts::ARCH.into()),
            ("cpu".into(), cpu_model()),
            ("rustc".into(), command_output("rustc", &["--version"])),
            (
                "workers".into(),
                std::thread::available_parallelism()?.to_string(),
            ),
        ]),
        fixtures: BTreeMap::from([
            ("nodes".into(), NODES),
            ("edges".into(), 0),
            ("largeNodes".into(), LARGE_NODES),
            ("largeEdges".into(), LARGE_EDGES),
            ("readers".into(), READERS),
            ("searchResults".into(), 200),
            ("samples".into(), sample_count),
        ]),
        metrics_ms: samples
            .iter()
            .map(|(name, values)| (name.clone(), median(values)))
            .collect(),
        samples_ms: samples,
    };
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_string_pretty(&report)? + "\n")?;
    for (name, value) in &report.metrics_ms {
        println!("{name}: {value:.3} ms");
    }
    println!("Report: {}", output.display());
    if let Some(path) = comparison {
        let before: Report = serde_json::from_slice(&fs::read(path)?)?;
        let budgets: Budgets =
            serde_json::from_slice(&fs::read(root.join("perf/api-budgets.json"))?)?;
        compare(&before, &report, &budgets)?;
    }
    Ok(())
}

fn cpu_model() -> String {
    if cfg!(target_os = "macos") {
        return command_output("sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    if cfg!(target_os = "linux") {
        if let Ok(contents) = fs::read_to_string("/proc/cpuinfo") {
            if let Some(model) = contents.lines().find_map(|line| {
                line.strip_prefix("model name")
                    .and_then(|value| value.split_once(':'))
                    .map(|(_, value)| value.trim().to_string())
            }) {
                return model;
            }
        }
    }
    "unknown".to_string()
}

fn command_output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|output| !output.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
