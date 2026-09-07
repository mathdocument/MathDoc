use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use mathdoc::{
    service::{router, Service},
    store::{Database, Node},
};
use std::time::Instant;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires local TerminusDB; creates an ETP-sized synthetic graph"]
async fn graph_checks_on_47435_nodes_and_368017_edges() {
    let existing = std::env::var("MDC_SCALE_DATABASE").ok();
    let db = Database::from_env(
        existing
            .clone()
            .unwrap_or_else(|| format!("mdcscale{}", uuid::Uuid::new_v4().simple())),
        "main".into(),
    )
    .unwrap();
    eprintln!("benchmark database: {}", db.database);
    if existing.is_none() {
        db.initialize().await.unwrap();
        let version = db.version().await.unwrap();
        let mut nodes: Vec<Node> = Vec::with_capacity(47435);
        let mut edges = 0;
        for i in 0..47435 {
            let mut node = Node::new(format!("Node {i}")).unwrap();
            for previous in nodes.iter().rev().take(8) {
                if edges == 368017 {
                    break;
                }
                node.depens.push(previous.fnode.clone());
                edges += 1;
            }
            nodes.push(node);
        }
        let started = Instant::now();
        db.put(&nodes, &version, "Synthetic graph benchmark")
            .await
            .unwrap();
        eprintln!("one-time database import: {:?}", started.elapsed());
    }
    let started = Instant::now();
    let app = router(Service::open(db).await.unwrap());
    eprintln!("one-time service startup: {:?}", started.elapsed());
    let mut times = vec![];
    for _ in 0..7 {
        let started = Instant::now();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/graph/check")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let report: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(status, 200, "{report}");
        assert_eq!(report["nodes"], 47435);
        assert_eq!(report["edges"], 368017);
        assert_eq!(report["cycles"], serde_json::json!([]));
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    eprintln!(
        "graph check milliseconds (includes real DB revision lookup): {times:?}; median={:.2}",
        times[3]
    );
}
