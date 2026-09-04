use anyhow::{bail, Context, Result};
use mathdoc::indcache::WorkspaceStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const REPORT_SCHEMA: u32 = 1;
const NODE_COUNT: usize = 10_000;
const EDGE_COUNT: usize = (NODE_COUNT - 1) + (NODE_COUNT - 6);
const SEARCH_RESULTS: usize = 200;
const SEARCH_ITERATIONS: usize = 100;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema: u32,
    generated_at_unix_seconds: u64,
    environment: Environment,
    fixtures: Fixtures,
    metrics: BTreeMap<String, Metric>,
    raw_samples: BTreeMap<String, Vec<f64>>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Environment {
    os: String,
    arch: String,
    cpu: String,
    rustc: String,
    samples: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Fixtures {
    nodes: usize,
    edges: usize,
    search_results: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct Metric {
    value: f64,
    unit: String,
}

#[derive(Debug, Deserialize)]
struct Budgets {
    schema: u32,
    metrics: BTreeMap<String, Budget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Budget {
    max_increase_ratio: f64,
    noise: f64,
    max: Option<f64>,
}

struct Options {
    output: PathBuf,
    compare: Option<PathBuf>,
}

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let perf_dir = root.join("perf");
    let options = options(&perf_dir)?;
    let sample_count = std::env::var("MDC_PERF_SAMPLES")
        .map_or(Ok(5), |value| value.parse::<usize>())
        .context("MDC_PERF_SAMPLES must be an integer")?;
    if sample_count < 3 {
        bail!("MDC_PERF_SAMPLES must be at least 3");
    }

    let report = run(sample_count)?;
    if let Some(parent) = options.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &options.output,
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    print_metrics(&report);
    println!("Report: {}", options.output.display());

    if let Some(compare_path) = options.compare {
        let baseline: Report = read_json(&compare_path)?;
        let budgets: Budgets = read_json(&perf_dir.join("budgets.json"))?;
        compare(&baseline, &report, &budgets)?;
    }
    Ok(())
}

fn options(perf_dir: &Path) -> Result<Options> {
    let mut args = std::env::args().skip(1);
    let mut output = None;
    let mut compare = None;
    let mut record = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bench" => {}
            "--record" => record = true,
            "--output" => output = Some(PathBuf::from(next_value(&mut args, "--output")?)),
            "--compare" => compare = Some(PathBuf::from(next_value(&mut args, "--compare")?)),
            _ if arg.starts_with("--output=") => {
                output = Some(PathBuf::from(&arg["--output=".len()..]));
            }
            _ if arg.starts_with("--compare=") => {
                compare = Some(PathBuf::from(&arg["--compare=".len()..]));
            }
            _ => bail!("unknown benchmark argument: {arg}"),
        }
    }
    if record && (output.is_some() || compare.is_some()) {
        bail!("--record cannot be combined with --output or --compare");
    }
    let explicit_output = output.is_some();
    Ok(if record {
        Options {
            output: perf_dir.join("baseline.json"),
            compare: None,
        }
    } else {
        Options {
            output: output.unwrap_or_else(|| perf_dir.join("latest.json")),
            compare: compare.or_else(|| (!explicit_output).then(|| perf_dir.join("baseline.json"))),
        }
    })
}

fn next_value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String> {
    args.next()
        .with_context(|| format!("{option} requires a path"))
}

fn run(sample_count: usize) -> Result<Report> {
    let workspace = tempfile::tempdir()?;
    create_fixture(workspace.path())?;
    let mut store = WorkspaceStore::open(workspace.path().to_path_buf())?;
    validate_store(&mut store)?;

    store.refresh_all()?;
    let refresh = samples(sample_count, || timed(|| store.refresh_all(), |_| Ok(())))?;

    store.discover_workspace_changes()?;
    let discover = samples(sample_count, || {
        timed(|| store.discover_workspace_changes(), |_| Ok(()))
    })?;

    validate_search(&store)?;
    let search = samples(sample_count, || {
        let elapsed = timed(
            || {
                let mut returned = 0;
                for _ in 0..SEARCH_ITERATIONS {
                    returned += black_box(
                        store.search(black_box("deterministic graph node"), SEARCH_RESULTS)?,
                    )
                    .len();
                }
                Ok(returned)
            },
            |returned| {
                if *returned != SEARCH_RESULTS * SEARCH_ITERATIONS {
                    bail!("search returned {returned} total rows");
                }
                Ok(())
            },
        )?;
        Ok(elapsed * 1_000.0 / SEARCH_ITERATIONS as f64)
    })?;

    validate_full_graph(&store)?;
    let full_graph = samples(sample_count, || {
        timed(
            || Ok((store.all_node_summaries()?, store.all_valid_edges()?)),
            |(nodes, edges)| {
                if nodes.len() != NODE_COUNT || edges.len() != EDGE_COUNT {
                    bail!(
                        "full graph returned {} nodes and {} edges",
                        nodes.len(),
                        edges.len()
                    );
                }
                Ok(())
            },
        )
    })?;

    validate_graph_check(&mut store)?;
    let graph_check = samples(sample_count, || {
        timed(
            || store.graph_check_report(),
            |report| {
                if report.nodes as usize != NODE_COUNT
                    || report.edges as usize != EDGE_COUNT
                    || !report.missing.is_empty()
                    || !report.invalid.is_empty()
                    || !report.cycles.is_empty()
                {
                    bail!("graph check did not return the clean fixture");
                }
                Ok(())
            },
        )
    })?;

    let raw_samples = BTreeMap::from([
        ("graph.check".to_string(), rounded(&graph_check)),
        ("graph.fullRead".to_string(), rounded(&full_graph)),
        ("index.discoverNoop".to_string(), rounded(&discover)),
        ("index.fullRefresh".to_string(), rounded(&refresh)),
        ("query.search200".to_string(), rounded(&search)),
    ]);
    let metrics = BTreeMap::from([
        ("graph.check".to_string(), metric(&graph_check, "ms")),
        ("graph.fullRead".to_string(), metric(&full_graph, "ms")),
        ("index.discoverNoop".to_string(), metric(&discover, "ms")),
        ("index.fullRefresh".to_string(), metric(&refresh, "ms")),
        ("query.search200".to_string(), metric(&search, "us")),
    ]);

    Ok(Report {
        schema: REPORT_SCHEMA,
        generated_at_unix_seconds: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        environment: Environment {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu: cpu_model(),
            rustc: command_output("rustc", &["--version"]),
            samples: sample_count,
        },
        fixtures: Fixtures {
            nodes: NODE_COUNT,
            edges: EDGE_COUNT,
            search_results: SEARCH_RESULTS,
        },
        metrics,
        raw_samples,
    })
}

fn create_fixture(root: &Path) -> Result<()> {
    mathdoc::workspace::initialize(root)?;
    let fixture_root = root.join("perf");
    for group in 0..NODE_COUNT.div_ceil(100) {
        fs::create_dir_all(fixture_root.join(format!("{group:03}")))?;
    }
    for index in 0..NODE_COUNT {
        let node_fnode = fnode(index);
        let title = if index == 0 {
            "Performance fixture".to_string()
        } else {
            format!("Deterministic graph node {index:05}")
        };
        let mut contents = format!("@fnode: {node_fnode}\n@title: {title}\n");
        if index > 0 {
            contents.push_str("\n@dep:\n");
            contents.push_str(&fnode((index - 1) / 2));
            contents.push('\n');
            if index >= 6 {
                contents.push_str(&fnode((index - 3) / 3));
                contents.push('\n');
            }
            contents.push_str("@end\n");
        }
        fs::write(
            fixture_root.join(format!("{:03}/node-{index:05}.mdoc", index / 100)),
            contents,
        )?;
    }
    Ok(())
}

fn fnode(index: usize) -> String {
    if index == 0 {
        "perf-root".to_string()
    } else {
        format!("perf-node-{index:05}")
    }
}

fn validate_store(store: &mut WorkspaceStore) -> Result<()> {
    if store.count()? as usize != NODE_COUNT {
        bail!("fixture did not index {NODE_COUNT} nodes");
    }
    validate_search(store)?;
    validate_full_graph(store)?;
    validate_graph_check(store)
}

fn validate_search(store: &WorkspaceStore) -> Result<()> {
    let results = store.search("deterministic graph node", SEARCH_RESULTS)?;
    if results.len() != SEARCH_RESULTS {
        bail!("search returned {} rows", results.len());
    }
    Ok(())
}

fn validate_full_graph(store: &WorkspaceStore) -> Result<()> {
    let nodes = store.all_node_summaries()?;
    let edges = store.all_valid_edges()?;
    if nodes.len() != NODE_COUNT || edges.len() != EDGE_COUNT {
        bail!(
            "fixture has {} nodes and {} edges, expected {NODE_COUNT} and {EDGE_COUNT}",
            nodes.len(),
            edges.len()
        );
    }
    Ok(())
}

fn validate_graph_check(store: &mut WorkspaceStore) -> Result<()> {
    let report = store.graph_check_report()?;
    if report.nodes as usize != NODE_COUNT
        || report.edges as usize != EDGE_COUNT
        || !report.missing.is_empty()
        || !report.invalid.is_empty()
        || !report.cycles.is_empty()
    {
        bail!("fixture graph check is not clean");
    }
    Ok(())
}

fn timed<T>(
    operation: impl FnOnce() -> Result<T>,
    validate: impl FnOnce(&T) -> Result<()>,
) -> Result<f64> {
    let start = Instant::now();
    let value = black_box(operation()?);
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    validate(&value)?;
    black_box(value);
    Ok(elapsed)
}

fn samples(count: usize, mut sample: impl FnMut() -> Result<f64>) -> Result<Vec<f64>> {
    (0..count).map(|_| sample()).collect()
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn round(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

fn rounded(values: &[f64]) -> Vec<f64> {
    values.iter().copied().map(round).collect()
}

fn metric(values: &[f64], unit: &str) -> Metric {
    Metric {
        value: round(median(values)),
        unit: unit.to_string(),
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("parsing {}", path.display()))
}

fn compare(baseline: &Report, current: &Report, budgets: &Budgets) -> Result<()> {
    let mut failures = Vec::new();
    if baseline.schema != current.schema || budgets.schema != current.schema {
        failures.push("report or budget schema differs".to_string());
    }
    if baseline.fixtures != current.fixtures {
        failures.push("fixtures differ".to_string());
    }
    if baseline.environment != current.environment {
        failures.push("benchmark environments differ".to_string());
    }
    if !same_metric_names(&baseline.metrics, &current.metrics) {
        failures.push("baseline and current metric names differ".to_string());
    }
    if !same_metric_names(&current.metrics, &budgets.metrics) {
        failures.push("report and budget metric names differ".to_string());
    }

    println!(
        "\n{:<28} {:>12} {:>12} {:>12}",
        "metric", "baseline", "current", "limit"
    );
    for (name, budget) in &budgets.metrics {
        let Some(before) = baseline.metrics.get(name) else {
            failures.push(format!("{name}: missing baseline metric"));
            continue;
        };
        let Some(after) = current.metrics.get(name) else {
            failures.push(format!("{name}: missing current metric"));
            continue;
        };
        if before.unit != after.unit {
            failures.push(format!("{name}: units differ"));
            continue;
        }
        let noise_limit = before.value + budget.noise;
        let relative_limit = before.value * (1.0 + budget.max_increase_ratio);
        let mut limit = noise_limit.max(relative_limit);
        if let Some(maximum) = budget.max {
            limit = limit.min(maximum);
        }
        let passed = after.value <= limit;
        println!(
            "{name:<28} {:>9.3} {:>9.3} {:>9.3} {}",
            before.value,
            after.value,
            limit,
            if passed { "ok" } else { "REGRESSION" }
        );
        if !passed {
            failures.push(format!(
                "{name}: {:.3} {} > {:.3} {}",
                after.value, after.unit, limit, after.unit
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("performance regression:\n{}", failures.join("\n"))
    }
}

fn same_metric_names<T, U>(left: &BTreeMap<String, T>, right: &BTreeMap<String, U>) -> bool {
    left.len() == right.len()
        && left
            .keys()
            .zip(right.keys())
            .all(|(left, right)| left == right)
}

fn print_metrics(report: &Report) {
    println!("{:<28} {:>12}", "metric", "median");
    for (name, metric) in &report.metrics {
        println!("{name:<28} {:>9.3} {}", metric.value, metric.unit);
    }
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
