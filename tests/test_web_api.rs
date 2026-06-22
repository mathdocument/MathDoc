use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tempfile::TempDir;

use mathdoc::indcache::IndCache;
use mathdoc::mdocnode::MdocNode;
use mathdoc::web;

// ── Workspace helpers ─────────────────────────────────────────────────────────

fn init_workspace(dir: &TempDir) -> PathBuf {
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".mdc")).unwrap();
    std::fs::write(root.join(".mdc").join("config.toml"), "# empty\n").unwrap();
    root
}

fn make_node(root: &Path, title: &str) -> MdocNode {
    let mut node = MdocNode::new_at_path(root, root, title);
    node.path = root.join(format!("{}.mdoc", &node.fnode[..8]));
    node
}

fn make_node_with_block(root: &Path, title: &str, srctype: &str, content: &str) -> MdocNode {
    let mut node = make_node(root, title);
    node.blocks.push(mathdoc::mdocnode::SrcBlock {
        srctype: srctype.to_string(),
        content: content.to_string(),
        metadata: Default::default(),
    });
    node
}

/// Build an axum app against a temp workspace. Returns (root, app).
fn build_app(dir: &TempDir) -> (PathBuf, axum::Router) {
    let root = init_workspace(dir);

    // Create two nodes with a dependency: root depends on dep.
    let dep = make_node_with_block(&root, "Background Lemma", "latex", "x = 1");
    let root_node = make_node_with_block(&root, "Main Theorem", "latex", "y = 2");
    root_node.path.file_name().unwrap();
    dep.save().unwrap();
    root_node.save().unwrap();

    let mut cache = IndCache::open(root.clone()).unwrap();
    cache.bootstrap_if_needed().unwrap();
    cache.discover_workspace_changes().unwrap();

    let state = web::AppState::new(root.clone(), cache);
    let app = build_router(state);
    (root, app)
}

/// Mirror of the production router, but without graceful shutdown wiring.
fn build_router(state: web::AppState) -> axum::Router {
    use axum::routing::{get, post, put, Router};
    use tower_http::cors::CorsLayer;

    let api_routes = Router::new()
        .route("/graph/roots", get(web::api::graph_roots))
        .route("/graph/check", get(web::api::graph_check))
        .route("/graph/full", get(web::api::graph_full))
        .route("/search", get(web::api::search))
        .route("/resolve", get(web::api::resolve_ref))
        .route("/node/:fnode", get(web::api::node_detail))
        .route("/node/:fnode/referrers", get(web::api::node_referrers))
        .route("/node/:fnode/children", get(web::api::node_children))
        .route("/node/:fnode/title", put(web::api::node_put_title))
        .route(
            "/node/:fnode/block/:srctype",
            put(web::api::node_put_block).delete(web::api::node_delete_block),
        )
        .route("/node/:fnode/dep/add", post(web::api::node_add_dep))
        .route("/node/:fnode/dep/rm", post(web::api::node_rm_deps))
        .route("/node/new", post(web::api::node_new));

    Router::new()
        .nest("/api", api_routes)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// Use axum's test helpers — `tower::ServiceExt::oneshot`.
use tower::ServiceExt;

async fn get_json(app: &axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, val)
}

async fn send_json(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, val)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn graph_roots_returns_both_nodes() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (status, val) = get_json(&app, "/api/graph/roots").await;
    assert_eq!(status, StatusCode::OK);
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 2);
}

#[tokio::test]
async fn search_finds_by_title() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (status, val) = get_json(&app, "/api/search?q=theorem").await;
    assert_eq!(status, StatusCode::OK);
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["title"], "Main Theorem");
}

#[tokio::test]
async fn graph_check_reports_clean() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (status, val) = get_json(&app, "/api/graph/check").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["nodes"], 2);
    assert_eq!(val["cycles"], serde_json::json!([]));
}

#[tokio::test]
async fn node_detail_returns_blocks_and_depens() {
    let dir = TempDir::new().unwrap();
    let (root, app) = build_app(&dir);

    // Find the fnode of Main Theorem.
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let fnode = main["fnode"].as_str().unwrap();

    let (status, val) = get_json(&app, &format!("/api/node/{}", fnode)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(val["title"], "Main Theorem");
    assert_eq!(val["blocks"][0]["srctype"], "latex");
    assert_eq!(val["blocks"][0]["content"], "y = 2\n");
    assert_eq!(val["depens"], serde_json::json!([]));

    let _ = root;
}

#[tokio::test]
async fn resolve_ref_with_prefix_works() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    // resolve_ref resolves by fnode / prefix / path, not by title.
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let prefix = &main["fnode"].as_str().unwrap()[..8];
    let (status, val) = get_json(&app, &format!("/api/resolve?ref={}", prefix)).await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    assert_eq!(val["title"], "Main Theorem");
}

#[tokio::test]
async fn referrers_and_children_are_consistent() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let bg = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Background Lemma")
        .unwrap();

    // Before linking, Main has no children, Background has no referrers.
    let (_, children) = get_json(
        &app,
        &format!("/api/node/{}/children", main["fnode"].as_str().unwrap()),
    )
    .await;
    assert_eq!(children.as_array().unwrap().len(), 0);

    // Link Main → Background via DepGraph directly.
    let root = dir.path().to_path_buf();
    let mut graph =
        mathdoc::depgraph::DepGraph::new(root.clone(), main["fnode"].as_str().unwrap()).unwrap();
    graph
        .add_direct_dependencies(vec![bg["fnode"].as_str().unwrap().to_string()])
        .unwrap();

    // The app's cache is stale; recreate to reflect the link.
    let mut cache = IndCache::open(root.clone()).unwrap();
    cache.discover_workspace_changes().unwrap();
    let state = web::AppState::new(root.clone(), cache);
    let app = build_router(state);

    let (_, children) = get_json(
        &app,
        &format!("/api/node/{}/children", main["fnode"].as_str().unwrap()),
    )
    .await;
    assert_eq!(children.as_array().unwrap().len(), 1);
    assert_eq!(children[0]["title"], "Background Lemma");

    let (_, referrers) = get_json(
        &app,
        &format!("/api/node/{}/referrers", bg["fnode"].as_str().unwrap()),
    )
    .await;
    assert_eq!(referrers.as_array().unwrap().len(), 1);
    assert_eq!(referrers[0]["title"], "Main Theorem");
}

// ── Write endpoint tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn put_block_creates_and_updates_block() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let fnode = main["fnode"].as_str().unwrap();

    // Add a new text block.
    let (status, val) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{}/block/text", fnode),
        serde_json::json!({ "content": "hello world\n" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    let blocks = val["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    let text_block = blocks.iter().find(|b| b["srctype"] == "text").unwrap();
    assert_eq!(text_block["content"], "hello world\n");

    // Update the existing text block.
    let (status, val) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{}/block/text", fnode),
        serde_json::json!({ "content": "updated\n" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text_block = val["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["srctype"] == "text")
        .unwrap();
    assert_eq!(text_block["content"], "updated\n");

    // Verify persistence on disk via a fresh GET.
    let (_, fresh) = get_json(&app, &format!("/api/node/{}", fnode)).await;
    let text_block = fresh["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["srctype"] == "text")
        .unwrap();
    assert_eq!(text_block["content"], "updated\n");
}

#[tokio::test]
async fn put_block_rejects_unknown_srctype() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let fnode = roots.as_array().unwrap()[0]["fnode"].as_str().unwrap();

    let (status, val) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{}/block/rust", fnode),
        serde_json::json!({ "content": "fn main() {}" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(val["error"]
        .as_str()
        .unwrap()
        .contains("unsupported srctype"));
}

#[tokio::test]
async fn delete_block_removes_block() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let fnode = main["fnode"].as_str().unwrap();
    assert_eq!(
        main["title"], "Main Theorem",
        "sanity: pre-built node has 1 block"
    );

    let (status, val) = send_json(
        &app,
        "DELETE",
        &format!("/api/node/{}/block/latex", fnode),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    assert!(val["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|b| b["srctype"] != "latex"));
}

#[tokio::test]
async fn put_title_updates_title() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let fnode = roots.as_array().unwrap()[0]["fnode"].as_str().unwrap();

    let (status, val) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{}/title", fnode),
        serde_json::json!({ "title": "Renamed Title" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    assert_eq!(val["title"], "Renamed Title");

    // Reject empty.
    let (status, val) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{}/title", fnode),
        serde_json::json!({ "title": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(val["error"].as_str().unwrap().contains("non-empty"));
}

// ── Dependency mutation tests ─────────────────────────────────────────────────

#[tokio::test]
async fn add_and_remove_dep_via_api() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let bg = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Background Lemma")
        .unwrap();
    let main_fnode = main["fnode"].as_str().unwrap();
    let bg_fnode = bg["fnode"].as_str().unwrap();

    // Add dep: Main → Background.
    let (status, val) = send_json(
        &app,
        "POST",
        &format!("/api/node/{}/dep/add", main_fnode),
        serde_json::json!({ "dep_fnode": bg_fnode }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    assert!(val["depens"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(bg_fnode)));

    // Children column should now show Background.
    let (_, children) = get_json(&app, &format!("/api/node/{}/children", main_fnode)).await;
    assert_eq!(children.as_array().unwrap().len(), 1);
    assert_eq!(children[0]["title"], "Background Lemma");

    // Adding the same dep again should fail (already present).
    let (status, _val) = send_json(
        &app,
        "POST",
        &format!("/api/node/{}/dep/add", main_fnode),
        serde_json::json!({ "dep_fnode": bg_fnode }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Remove the dep.
    let (status, val) = send_json(
        &app,
        "POST",
        &format!("/api/node/{}/dep/rm", main_fnode),
        serde_json::json!({ "dep_fnodes": [bg_fnode] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    assert!(!val["depens"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(bg_fnode)));
}

#[tokio::test]
async fn add_dep_rejects_cycle() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let bg = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Background Lemma")
        .unwrap();
    let main_fnode = main["fnode"].as_str().unwrap();
    let bg_fnode = bg["fnode"].as_str().unwrap();

    // Main → Background (legal).
    let (status, _) = send_json(
        &app,
        "POST",
        &format!("/api/node/{}/dep/add", main_fnode),
        serde_json::json!({ "dep_fnode": bg_fnode }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Background → Main (cycle) should be rejected.
    let (status, val) = send_json(
        &app,
        "POST",
        &format!("/api/node/{}/dep/add", bg_fnode),
        serde_json::json!({ "dep_fnode": main_fnode }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(val["error"].as_str().unwrap().contains("cycle"));
}

#[tokio::test]
async fn new_node_creates_and_links_to_parent() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let main_fnode = main["fnode"].as_str().unwrap();

    let (status, val) = send_json(
        &app,
        "POST",
        "/api/node/new",
        serde_json::json!({
            "title": "Sub Lemma",
            "parent_fnode": main_fnode,
        }),
    )
    .await;
    // The handler returns the parent's detail so the UI can refresh.
    assert_eq!(status, StatusCode::OK, "val={val}");
    assert_eq!(val["fnode"], main_fnode);
    assert_eq!(val["depens"].as_array().unwrap().len(), 1);

    // Verify the new node is searchable.
    let (_, results) = get_json(&app, "/api/search?q=Sub").await;
    assert_eq!(results.as_array().unwrap().len(), 1);
    assert_eq!(results[0]["title"], "Sub Lemma");
}

#[tokio::test]
async fn new_node_standalone_no_parent() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (status, val) = send_json(
        &app,
        "POST",
        "/api/node/new",
        serde_json::json!({
            "title": "Lone Node",
            "file": "notes/lone",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    assert_eq!(val["title"], "Lone Node");
    assert_eq!(val["rel_path"], "notes/lone.mdoc");
}

// ── Force graph endpoint ──────────────────────────────────────────────────────

#[tokio::test]
async fn graph_full_returns_nodes_and_edges() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    // Link Main → Background via DepGraph directly so we have an edge.
    let root = dir.path().to_path_buf();
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let bg = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Background Lemma")
        .unwrap();
    let mut graph =
        mathdoc::depgraph::DepGraph::new(root.clone(), main["fnode"].as_str().unwrap()).unwrap();
    graph
        .add_direct_dependencies(vec![bg["fnode"].as_str().unwrap().to_string()])
        .unwrap();
    drop(graph);

    // Rebuild app cache to pick up the new edge.
    let mut cache = IndCache::open(root.clone()).unwrap();
    cache.discover_workspace_changes().unwrap();
    let state = web::AppState::new(root.clone(), cache);
    let app = build_router(state);

    let (status, val) = get_json(&app, "/api/graph/full").await;
    assert_eq!(status, StatusCode::OK, "val={val}");
    let nodes = val["nodes"].as_array().unwrap();
    let edges = val["edges"].as_array().unwrap();
    assert_eq!(nodes.len(), 2, "both valid nodes should be present");
    assert_eq!(edges.len(), 1, "one edge Main → Background");
    assert_eq!(edges[0]["source"], main["fnode"]);
    assert_eq!(edges[0]["target"], bg["fnode"]);
}
