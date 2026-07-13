use std::path::{Path, PathBuf};

use axum::body::Body;
#[cfg(not(feature = "dev-web"))]
use axum::http::Uri;
use axum::http::{Request, StatusCode};
#[cfg(not(feature = "dev-web"))]
use regex::Regex;
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
    cache.discover_workspace_changes().unwrap();

    let state = web::AppState::new(root.clone(), cache);
    let app = build_router(state);
    (root, app)
}

/// Mirror of the production router, but without graceful shutdown wiring.
fn build_router(state: web::AppState) -> axum::Router {
    web::server::router(state)
}

// Use axum's test helpers — `tower::ServiceExt::oneshot`.
use tower::ServiceExt;

fn local_request() -> axum::http::request::Builder {
    Request::builder().header("host", "127.0.0.1:7878")
}

async fn get_json(app: &axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(local_request().uri(path).body(Body::empty()).unwrap())
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
            local_request()
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
async fn production_router_does_not_allow_cross_origin_api_requests() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let response = app
        .clone()
        .oneshot(
            local_request()
                .method("OPTIONS")
                .uri("/api/node/new")
                .header("origin", "https://attacker.example")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn production_router_only_accepts_local_host_headers() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    for host in [
        "127.0.0.1:7878",
        "[::1]:7878",
        "localhost:7878",
        "math.localhost:7878",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/graph/roots")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "rejected {host}");
    }

    for host in [
        "0.0.0.0:7878",
        "192.168.1.10:7878",
        "8.8.8.8:7878",
        "attacker.example:7878",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/graph/roots")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::MISDIRECTED_REQUEST,
            "accepted {host}"
        );
    }

    let missing = app
        .oneshot(
            Request::builder()
                .uri("/api/graph/roots")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn production_router_returns_404_for_missing_static_asset() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let response = app
        .clone()
        .oneshot(
            local_request()
                .uri("/assets/does-not-exist.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_ne!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/html")
    );

    let exact_assets = app
        .oneshot(local_request().uri("/assets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(exact_assets.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_errors_use_json_status_contract() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);

    let (status, value) = get_json(&app, "/api/node/missing-node").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(value["error"], "node not found");

    let (status, value) = get_json(&app, "/api/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(value["error"], "API route not found");

    let response = app
        .clone()
        .oneshot(
            local_request()
                .method("POST")
                .uri("/api/node/new")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("application/json"));
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(value["error"].is_string());
}

#[tokio::test]
async fn spa_index_is_not_cached() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let response = app
        .oneshot(local_request().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
}

#[tokio::test]
#[cfg(not(feature = "dev-web"))]
async fn embedded_index_assets_use_release_mime_and_cache_policy() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let embedded_index = web::assets::WebAssets::get("index.html").unwrap();

    let response = app
        .clone()
        .oneshot(local_request().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "text/html");
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    let index = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(index.as_ref(), embedded_index.data.as_ref());

    let index = std::str::from_utf8(&index).unwrap();
    let url_pattern = Regex::new(r#"(?:src|href)="([^"]+)""#).unwrap();
    let mut asset_count = 0;
    for captures in url_pattern.captures_iter(index) {
        let url = &captures[1];
        if url.starts_with("data:") {
            continue;
        }
        let uri: Uri = url.parse().unwrap();
        assert!(
            uri.scheme().is_none() && uri.authority().is_none(),
            "external URL: {url}"
        );
        let path = uri.path().trim_start_matches('/').to_string();
        let embedded = web::assets::WebAssets::get(&path)
            .unwrap_or_else(|| panic!("index references missing embedded asset: {url}"));

        let response = app
            .clone()
            .oneshot(local_request().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "failed to serve {url}");
        let expected_mime = mime_guess::from_path(&path)
            .first_or_octet_stream()
            .essence_str()
            .to_string();
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            &expected_mime
        );
        assert_eq!(
            response.headers().get("cache-control").unwrap(),
            "public, max-age=31536000, immutable"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            body.as_ref(),
            embedded.data.as_ref(),
            "wrong body for {url}"
        );
        asset_count += 1;
    }
    assert!(
        asset_count > 0,
        "index.html contains no embedded asset URLs"
    );
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
async fn search_caps_requested_rows() {
    let dir = TempDir::new().unwrap();
    let (root, app) = build_app(&dir);
    for index in 0..250 {
        let path = root.join(format!("bulk-{index}.mdoc"));
        let mut node = MdocNode::new_at_path(&root, &path, &format!("Bulk {index}"));
        node.fnode = format!("bulk-{index}");
        node.save_new().unwrap();
    }

    let (status, value) = get_json(&app, "/api/search?q=Bulk&n=1000000").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value.as_array().unwrap().len(), 200);

    let (status, value) = get_json(&app, "/api/search?q=Bulk&n=7").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value.as_array().unwrap().len(), 7);
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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(val["error"].as_str().unwrap().contains("non-empty"));
}

#[tokio::test]
async fn title_write_restores_file_when_index_update_fails() {
    let dir = TempDir::new().unwrap();
    let (root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let fnode = roots.as_array().unwrap()[0]["fnode"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, detail) = get_json(&app, &format!("/api/node/{fnode}")).await;
    let path = root.join(detail["rel_path"].as_str().unwrap());
    let original_title = MdocNode::load(&root, &path).unwrap().title;

    let conn = rusqlite::Connection::open(root.join(".mdc/index.db")).unwrap();
    conn.execute("DROP TABLE mdoc_files", []).unwrap();
    drop(conn);

    let (status, value) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{fnode}/title"),
        serde_json::json!({ "title": "Must Roll Back" }),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(value["error"], "internal server error");
    assert_eq!(MdocNode::load(&root, &path).unwrap().title, original_title);
}

#[tokio::test]
async fn writes_reject_structural_injection_without_changing_the_file() {
    let dir = TempDir::new().unwrap();
    let (root, app) = build_app(&dir);

    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let main = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap();
    let fnode = main["fnode"].as_str().unwrap();
    let path = root.join(main["rel_path"].as_str().unwrap());
    let original = std::fs::read_to_string(&path).unwrap();

    let (status, val) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{}/block/latex", fnode),
        serde_json::json!({ "content": "before\n@end\nafter" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "val={val}");
    assert!(val["error"].is_string());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

    let (status, val) = send_json(
        &app,
        "PUT",
        &format!("/api/node/{}/title", fnode),
        serde_json::json!({ "title": "Injected\n@dep:\nevil\n@end" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "val={val}");
    assert!(val["error"].is_string());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_block_updates_do_not_lose_changes() {
    use std::sync::{Arc, Barrier};

    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let fnode = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Main Theorem")
        .unwrap()["fnode"]
        .as_str()
        .unwrap()
        .to_string();

    let updates = [
        ("text", "text body"),
        ("python", "python body"),
        ("lean", "lean body"),
        ("rocq", "rocq body"),
    ];
    let barrier = Arc::new(Barrier::new(updates.len() + 1));
    let mut tasks = Vec::new();
    for (srctype, content) in updates {
        let app = app.clone();
        let fnode = fnode.clone();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait();
            send_json(
                &app,
                "PUT",
                &format!("/api/node/{fnode}/block/{srctype}"),
                serde_json::json!({ "content": content }),
            )
            .await
        }));
    }
    barrier.wait();
    for task in tasks {
        let (status, val) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "val={val}");
    }

    let (_, node) = get_json(&app, &format!("/api/node/{fnode}")).await;
    let srctypes: std::collections::HashSet<&str> = node["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block["srctype"].as_str())
        .collect();
    assert_eq!(srctypes.len(), 5);
    for expected in ["latex", "text", "python", "lean", "rocq"] {
        assert!(
            srctypes.contains(expected),
            "missing block {expected}: {node}"
        );
    }
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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(val["error"].is_string());
}

#[tokio::test]
async fn add_dep_resolves_prefix_and_case_to_exact_fnode() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let roots = roots.as_array().unwrap();
    let main = roots
        .iter()
        .find(|item| item["title"] == "Main Theorem")
        .unwrap();
    let target = roots
        .iter()
        .find(|item| item["title"] == "Background Lemma")
        .unwrap();
    let main_fnode = main["fnode"].as_str().unwrap();
    let target_fnode = target["fnode"].as_str().unwrap();
    let prefix = &target_fnode[..8];

    let (status, value) = send_json(
        &app,
        "POST",
        &format!("/api/node/{main_fnode}/dep/add"),
        serde_json::json!({ "dep_fnode": prefix }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["depens"], serde_json::json!([target_fnode]));

    let (status, _) = send_json(
        &app,
        "POST",
        &format!("/api/node/{main_fnode}/dep/rm"),
        serde_json::json!({ "dep_fnodes": [target_fnode] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let upper = target_fnode.to_ascii_uppercase();
    let (status, value) = send_json(
        &app,
        "POST",
        &format!("/api/node/{main_fnode}/dep/add"),
        serde_json::json!({ "dep_fnode": upper }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["depens"], serde_json::json!([target_fnode]));
}

#[tokio::test]
async fn add_dep_rejects_missing_duplicate_invalid_and_self_targets() {
    let dir = TempDir::new().unwrap();
    let (root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let roots = roots.as_array().unwrap();
    let main = roots
        .iter()
        .find(|item| item["title"] == "Main Theorem")
        .unwrap();
    let target = roots
        .iter()
        .find(|item| item["title"] == "Background Lemma")
        .unwrap();
    let main_fnode = main["fnode"].as_str().unwrap();
    let target_fnode = target["fnode"].as_str().unwrap();
    let target_path = root.join(target["rel_path"].as_str().unwrap());

    for rejected in ["missing-target", &main_fnode[..8]] {
        let (status, _) = send_json(
            &app,
            "POST",
            &format!("/api/node/{main_fnode}/dep/add"),
            serde_json::json!({ "dep_fnode": rejected }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    std::fs::copy(&target_path, root.join("duplicate-target.mdoc")).unwrap();
    let (status, _) = send_json(
        &app,
        "POST",
        &format!("/api/node/{main_fnode}/dep/add"),
        serde_json::json!({ "dep_fnode": target_fnode }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    std::fs::write(
        root.join("invalid-target.mdoc"),
        "@fnode: invalid-target\n@title: One\n@title: Two\n",
    )
    .unwrap();
    let (status, _) = send_json(
        &app,
        "POST",
        &format!("/api/node/{main_fnode}/dep/add"),
        serde_json::json!({ "dep_fnode": "invalid-target" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn remove_dep_allows_existing_dangling_target() {
    let dir = TempDir::new().unwrap();
    let (root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let roots = roots.as_array().unwrap();
    let main = roots
        .iter()
        .find(|item| item["title"] == "Main Theorem")
        .unwrap();
    let main_fnode = main["fnode"].as_str().unwrap();
    let main_path = root.join(main["rel_path"].as_str().unwrap());
    let mut node = MdocNode::load(&root, &main_path).unwrap();
    node.add_dependency("dangling-target");
    node.save().unwrap();

    let (status, value) = send_json(
        &app,
        "POST",
        &format!("/api/node/{main_fnode}/dep/rm"),
        serde_json::json!({ "dep_fnodes": ["dangling-target"] }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(value["depens"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_opposite_edges_cannot_create_a_cycle() {
    use std::sync::{Arc, Barrier};

    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let nodes = roots.as_array().unwrap();
    let a = nodes[0]["fnode"].as_str().unwrap().to_string();
    let b = nodes[1]["fnode"].as_str().unwrap().to_string();
    let barrier = Arc::new(Barrier::new(3));

    let app_ab = app.clone();
    let barrier_ab = Arc::clone(&barrier);
    let a_for_ab = a.clone();
    let b_for_ab = b.clone();
    let add_ab = tokio::spawn(async move {
        barrier_ab.wait();
        send_json(
            &app_ab,
            "POST",
            &format!("/api/node/{a_for_ab}/dep/add"),
            serde_json::json!({ "dep_fnode": b_for_ab }),
        )
        .await
    });

    let app_ba = app.clone();
    let barrier_ba = Arc::clone(&barrier);
    let a_for_ba = a.clone();
    let b_for_ba = b.clone();
    let add_ba = tokio::spawn(async move {
        barrier_ba.wait();
        send_json(
            &app_ba,
            "POST",
            &format!("/api/node/{b_for_ba}/dep/add"),
            serde_json::json!({ "dep_fnode": a_for_ba }),
        )
        .await
    });

    barrier.wait();
    let first = add_ab.await.unwrap();
    let second = add_ba.await.unwrap();
    let statuses = [first.0, second.0];
    assert_eq!(statuses.iter().filter(|&&s| s == StatusCode::OK).count(), 1);
    assert_eq!(
        statuses
            .iter()
            .filter(|&&s| s == StatusCode::UNPROCESSABLE_ENTITY)
            .count(),
        1
    );

    let (status, report) = get_json(&app, "/api/graph/check").await;
    assert_eq!(status, StatusCode::OK, "report={report}");
    assert_eq!(report["cycles"], serde_json::json!([]));
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

#[tokio::test]
async fn new_node_file_rules_match_with_and_without_parent() {
    let dir = TempDir::new().unwrap();
    let (_root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let parent = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["title"] == "Main Theorem")
        .unwrap()["fnode"]
        .as_str()
        .unwrap();

    for (title, file, expected) in [
        ("Standalone Empty", "", None),
        ("Standalone Dot", ".", None),
        (
            "Standalone Suffix",
            "notes/standalone-suffix.mdoc",
            Some("notes/standalone-suffix.mdoc"),
        ),
    ] {
        let (status, node) = send_json(
            &app,
            "POST",
            "/api/node/new",
            serde_json::json!({ "title": title, "file": file }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "node={node}");
        let rel_path = node["rel_path"].as_str().unwrap();
        if let Some(expected) = expected {
            assert_eq!(rel_path, expected);
        } else {
            assert!(!rel_path.contains('/'));
            assert!(rel_path.ends_with(".mdoc"));
        }
    }

    for (title, file, expected) in [
        ("Linked Empty", "", None),
        ("Linked Dot", ".", None),
        (
            "Linked Suffix",
            "notes/linked-suffix.mdoc",
            Some("notes/linked-suffix.mdoc"),
        ),
    ] {
        let (status, result) = send_json(
            &app,
            "POST",
            "/api/node/new",
            serde_json::json!({
                "title": title,
                "file": file,
                "parent_fnode": parent,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "result={result}");
        let (_, matches) = get_json(
            &app,
            &format!("/api/search?q={}", title.replace(' ', "%20")),
        )
        .await;
        let rel_path = matches[0]["rel_path"].as_str().unwrap();
        if let Some(expected) = expected {
            assert_eq!(rel_path, expected);
        } else {
            assert!(!rel_path.contains('/'));
            assert!(rel_path.ends_with(".mdoc"));
        }
    }
}

#[tokio::test]
async fn new_node_rejects_absolute_file_with_and_without_parent() {
    let dir = TempDir::new().unwrap();
    let (root, app) = build_app(&dir);
    let (_, roots) = get_json(&app, "/api/graph/roots").await;
    let parent = roots
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["title"] == "Main Theorem")
        .unwrap()["fnode"]
        .as_str()
        .unwrap();
    let absolute = root.join("absolute-target");

    for parent_fnode in [None, Some(parent)] {
        let (status, _) = send_json(
            &app,
            "POST",
            "/api/node/new",
            serde_json::json!({
                "title": "Absolute Target",
                "file": absolute,
                "parent_fnode": parent_fnode,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(!root.join("absolute-target.mdoc").exists());
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
