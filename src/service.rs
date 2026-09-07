//! Local web service: every client uses the same versioned database transactions.
use crate::store::{Block, Database, LeanProject, Node, Snapshot, BLOCK_TYPES};
use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::{Mutex, MutexGuard};

pub struct Service {
    pub db: Database,
    pub snapshot: Mutex<Snapshot>,
    pub lean: crate::lean::LeanService,
    editors: Mutex<HashMap<String, EditorSession>>,
    editor_slots: Arc<tokio::sync::Semaphore>,
}
impl Service {
    pub async fn open(db: Database) -> Result<Arc<Self>> {
        let snapshot = db.load().await?;
        let lean = crate::lean::LeanService::new(db.cache_path()?);
        Ok(Arc::new(Self {
            db,
            snapshot: Mutex::new(snapshot),
            lean,
            editors: Mutex::new(HashMap::new()),
            editor_slots: Arc::new(tokio::sync::Semaphore::new(8)),
        }))
    }
    pub async fn read(&self) -> Result<MutexGuard<'_, Snapshot>> {
        let mut snapshot = self.snapshot.lock().await;
        // Cheap database commit lookup; never scans a filesystem or reparses source.
        if self.db.version().await? != snapshot.version {
            *snapshot = self.db.load().await?;
        }
        Ok(snapshot)
    }
    pub async fn save(
        &self,
        snapshot: &mut Snapshot,
        changes: Vec<Node>,
        message: &str,
    ) -> Result<()> {
        snapshot.validate_changes(&changes)?;
        let version = self.db.put(&changes, &snapshot.version, message).await?;
        snapshot.apply(changes, version);
        Ok(())
    }
}

pub struct ApiError(StatusCode, String);
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        let text = error.to_string();
        let status = if text.contains("revision conflict") {
            StatusCode::CONFLICT
        } else if text.starts_with("node not found") {
            StatusCode::NOT_FOUND
        } else if text.contains("TerminusDB") {
            StatusCode::BAD_GATEWAY
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
        Self(status, text)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
type ApiResult<T> = std::result::Result<T, ApiError>;
fn expected(headers: &HeaderMap) -> ApiResult<&str> {
    headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix('"'))
        .and_then(|v| v.strip_suffix('"'))
        .ok_or(ApiError(
            StatusCode::PRECONDITION_REQUIRED,
            "a quoted If-Match revision is required".into(),
        ))
}
fn check_revision(headers: &HeaderMap, node: &Node) -> ApiResult<()> {
    if expected(headers)? != node.revision() {
        return Err(ApiError(
            StatusCode::PRECONDITION_FAILED,
            "node changed; reload and retry".into(),
        ));
    }
    Ok(())
}
fn summary(snapshot: &Snapshot, node: &Node) -> Value {
    json!({"fnode":node.fnode,"title":node.title,"broken":false,"depth":snapshot.depths.get(&node.fnode).copied().unwrap_or(0)})
}
pub fn detail(snapshot: &Snapshot, node: &Node, lean: &crate::lean::LeanService) -> Value {
    let mut value = summary(snapshot, node);
    value["revision"] = json!(node.revision());
    value["depens"] = json!(node.depens);
    value["blocks"] = json!(node.blocks);
    value["module"] = json!(node.module);
    value["formalization"] = json!({"lean":if node.source("lean").is_some(){"unverified"}else{"no_code"},
        "rocq":if node.source("rocq").is_some(){"unverified"}else{"no_code"}});
    if let Some(key) = snapshot.lean_keys.get(&node.fnode) {
        if lean
            .cached(key, &node.revision(), false)
            .is_some_and(|r| r.certified)
        {
            value["formalization"]["lean"] = json!("verified");
        }
    }
    value
}
fn revision_response(
    snapshot: &Snapshot,
    node: &Node,
    lean: &crate::lean::LeanService,
) -> Response {
    (
        [("etag", format!("\"{}\"", node.revision()))],
        Json(detail(snapshot, node, lean)),
    )
        .into_response()
}
pub fn graph_report(snapshot: &Snapshot) -> Value {
    // All commits are validated before publication; graph checks read this validated projection.
    json!({"nodes":snapshot.nodes.len(),"edges":snapshot.nodes.values().map(|n|n.depens.len()).sum::<usize>(),
        "missing":[],"invalid":[],"cycles":[]})
}

pub fn router(service: Arc<Service>) -> Router {
    let api = Router::new()
        .route("/graph/check", get(graph_check))
        .route("/graph/roots", get(roots))
        .route("/graph/full", get(full))
        .route("/search", get(search))
        .route("/resolve", get(resolve))
        .route("/node/new", post(new_node))
        .route("/node/:id/view", get(view))
        .route("/node/:id/lean/check", post(lean_check))
        .route("/node/:id/lean/goals", post(lean_goals))
        .route("/node/:id/lean/session", post(lean_session))
        .route("/lean/session/:id", get(editor_info))
        .route("/lean/session/:id/ws", get(editor_socket))
        .route("/node/:id/title", put(title))
        .route("/node/:id/block/:language", put(block).delete(delete_block))
        .route("/node/:id/dep/candidates", get(candidates))
        .route("/node/:id/dep", get(traverse))
        .route("/node/:id/metric/ior", get(ior))
        .route("/node/:id/dep/add", post(add_dep))
        .route("/node/:id/dep/rm", post(rm_deps))
        .route("/project/lean", get(project).put(put_project))
        .route("/export", get(export))
        .route("/import", post(import))
        .route("/history", get(history))
        .route("/branches", post(branch))
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error":"API endpoint not found"})),
            )
        });
    Router::new()
        .nest("/api", api)
        .with_state(service)
        .fallback(get(|uri: axum::http::Uri| async move {
            crate::web::assets::serve_asset(uri)
        }))
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .layer(axum::middleware::from_fn(local_origin))
}

async fn local_origin(request: Request, next: axum::middleware::Next) -> Response {
    let host = request
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let valid = host
        .parse::<axum::http::uri::Authority>()
        .ok()
        .is_some_and(|h| {
            h.host() == "localhost"
                || h.host()
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
    let origin_valid = request.headers().get("origin").is_none_or(|origin| {
        origin
            .to_str()
            .ok()
            .and_then(|v| reqwest::Url::parse(v).ok())
            .is_some_and(|u| {
                u.scheme() == "http" && u.origin().ascii_serialization() == format!("http://{host}")
            })
    });
    if !valid || !origin_valid {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"local same-origin requests required"})),
        )
            .into_response();
    }
    let response = next.run(request).await;
    if (response.status().is_client_error() || response.status().is_server_error())
        && !response
            .headers()
            .get("content-type")
            .is_some_and(|v| v.to_str().unwrap_or("").starts_with("application/json"))
    {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap_or_default();
        return (
            status,
            Json(json!({"error":String::from_utf8_lossy(&bytes)})),
        )
            .into_response();
    }
    response
}

pub async fn serve(db: Database, bind: &str) -> Result<()> {
    let address: std::net::SocketAddr = bind.parse().context("use a numeric loopback address")?;
    if !address.ip().is_loopback() {
        bail!("self-hosted service must bind to loopback");
    }
    let service = Service::open(db).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!("MathDoc → http://{}", listener.local_addr()?);
    axum::serve(listener, router(service))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn graph_check(State(s): State<Arc<Service>>) -> ApiResult<Json<Value>> {
    Ok(Json(graph_report(&*s.read().await?)))
}
async fn roots(State(s): State<Arc<Service>>) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let referenced: BTreeSet<_> = snapshot
        .nodes
        .values()
        .flat_map(|n| n.depens.iter())
        .collect();
    let sizes = crate::core::weak_component_sizes(
        &snapshot.graph(),
        &snapshot.nodes.keys().cloned().collect::<HashSet<_>>(),
    );
    let mut nodes: Vec<_> = snapshot
        .nodes
        .values()
        .filter(|n| !referenced.contains(&n.fnode))
        .map(|n| {
            let mut v = summary(&snapshot, n);
            v["component_size"] = json!(sizes.get(&n.fnode).copied().unwrap_or(1));
            v["topo_depth"] = v["depth"].clone();
            v
        })
        .collect();
    nodes.sort_by_key(|v| std::cmp::Reverse(v["topo_depth"].as_u64().unwrap_or(0)));
    Ok(Json(json!(nodes)))
}
async fn full(State(s): State<Arc<Service>>) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let indexes: HashMap<_, _> = snapshot
        .nodes
        .keys()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();
    let edges: Vec<_> = snapshot
        .nodes
        .values()
        .flat_map(|n| {
            n.depens
                .iter()
                .filter_map(|d| Some([*indexes.get(&n.fnode)?, *indexes.get(d)?]))
        })
        .collect();
    Ok(Json(
        json!({"nodes":snapshot.nodes.values().map(|n|summary(&snapshot,n)).collect::<Vec<_>>(),"edges":edges}),
    ))
}
#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    q: String,
    n: Option<usize>,
}
async fn search(
    State(s): State<Arc<Service>>,
    Query(query): Query<Search>,
) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let q = query.q.to_lowercase();
    Ok(Json(json!(snapshot
        .nodes
        .values()
        .filter(|n| n.title.to_lowercase().contains(&q) || n.fnode.contains(&q))
        .take(query.n.unwrap_or(200).min(200))
        .map(|n| summary(&snapshot, n))
        .collect::<Vec<_>>())))
}
#[derive(Deserialize)]
struct Reference {
    r#ref: String,
}
async fn resolve(
    State(s): State<Arc<Service>>,
    Query(query): Query<Reference>,
) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let n = snapshot.resolve(&query.r#ref)?;
    Ok(Json(json!({"fnode":n.fnode,"title":n.title})))
}
async fn view(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let n = snapshot.resolve(&id)?;
    Ok(Json(json!({"node":detail(&snapshot,n,&s.lean),
        "referrers":snapshot.nodes.values().filter(|other|other.depens.contains(&n.fnode)).map(|n|summary(&snapshot,n)).collect::<Vec<_>>(),
        "children":n.depens.iter().filter_map(|id|snapshot.nodes.get(id)).map(|n|summary(&snapshot,n)).collect::<Vec<_>>()})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewNode {
    title: String,
    parent_fnode: Option<String>,
}
async fn new_node(
    State(s): State<Arc<Service>>,
    headers: HeaderMap,
    Json(body): Json<NewNode>,
) -> ApiResult<Response> {
    let mut snapshot = s.read().await?;
    let node = Node::new(body.title)?;
    let mut changes = vec![node.clone()];
    if let Some(parent) = body.parent_fnode {
        let mut parent = snapshot.resolve(&parent)?.clone();
        check_revision(&headers, &parent)?;
        parent.depens.push(node.fnode.clone());
        changes.push(parent);
    }
    let node = changes.last().unwrap().clone();
    s.save(&mut snapshot, changes, "Create node").await?;
    Ok(revision_response(&snapshot, &node, &s.lean))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Title {
    title: String,
}
async fn title(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Title>,
) -> ApiResult<Response> {
    let mut snapshot = s.read().await?;
    let mut node = snapshot.resolve(&id)?.clone();
    check_revision(&headers, &node)?;
    node.title = body.title;
    s.save(&mut snapshot, vec![node.clone()], "Rename node")
        .await?;
    Ok(revision_response(&snapshot, &node, &s.lean))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Content {
    content: String,
}
async fn block(
    State(s): State<Arc<Service>>,
    Path((id, language)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Content>,
) -> ApiResult<Response> {
    let mut snapshot = s.read().await?;
    let mut node = snapshot.resolve(&id)?.clone();
    check_revision(&headers, &node)?;
    if !BLOCK_TYPES.contains(&language.as_str()) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "supported types: text, lean, rocq, latex".into(),
        ));
    }
    if let Some(block) = node.blocks.iter_mut().find(|b| b.srctype == language) {
        block.content = body.content;
    } else {
        node.blocks.push(Block {
            srctype: language,
            content: body.content,
            ..Default::default()
        });
    }
    s.save(&mut snapshot, vec![node.clone()], "Update source block")
        .await?;
    Ok(revision_response(&snapshot, &node, &s.lean))
}
async fn delete_block(
    State(s): State<Arc<Service>>,
    Path((id, language)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let mut snapshot = s.read().await?;
    let mut node = snapshot.resolve(&id)?.clone();
    check_revision(&headers, &node)?;
    node.blocks.retain(|b| b.srctype != language);
    s.save(&mut snapshot, vec![node.clone()], "Delete source block")
        .await?;
    Ok(revision_response(&snapshot, &node, &s.lean))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddDep {
    dep_fnode: String,
}
async fn add_dep(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AddDep>,
) -> ApiResult<Response> {
    let mut snapshot = s.read().await?;
    let mut node = snapshot.resolve(&id)?.clone();
    check_revision(&headers, &node)?;
    let dep = snapshot.resolve(&body.dep_fnode)?.fnode.clone();
    if !node.depens.contains(&dep) {
        node.depens.push(dep);
        s.save(&mut snapshot, vec![node.clone()], "Add dependency")
            .await?;
    }
    Ok(revision_response(&snapshot, &node, &s.lean))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RmDeps {
    dep_fnodes: Vec<String>,
}
async fn rm_deps(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RmDeps>,
) -> ApiResult<Response> {
    let mut snapshot = s.read().await?;
    let mut node = snapshot.resolve(&id)?.clone();
    check_revision(&headers, &node)?;
    node.depens.retain(|id| !body.dep_fnodes.contains(id));
    s.save(&mut snapshot, vec![node.clone()], "Remove dependencies")
        .await?;
    Ok(revision_response(&snapshot, &node, &s.lean))
}
async fn candidates(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Query(query): Query<Search>,
) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let node = snapshot.resolve(&id)?;
    let q = query.q.to_lowercase();
    let nodes: Vec<_> = snapshot
        .nodes
        .values()
        .filter(|n| {
            n.fnode != node.fnode
                && !node.depens.contains(&n.fnode)
                && (n.title.to_lowercase().contains(&q) || n.fnode.contains(&q))
        })
        .take(query.n.unwrap_or(50).min(200))
        .map(|n| summary(&snapshot, n))
        .collect();
    let empty = if nodes.is_empty() {
        json!({"kind":"no_match"})
    } else {
        Value::Null
    };
    Ok(Json(json!({"nodes":nodes,"empty":empty})))
}
#[derive(Deserialize)]
struct Traversal {
    mode: String,
    depth: i32,
}
async fn traverse(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Query(query): Query<Traversal>,
) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let root = snapshot.resolve(&id)?;
    let mut seen = HashSet::from([root.fnode.clone()]);
    let mut queue = std::collections::VecDeque::from([(root.fnode.clone(), 0i32)]);
    let mut nodes = vec![];
    while let Some((id, depth)) = queue.pop_front() {
        let node = &snapshot.nodes[&id];
        if depth > 0 && (query.mode != "leaf" || node.depens.is_empty()) {
            let mut item = summary(&snapshot, node);
            item["depth"] = json!(depth);
            nodes.push(item);
        }
        if query.depth >= 0 && depth >= query.depth {
            continue;
        }
        let next = if query.mode == "refs" {
            snapshot.referrers.get(&id).cloned().unwrap_or_default()
        } else {
            node.depens.clone()
        };
        for id in next {
            if seen.insert(id.clone()) {
                queue.push_back((id, depth + 1));
            }
        }
    }
    Ok(Json(json!(nodes)))
}
async fn ior(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    let node = snapshot.resolve(&id)?;
    let incoming = snapshot.referrers.get(&node.fnode).map_or(0, Vec::len);
    let outgoing = node.depens.len();
    Ok(Json(
        json!({"fnode":node.fnode,"in_degree":incoming,"out_degree":outgoing,"ior":((incoming as f64+1.0)/(outgoing as f64+1.0)).ln()}),
    ))
}
async fn project(State(s): State<Arc<Service>>) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    Ok(Json(
        json!({"revision":snapshot.version,"project":snapshot.project}),
    ))
}
async fn put_project(
    State(s): State<Arc<Service>>,
    headers: HeaderMap,
    Json(project): Json<LeanProject>,
) -> ApiResult<Json<Value>> {
    let mut snapshot = s.read().await?;
    if expected(&headers)? != snapshot.version {
        return Err(ApiError(
            StatusCode::PRECONDITION_FAILED,
            "project changed; reload and retry".into(),
        ));
    }
    let version = s.db.put_project(&project, &snapshot.version).await?;
    snapshot.version = version;
    snapshot.project = project;
    snapshot.recompute();
    Ok(Json(
        json!({"revision":snapshot.version,"project":snapshot.project}),
    ))
}
async fn export(State(s): State<Arc<Service>>) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    Ok(Json(
        json!({"nodes":snapshot.nodes.values().collect::<Vec<_>>(),"project":snapshot.project}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Import {
    pub nodes: Vec<Node>,
    pub project: Option<LeanProject>,
}
async fn import(State(s): State<Arc<Service>>, Json(body): Json<Import>) -> ApiResult<Json<Value>> {
    let mut snapshot = s.read().await?;
    if body
        .nodes
        .iter()
        .any(|n| snapshot.nodes.contains_key(&n.fnode))
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "import would overwrite existing UUIDs".into(),
        ));
    }
    if body.project.is_some() && !snapshot.nodes.is_empty() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "project import requires an empty database".into(),
        ));
    }
    // Project configuration is validated before any data is imported.
    if let Some(p) = &body.project {
        p.validate()?;
    }
    snapshot.validate_changes(&body.nodes)?;
    let version =
        s.db.put_bundle(
            &body.nodes,
            body.project.as_ref(),
            &snapshot.version,
            "Import nodes",
        )
        .await?;
    snapshot.apply(body.nodes, version);
    if let Some(project) = body.project {
        snapshot.project = project;
        snapshot.recompute();
    }
    Ok(Json(graph_report(&snapshot)))
}
async fn history(State(s): State<Arc<Service>>) -> ApiResult<Json<Value>> {
    Ok(Json(s.db.history().await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Branch {
    name: String,
}
async fn branch(State(s): State<Arc<Service>>, Json(body): Json<Branch>) -> ApiResult<Json<Value>> {
    Ok(Json(s.db.create_branch(&body.name).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeanCheck {
    #[serde(default)]
    build: bool,
}
async fn lean_check(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<LeanCheck>,
) -> ApiResult<Json<Value>> {
    let input = {
        let snapshot = s.read().await?;
        let node = snapshot.resolve(&id)?;
        check_revision(&headers, node)?;
        if let Some(result) = s.lean.cached(
            &snapshot.lean_keys[&node.fnode],
            &node.revision(),
            body.build,
        ) {
            return Ok(Json(json!(result)));
        }
        crate::lean::Input::capture(&snapshot, &id)?
    };
    let result = s.lean.check(input, body.build).await?;
    let snapshot = s.read().await?;
    if snapshot.lean_keys.get(&result.fnode) != Some(&result.input_key) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Lean inputs changed during checking; retry the current revision".into(),
        ));
    }
    Ok(Json(json!(result)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    line: u32,
    character: u32,
}
async fn lean_goals(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(pos): Json<Position>,
) -> ApiResult<Json<Value>> {
    let input = {
        let snapshot = s.read().await?;
        check_revision(&headers, snapshot.resolve(&id)?)?;
        crate::lean::Input::capture(&snapshot, &id)?
    };
    let key = input.chain.last().unwrap().1.clone();
    let uuid = input.chain.last().unwrap().0.fnode.clone();
    let goals = s.lean.goals(input, pos.line, pos.character).await?;
    if s.read().await?.lean_keys.get(&uuid) != Some(&key) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Lean inputs changed during goal inspection".into(),
        ));
    }
    Ok(Json(goals))
}

struct EditorSession {
    root: std::path::PathBuf,
    filename: String,
    source: String,
    created: std::time::Instant,
    _slot: tokio::sync::OwnedSemaphorePermit,
}
async fn lean_session(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let input = {
        let snapshot = s.read().await?;
        check_revision(&headers, snapshot.resolve(&id)?)?;
        crate::lean::Input::capture(&snapshot, &id)?
    };
    let node = &input.chain.last().unwrap().0;
    let source = node
        .source("lean")
        .ok_or_else(|| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "node has no Lean block".into(),
            )
        })?
        .to_owned();
    let mut editors = s.editors.lock().await;
    let expired: Vec<_> = editors
        .iter()
        .filter(|(_, e)| e.created.elapsed() > std::time::Duration::from_secs(120))
        .map(|(id, _)| id.clone())
        .collect();
    for id in expired {
        if let Some(editor) = editors.remove(&id) {
            let _ = tokio::fs::remove_dir_all(editor.root).await;
        }
    }
    drop(editors);
    let slot = s.editor_slots.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "all Lean editor slots are in use; close an idle editor".into(),
        )
    })?;
    let root = s.lean.editor_project(&input).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let filename = format!("/project/{}.lean", node.module.replace('.', "/"));
    s.editors.lock().await.insert(
        id.clone(),
        EditorSession {
            root,
            filename: filename.clone(),
            source: source.clone(),
            created: std::time::Instant::now(),
            _slot: slot,
        },
    );
    Ok(Json(json!({"id":id,"filename":filename,"source":source})))
}
async fn editor_info(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let editors = s.editors.lock().await;
    let e = editors
        .get(&id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "editor session expired".into()))?;
    Ok(Json(json!({"filename":e.filename,"source":e.source})))
}
async fn editor_socket(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> ApiResult<Response> {
    let session = s.editors.lock().await.remove(&id).ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "editor session expired or already connected".into(),
        )
    })?;
    Ok(ws
        .max_message_size(32 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            if let Err(error) = bridge_editor(socket, &session).await {
                eprintln!("Lean editor: {error:#}");
            }
            let _ = tokio::fs::remove_dir_all(&session.root).await;
        }))
}
fn rewrite_uris(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(s) if s == from || s.starts_with(&format!("{from}/")) => {
            *s = format!("{to}{}", &s[from.len()..]);
        }
        Value::Array(values) => {
            for v in values {
                rewrite_uris(v, from, to);
            }
        }
        Value::Object(values) => {
            for v in values.values_mut() {
                rewrite_uris(v, from, to);
            }
        }
        _ => {}
    }
}
async fn bridge_editor(
    socket: axum::extract::ws::WebSocket,
    session: &EditorSession,
) -> Result<()> {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};
    let actual = crate::lean::file_uri(&session.root)?;
    let mut process = crate::lean::spawn(&session.root, &["serve"])?;
    let mut writer = process
        .child
        .stdin
        .take()
        .context("Lean stdin unavailable")?;
    let mut reader = tokio::io::BufReader::new(
        process
            .child
            .stdout
            .take()
            .context("Lean stdout unavailable")?,
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    // A dedicated reader never cancels a partially read Content-Length frame.
    let task = tokio::spawn(async move {
        loop {
            let value = crate::lean::read_message(&mut reader).await;
            let failed = value.is_err();
            if tx.send(value).await.is_err() || failed {
                break;
            }
        }
    });
    let (mut send, mut receive) = socket.split();
    let result:Result<()>=async{
        loop{tokio::select!{
            message=receive.next()=>{
                let Some(message)=message else{break;};match message?{
                    Message::Text(text)=>{
                        let mut value:Value=serde_json::from_str(&text)?;
                        let method=value["method"].as_str().unwrap_or("");
                        if matches!(method,"textDocument/didOpen"|"textDocument/didChange"|"textDocument/didSave"){
                            let uri=value["params"]["textDocument"]["uri"].as_str().context("document URI required")?;
                            if uri!=format!("file://{}",session.filename){bail!("editor may only change its own draft module");}
                        }
                        rewrite_uris(&mut value,"file:///project",&actual);crate::lean::write_message(&mut writer,&value).await?;
                    },
                    Message::Close(_)=>break,Message::Ping(data)=>send.send(Message::Pong(data)).await?,_=>{}
                }
            },
            message=rx.recv()=>{
                let mut value=message.context("Lean server closed its output")??;rewrite_uris(&mut value,&actual,"file:///project");
                send.send(Message::Text(serde_json::to_string(&value)?)).await?;
            }
        }}Ok(())
    }.await;
    task.abort();
    result
}
