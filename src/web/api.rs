use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::core::{GraphCheckReport, GraphRootItem};
use crate::indcache::IndCache;
use crate::mdocnode::{MdocNode, SrcBlock};
use crate::workspace::to_rel_path;

use super::AppState;

/// `bail!`-equivalent that targets `ApiResult` (which needs an
/// `ApiError`, not a bare `anyhow::Error`). `bail!` itself returns
/// `Err(anyhow::Error)` without `.into()`, so it cannot be used directly in
/// functions returning `ApiResult`.
macro_rules! bail {
    ($($t:tt)*) => {
        return Err(ApiError::validation(format!($($t)*)))
    };
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

/// Minimal node summary used in lists (referrers, children, search results).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
    pub broken: bool,
    pub depth: u32,
}

/// Full node detail returned by `GET /api/node/:fnode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDetail {
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
    pub broken: bool,
    pub depth: u32,
    /// Direct dependency fnodes (in source order, deduplicated).
    pub depens: Vec<String>,
    pub blocks: Vec<crate::mdocnode::SrcBlock>,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_n")]
    pub n: usize,
}
fn default_n() -> usize {
    200
}
const MAX_SEARCH_RESULTS: usize = 200;

#[derive(Debug, Serialize)]
pub struct ResolveResponse {
    pub fnode: String,
    pub title: String,
    pub rel_path: String,
}

/// Full workspace graph: nodes + edges, for the force-directed view.
#[derive(Debug, Serialize)]
pub struct GraphFull {
    pub nodes: Vec<NodeInfo>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
}

// ── Error handling ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum ApiErrorKind {
    NotFound,
    Validation,
    Conflict,
    Internal,
}

#[derive(Debug)]
pub struct ApiError {
    kind: ApiErrorKind,
    public_message: String,
    detail: anyhow::Error,
}

impl ApiError {
    fn new(kind: ApiErrorKind, public_message: impl Into<String>, detail: anyhow::Error) -> Self {
        Self {
            kind,
            public_message: public_message.into(),
            detail,
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        let message = message.into();
        Self::new(
            ApiErrorKind::Validation,
            message.clone(),
            anyhow::anyhow!(message),
        )
    }

    fn rejected(detail: anyhow::Error) -> Self {
        if detail
            .downcast_ref::<crate::workspace::FileConflict>()
            .is_some()
            || detail.downcast_ref::<std::io::Error>().is_some()
            || detail.downcast_ref::<rusqlite::Error>().is_some()
        {
            Self::from(detail)
        } else {
            Self::new(
                ApiErrorKind::Validation,
                "request could not be applied",
                detail,
            )
        }
    }

    fn from_resolve(detail: anyhow::Error) -> Self {
        match detail.downcast_ref::<crate::indcache::ResolveRefError>() {
            Some(crate::indcache::ResolveRefError::NotFound(_)) => {
                Self::new(ApiErrorKind::NotFound, "node not found", detail)
            }
            Some(crate::indcache::ResolveRefError::Empty) => Self::new(
                ApiErrorKind::Validation,
                "reference cannot be empty",
                detail,
            ),
            Some(crate::indcache::ResolveRefError::Ambiguous { .. }) => {
                Self::new(ApiErrorKind::Validation, "reference is ambiguous", detail)
            }
            Some(crate::indcache::ResolveRefError::Invalid(_)) => Self::new(
                ApiErrorKind::Validation,
                "reference points to an invalid node",
                detail,
            ),
            None => Self::from(detail),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(detail: anyhow::Error) -> Self {
        if detail
            .downcast_ref::<crate::workspace::FileConflict>()
            .is_some()
        {
            Self::new(
                ApiErrorKind::Conflict,
                "resource changed; refresh and retry",
                detail,
            )
        } else {
            Self::new(ApiErrorKind::Internal, "internal server error", detail)
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.kind {
            ApiErrorKind::NotFound => StatusCode::NOT_FOUND,
            ApiErrorKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorKind::Conflict => StatusCode::CONFLICT,
            ApiErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if matches!(self.kind, ApiErrorKind::Internal) {
            eprintln!(
                "web API internal error: {}",
                crate::core::escape_terminal(&self.detail.to_string())
            );
        }
        (
            status,
            Json(serde_json::json!({ "error": self.public_message })),
        )
            .into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

pub async fn api_not_found() -> Response {
    json_error_response(StatusCode::NOT_FOUND, "API route not found")
}

pub async fn normalize_error_response(request: axum::extract::Request, next: Next) -> Response {
    let response = next.run(request).await;
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }
    let is_json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if is_json {
        return response;
    }
    let status = response.status();
    let message = match status {
        StatusCode::BAD_REQUEST => "invalid request",
        StatusCode::NOT_FOUND => "API route not found",
        StatusCode::METHOD_NOT_ALLOWED => "method not allowed",
        _ if status.is_client_error() => "request rejected",
        _ => "internal server error",
    };
    json_error_response(status, message)
}

fn json_error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

// ── Cache helpers ─────────────────────────────────────────────────────────────

/// Lock the cache, run a closure, return the result.
fn with_cache<R>(
    state: &AppState,
    f: impl FnOnce(&mut IndCache) -> anyhow::Result<R>,
) -> ApiResult<R> {
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    Ok(f(&mut cache)?)
}

/// Serialize each complete read-modify-write operation. Locking only individual
/// cache calls would still allow two handlers to load the same old file and
/// overwrite each other's changes, or to race cycle checks.
fn with_mutation<R>(state: &AppState, f: impl FnOnce() -> ApiResult<R>) -> ApiResult<R> {
    let _guard = state.mutation_lock.lock().expect("mutation mutex poisoned");
    f()
}

fn with_workspace_mutation<R>(state: &AppState, f: impl FnOnce() -> ApiResult<R>) -> ApiResult<R> {
    let _guard = state.mutation_lock.lock().expect("mutation mutex poisoned");
    let _workspace_guard = crate::workspace::WorkspaceMutationLock::acquire(&state.mdcroot)?;
    f()
}

/// Resolve a ref (fnode, prefix, or path) and return (fnode, title, abs_path).
fn resolve(state: &AppState, raw: &str) -> ApiResult<(String, String, std::path::PathBuf)> {
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    cache.discover_workspace_changes()?;
    cache
        .resolve_ref(raw, Some(&state.mdcroot))
        .map_err(ApiError::from_resolve)
}

/// Build a NodeInfo from (fnode, title, rel_path), fetching broken + depth.
fn node_info(state: &AppState, fnode: &str, title: &str, rel_path: &str) -> ApiResult<NodeInfo> {
    let (broken, depth) = with_cache(state, |c| {
        let broken = c.has_issues(fnode)?;
        let depth = c.all_topo_depths()?.get(fnode).copied().unwrap_or(0);
        Ok::<_, anyhow::Error>((broken, depth))
    })?;
    Ok(NodeInfo {
        fnode: fnode.to_string(),
        title: title.to_string(),
        rel_path: rel_path.to_string(),
        broken,
        depth,
    })
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn graph_roots(State(state): State<AppState>) -> ApiResult<Json<Vec<GraphRootItem>>> {
    let roots = with_cache(&state, |c| {
        c.discover_workspace_changes()?;
        c.global_root_items()
    })?;
    Ok(Json(roots))
}

pub async fn graph_check(State(state): State<AppState>) -> ApiResult<Json<GraphCheckReport>> {
    let report = with_cache(&state, |c| {
        c.refresh_workspace_index()?;
        c.graph_check_report()
    })?;
    Ok(Json(report))
}

/// Full workspace graph for the force-directed view: all valid nodes + edges.
pub async fn graph_full(State(state): State<AppState>) -> ApiResult<Json<GraphFull>> {
    let (nodes, edges) = with_cache(&state, |c| {
        c.discover_workspace_changes()?;
        let nodes: Vec<NodeInfo> = c
            .search_with_metadata("", usize::MAX)?
            .into_iter()
            .filter(|item| !item.broken)
            .map(|item| NodeInfo {
                fnode: item.fnode,
                title: item.title,
                rel_path: item.rel_path,
                broken: false,
                depth: item.depth,
            })
            .collect();
        let edges_raw = c.all_valid_edges()?;
        // Filter edges to only those whose both endpoints are in the node set.
        let known: std::collections::HashSet<&str> =
            nodes.iter().map(|n| n.fnode.as_str()).collect();
        let edges: Vec<GraphEdge> = edges_raw
            .into_iter()
            .filter(|(s, d)| known.contains(s.as_str()) && known.contains(d.as_str()))
            .map(|(source, target)| GraphEdge { source, target })
            .collect();
        Ok::<_, anyhow::Error>((nodes, edges))
    })?;
    Ok(Json(GraphFull { nodes, edges }))
}

pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<Vec<NodeInfo>>> {
    let limit = q.n.min(MAX_SEARCH_RESULTS);
    let out = with_cache(&state, |c| {
        c.discover_workspace_changes()?;
        let rows = c.search_with_metadata(&q.q, limit)?;
        Ok::<_, anyhow::Error>(
            rows.into_iter()
                .map(|item| NodeInfo {
                    fnode: item.fnode,
                    title: item.title,
                    rel_path: item.rel_path,
                    broken: item.broken,
                    depth: item.depth,
                })
                .collect(),
        )
    })?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    pub r#ref: String,
}

pub async fn resolve_ref(
    State(state): State<AppState>,
    Query(q): Query<ResolveQuery>,
) -> ApiResult<Json<ResolveResponse>> {
    let (fnode, title, abs_path) = resolve(&state, &q.r#ref)?;
    let rel_path = to_rel_path(&state.mdcroot, &abs_path);
    Ok(Json(ResolveResponse {
        fnode,
        title,
        rel_path,
    }))
}

pub async fn node_detail(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
) -> ApiResult<Json<NodeDetail>> {
    let (fnode, title, abs_path) = resolve(&state, &fnode)?;
    let rel_path = to_rel_path(&state.mdcroot, &abs_path);
    let node = MdocNode::load(&state.mdcroot, &abs_path)?;
    let info = node_info(&state, &fnode, &title, &rel_path)?;
    Ok(Json(NodeDetail {
        fnode: info.fnode,
        title: info.title,
        rel_path: info.rel_path,
        broken: info.broken,
        depth: info.depth,
        depens: node.depens.clone(),
        blocks: node.blocks,
    }))
}

pub async fn node_referrers(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
) -> ApiResult<Json<Vec<NodeInfo>>> {
    // Resolve first so prefix refs work and the node is indexed.
    let (fnode, _, _) = resolve(&state, &fnode)?;
    let rows = with_cache(&state, |c| c.direct_referrers_for_fnode(&fnode))?;
    let mut out = Vec::with_capacity(rows.len());
    for (rf, rt, rp) in rows {
        out.push(node_info(&state, &rf, &rt, &rp)?);
    }
    Ok(Json(out))
}

pub async fn node_children(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
) -> ApiResult<Json<Vec<NodeInfo>>> {
    let (fnode, _, _) = resolve(&state, &fnode)?;
    let report = with_cache(&state, |c| c.dependency_report(&fnode, 1))?;
    let mut out = Vec::new();
    for item in report.items.into_iter().filter(|i| i.depth == 1) {
        let broken = report.issues_by_fnode.contains_key(&item.fnode);
        let depth = with_cache(&state, |c| {
            Ok::<_, anyhow::Error>(c.all_topo_depths()?.get(&item.fnode).copied().unwrap_or(0))
        })?;
        out.push(NodeInfo {
            fnode: item.fnode,
            title: item.title,
            rel_path: item.rel_path,
            broken,
            depth,
        });
    }
    Ok(Json(out))
}

// ── Write handlers ────────────────────────────────────────────────────────────

/// Replace a single srctype block's content on the focused node.
/// If the block does not yet exist, it is appended.
pub async fn node_put_block(
    State(state): State<AppState>,
    Path((fnode, srctype)): Path<(String, String)>,
    Json(body): Json<BlockBody>,
) -> ApiResult<Json<NodeDetail>> {
    with_workspace_mutation(&state, || {
        validate_srctype(&srctype)?;
        let (fnode, _, abs_path) = resolve(&state, &fnode)?;
        let (snapshot, mut node) = snapshot_node(&state, &abs_path)?;
        if node.fnode != fnode {
            bail!("fnode mismatch when writing block");
        }

        let content = normalize_block_content(&body.content);
        match node.blocks.iter_mut().find(|b| b.srctype == srctype) {
            Some(block) => block.content = content,
            None => node.blocks.push(SrcBlock {
                srctype,
                content,
                metadata: Default::default(),
            }),
        }
        save_and_index(&state, &node, &snapshot)?;
        Ok(committed_node_detail(&state, &node))
    })
}

/// Delete a single srctype block from the focused node.
pub async fn node_delete_block(
    State(state): State<AppState>,
    Path((fnode, srctype)): Path<(String, String)>,
) -> ApiResult<Json<NodeDetail>> {
    with_workspace_mutation(&state, || {
        validate_srctype(&srctype)?;
        let (fnode, _, abs_path) = resolve(&state, &fnode)?;
        let (snapshot, mut node) = snapshot_node(&state, &abs_path)?;
        if node.fnode != fnode {
            bail!("fnode mismatch when deleting block");
        }
        let before = node.blocks.len();
        node.blocks.retain(|b| b.srctype != srctype);
        if node.blocks.len() == before {
            bail!("no '@src: {srctype}' block on this node");
        }
        save_and_index(&state, &node, &snapshot)?;
        Ok(committed_node_detail(&state, &node))
    })
}

/// Update the @title of the focused node.
pub async fn node_put_title(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
    Json(body): Json<TitleBody>,
) -> ApiResult<Json<NodeDetail>> {
    with_workspace_mutation(&state, || {
        let title = body.title.trim();
        if title.is_empty() {
            bail!("@title must be non-empty");
        }
        let (fnode, _, abs_path) = resolve(&state, &fnode)?;
        let (snapshot, mut node) = snapshot_node(&state, &abs_path)?;
        if node.fnode != fnode {
            bail!("fnode mismatch when updating title");
        }
        node.title = title.to_string();
        save_and_index(&state, &node, &snapshot)?;
        Ok(committed_node_detail(&state, &node))
    })
}

// ── Write helpers ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct BlockBody {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct TitleBody {
    pub title: String,
}

/// The five built-in srctypes. Rejecting unknown srctypes keeps the work/back
/// pipeline (which keys off the compiler registry) consistent.
fn validate_srctype(srctype: &str) -> ApiResult<()> {
    if crate::config::BUILTIN_SRCTYPES.contains(&srctype) {
        Ok(())
    } else {
        bail!("unsupported srctype '{srctype}'")
    }
}

/// Normalise block content so save→load→save is stable.
/// MdocNode::save() writes block content via `content.lines()` which drops a
/// trailing newline, so the canonical stored form has no trailing newline.
fn normalize_block_content(raw: &str) -> String {
    let mut s = raw.to_string();
    while s.ends_with('\n') {
        s.pop();
    }
    s
}

/// Once persistence and indexing succeed, response construction is infallible:
/// callers receive the committed node even if optional derived metadata is unavailable.
fn committed_node_detail(state: &AppState, node: &MdocNode) -> Json<NodeDetail> {
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    Json(node_detail_from_committed_cache(
        &mut cache,
        &state.mdcroot,
        node,
    ))
}

fn committed_graph_detail(
    state: &AppState,
    mut graph: crate::depgraph::DepGraph,
) -> Json<NodeDetail> {
    let node = graph.root_node().clone();
    let mdcroot = graph.mdcroot().to_path_buf();
    let detail = node_detail_from_committed_cache(graph.cache_mut(), &mdcroot, &node);
    *state.cache.lock().expect("cache mutex poisoned") = graph.into_cache();
    Json(detail)
}

fn node_detail_from_committed_cache(
    cache: &mut IndCache,
    mdcroot: &std::path::Path,
    node: &MdocNode,
) -> NodeDetail {
    let broken = cache.has_issues(&node.fnode).unwrap_or(true);
    let depth = cache
        .all_topo_depths()
        .ok()
        .and_then(|depths| depths.get(&node.fnode).copied())
        .unwrap_or(0);
    let root = mdcroot
        .canonicalize()
        .unwrap_or_else(|_| mdcroot.to_path_buf());
    let mut blocks = node.blocks.clone();
    for block in &mut blocks {
        if !block.content.is_empty() && !block.content.ends_with('\n') {
            block.content.push('\n');
        }
    }
    NodeDetail {
        fnode: node.fnode.clone(),
        title: node.title.clone(),
        rel_path: to_rel_path(&root, &node.path),
        broken,
        depth,
        depens: node.depens.clone(),
        blocks,
    }
}

fn snapshot_node(
    state: &AppState,
    abs_path: &std::path::Path,
) -> ApiResult<(crate::workspace::FileSnapshot, MdocNode)> {
    let snapshot = crate::workspace::FileSnapshot::capture(abs_path)?;
    let content = snapshot
        .content()
        .ok_or_else(|| anyhow::anyhow!("mdoc file disappeared: {}", abs_path.display()))?;
    let node = MdocNode::load_bytes(&state.mdcroot, abs_path, content)?;
    Ok((snapshot, node))
}

fn save_and_index(
    state: &AppState,
    node: &MdocNode,
    snapshot: &crate::workspace::FileSnapshot,
) -> ApiResult<()> {
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    crate::depgraph::replace_indexed_node(&mut cache, node, snapshot).map_err(ApiError::rejected)
}

// ── Dependency mutation handlers ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AddDepBody {
    pub dep_fnode: String,
}

/// Add a direct dependency to the focused node. Cycles are rejected by
/// DepGraph::add_direct_dependencies.
pub async fn node_add_dep(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
    Json(body): Json<AddDepBody>,
) -> ApiResult<Json<NodeDetail>> {
    with_mutation(&state, || {
        let (fnode, _, _) = resolve(&state, &fnode)?;
        let mut graph = crate::depgraph::DepGraph::new(state.mdcroot.clone(), &fnode)?;
        let (added, _, _) = graph
            .add_direct_dependency_ref(&body.dep_fnode)
            .map_err(ApiError::rejected)?;
        if added.is_empty() {
            bail!("dependency already present or equals self");
        }
        Ok(committed_graph_detail(&state, graph))
    })
}

#[derive(Debug, Deserialize)]
pub struct RmDepBody {
    pub dep_fnodes: Vec<String>,
}

/// Remove direct dependencies from the focused node.
pub async fn node_rm_deps(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
    Json(body): Json<RmDepBody>,
) -> ApiResult<Json<NodeDetail>> {
    with_mutation(&state, || {
        if body.dep_fnodes.is_empty() {
            bail!("dep_fnodes must be non-empty");
        }
        let (fnode, _, _) = resolve(&state, &fnode)?;
        let mut graph = crate::depgraph::DepGraph::new(state.mdcroot.clone(), &fnode)?;
        let removed = graph
            .remove_direct_dependencies(body.dep_fnodes)
            .map_err(ApiError::rejected)?;
        if removed.is_empty() {
            bail!("none of the given fnodes are direct dependencies");
        }
        Ok(committed_graph_detail(&state, graph))
    })
}

#[derive(Debug, Deserialize)]
pub struct NewNodeBody {
    pub title: String,
    /// Optional relative path (without .mdoc suffix). Defaults to {fnode}.mdoc.
    pub file: Option<String>,
    /// If set, the new node is added as a direct dependency of this node.
    pub parent_fnode: Option<String>,
}

/// Create a new .mdoc file. If `parent_fnode` is given, also add it as a
/// dependency of that node (cycle-checked, atomic via DepGraph).
pub async fn node_new(
    State(state): State<AppState>,
    Json(body): Json<NewNodeBody>,
) -> ApiResult<Json<NodeDetail>> {
    with_mutation(&state, || {
        let title = body.title.trim();
        if title.is_empty() {
            bail!("title must be non-empty");
        }
        let file_path = body.file.as_deref().unwrap_or(".").trim();

        if let Some(parent) = &body.parent_fnode {
            // Resolve parent first so we can produce a clear error before write.
            let (parent_fnode, _, _) = resolve(&state, parent)?;
            let mut graph = crate::depgraph::DepGraph::new(state.mdcroot.clone(), &parent_fnode)?;
            let mut new_node = crate::mdocnode::MdocNode::new_at_path(
                &state.mdcroot,
                &state.mdcroot.join("."),
                title,
            );
            new_node.path =
                crate::depgraph::resolve_new_node_path(&state.mdcroot, file_path, &new_node.fnode)
                    .map_err(ApiError::rejected)?;
            graph
                .create_and_add_dependency(new_node)
                .map_err(ApiError::rejected)?;
            // Return the parent (the user is editing the parent and just added a
            // dep — they want to see it appear in the children column).
            Ok(committed_graph_detail(&state, graph))
        } else {
            // Standalone new node, no parent.
            let graph = crate::depgraph::DepGraph::create_root(
                state.mdcroot.clone(),
                file_path,
                title,
                None,
                None,
            )
            .map_err(ApiError::rejected)?;
            Ok(committed_graph_detail(&state, graph))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_state() -> (tempfile::TempDir, AppState, std::path::PathBuf, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir(root.join(".mdc")).unwrap();
        let path = root.join("node.mdoc");
        let mut node = MdocNode::new_at_path(&root, &path, "Original");
        node.blocks.push(SrcBlock {
            srctype: "latex".to_string(),
            content: "original block".to_string(),
            metadata: Default::default(),
        });
        let fnode = node.fnode.clone();
        node.save_new().unwrap();

        let mut cache = IndCache::open(root.clone()).unwrap();
        cache.refresh_all().unwrap();
        let state = AppState::new(root, cache);
        (dir, state, path, fnode)
    }

    #[test]
    fn title_block_and_delete_conflicts_preserve_external_edit_and_index() {
        for operation in ["title", "block", "delete"] {
            let (_dir, state, path, fnode) = setup_state();
            let (snapshot, mut desired) = snapshot_node(&state, &path).unwrap();
            match operation {
                "title" => desired.title = "Requested title".to_string(),
                "block" => desired.blocks[0].content = "requested block".to_string(),
                "delete" => desired.blocks.clear(),
                _ => unreachable!(),
            }

            // Deterministic failpoint: another writer commits after our parse but
            // before replacement.
            let mut external = MdocNode::load(&state.mdcroot, &path).unwrap();
            external.title = format!("External edit during {operation}");
            external.save().unwrap();
            let external_bytes = std::fs::read(&path).unwrap();

            let error = save_and_index(&state, &desired, &snapshot).unwrap_err();
            assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
            assert_eq!(std::fs::read(&path).unwrap(), external_bytes);

            let cache = state.cache.lock().unwrap();
            let rows = cache.exact_fnode_rows(&fnode).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, "Original");
        }
    }

    #[test]
    fn post_commit_cache_failure_returns_committed_graph_state() {
        let (_dir, state, path, fnode) = setup_state();
        let target_path = state.mdcroot.join("target.mdoc");
        let target = MdocNode::new_at_path(&state.mdcroot, &target_path, "Target");
        let target_fnode = target.fnode.clone();
        target.save_new().unwrap();

        let mut graph = crate::depgraph::DepGraph::new(state.mdcroot.clone(), &fnode).unwrap();
        graph.add_direct_dependency_ref(&target_fnode).unwrap();

        // Fault injection at the old post-commit refresh point: this cache belongs
        // to another workspace, so refreshing `path` through it would fail.
        let other = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(other.path().join(".mdc")).unwrap();
        let other_cache = IndCache::open(other.path().to_path_buf()).unwrap();
        *state.cache.lock().unwrap() = other_cache;

        let detail = committed_graph_detail(&state, graph).0;

        assert_eq!(detail.depens, vec![target_fnode.clone()]);
        assert_eq!(
            MdocNode::load(&state.mdcroot, &path).unwrap().depens,
            vec![target_fnode]
        );
        assert_eq!(
            state.cache.lock().unwrap().db_path(),
            state.mdcroot.canonicalize().unwrap().join(".mdc/index.db")
        );
    }
}
