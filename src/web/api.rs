use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::core::{
    DependencyCandidates, FormalizationStatus, GraphCheckReport, GraphRootItem, NodeSummary,
};
use crate::indcache::IndCache;
use crate::mdocnode::MdocNode;
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
    pub formalization: FormalizationStatus,
}

/// Focused node data needed by the three-column browser in one response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeView {
    pub node: NodeDetail,
    pub referrers: Vec<NodeSummary>,
    pub children: Vec<NodeSummary>,
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
    pub nodes: Vec<NodeSummary>,
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

    fn generation_conflict(expected_fnode: &str, loaded_fnode: &str) -> Self {
        Self::new(
            ApiErrorKind::Conflict,
            "resource changed; refresh and retry",
            anyhow::anyhow!("resolved node generation {expected_fnode}, but loaded {loaded_fnode}"),
        )
    }

    fn snapshot_conflict(path: &std::path::Path) -> Self {
        Self::new(
            ApiErrorKind::Conflict,
            "resource changed; refresh and retry",
            anyhow::anyhow!("{} changed while loading node data", path.display()),
        )
    }

    fn rejected(detail: anyhow::Error) -> Self {
        if crate::workspace::error_has_file_conflict(&detail)
            || crate::workspace::error_has_infrastructure_failure(&detail)
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
        if crate::workspace::error_has_infrastructure_failure(&detail) {
            Self::new(ApiErrorKind::Internal, "internal server error", detail)
        } else if crate::workspace::error_has_file_conflict(&detail) {
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

fn with_workspace_mutation<R>(
    state: &AppState,
    f: impl FnOnce(&mut IndCache, &crate::workspace::WorkspaceMutationLock) -> ApiResult<R>,
) -> ApiResult<R> {
    let _process_guard = state.mutation_lock.lock().expect("mutation mutex poisoned");
    let root = {
        let cache = state.cache.lock().expect("cache mutex poisoned");
        cache.root().to_path_buf()
    };
    let mutation_lock = crate::workspace::WorkspaceMutationLock::acquire(&root)?;
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    cache.validate_mutation_lock(&mutation_lock)?;
    f(&mut cache, &mutation_lock)
}

fn resolve_with_cache(
    cache: &mut IndCache,
    raw: &str,
) -> anyhow::Result<(String, String, std::path::PathBuf)> {
    cache.discover_workspace_changes()?;
    cache.resolve_ref(raw, Some(cache.root()))
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn graph_roots(State(state): State<AppState>) -> ApiResult<Json<Vec<GraphRootItem>>> {
    let _profile = crate::profile::scope("web::api::graph_roots");
    let roots = with_cache(&state, |c| {
        c.discover_workspace_changes()?;
        c.global_root_items()
    })?;
    Ok(Json(roots))
}

pub async fn graph_check(State(state): State<AppState>) -> ApiResult<Json<GraphCheckReport>> {
    let report = with_cache(&state, |c| {
        c.refresh_all()?;
        c.graph_check_report()
    })?;
    Ok(Json(report))
}

/// Full workspace graph for the force-directed view: all valid nodes + edges.
pub async fn graph_full(State(state): State<AppState>) -> ApiResult<Json<GraphFull>> {
    let _profile = crate::profile::scope("web::api::graph_full");
    let (nodes, edges) = with_cache(&state, |c| {
        c.discover_workspace_changes()?;
        let nodes: Vec<NodeSummary> = c
            .all_node_summaries()?
            .into_iter()
            .filter(|item| !item.broken)
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
) -> ApiResult<Json<Vec<NodeSummary>>> {
    let limit = q.n.min(MAX_SEARCH_RESULTS);
    let out = with_cache(&state, |c| {
        c.discover_workspace_changes()?;
        c.search(&q.q, limit)
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
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    let (fnode, title, abs_path) =
        resolve_with_cache(&mut cache, &q.r#ref).map_err(ApiError::from_resolve)?;
    let rel_path = to_rel_path(cache.root(), &abs_path);
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
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    let (fnode, _, abs_path) =
        resolve_with_cache(&mut cache, &fnode).map_err(ApiError::from_resolve)?;
    let (snapshot, node) = load_node_generation(&mut cache, &fnode, &abs_path)?;
    let cache_fields = (|| {
        Ok::<_, anyhow::Error>((
            cache.node_summary(&fnode)?,
            cache.formalization_status(&fnode)?,
        ))
    })();
    ensure_snapshot_unchanged(&snapshot, &abs_path)?;
    let (summary, formalization) = cache_fields?;
    Ok(Json(node_detail_from_generation(
        summary,
        node,
        formalization,
    )))
}

pub async fn node_view(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
) -> ApiResult<Json<NodeView>> {
    let _profile = crate::profile::scope("web::api::node_view");
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    let (fnode, _, abs_path) =
        resolve_with_cache(&mut cache, &fnode).map_err(ApiError::from_resolve)?;
    let (snapshot, node) = load_node_generation(&mut cache, &fnode, &abs_path)?;
    let cache_fields = (|| {
        Ok::<_, anyhow::Error>((
            cache.node_summary(&fnode)?,
            cache.formalization_status(&fnode)?,
            cache.direct_referrer_summaries(&fnode)?,
            cache.direct_dependency_summaries(&fnode)?,
        ))
    })();
    ensure_snapshot_unchanged(&snapshot, &abs_path)?;
    let (summary, formalization, referrers, children) = cache_fields?;
    Ok(Json(NodeView {
        node: node_detail_from_generation(summary, node, formalization),
        referrers,
        children,
    }))
}

pub async fn node_children(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
) -> ApiResult<Json<Vec<NodeSummary>>> {
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    let (fnode, _, _) = resolve_with_cache(&mut cache, &fnode).map_err(ApiError::from_resolve)?;
    let out = cache.direct_dependency_summaries(&fnode)?;
    Ok(Json(out))
}

fn load_node_generation(
    cache: &mut IndCache,
    fnode: &str,
    abs_path: &std::path::Path,
) -> ApiResult<(crate::workspace::FileSnapshot, MdocNode)> {
    let (snapshot, node) = snapshot_node(abs_path)?;
    if node.fnode != fnode {
        return Err(ApiError::generation_conflict(fnode, &node.fnode));
    }

    // This is deliberately a strong single-path upsert: discovery's metadata
    // fast path cannot detect every external edit.
    if let Err(error) = cache.upsert_path(abs_path) {
        ensure_snapshot_unchanged(&snapshot, abs_path)?;
        return Err(error.into());
    }
    ensure_snapshot_unchanged(&snapshot, abs_path)?;
    Ok((snapshot, node))
}

fn ensure_snapshot_unchanged(
    snapshot: &crate::workspace::FileSnapshot,
    abs_path: &std::path::Path,
) -> ApiResult<()> {
    if snapshot.unchanged(abs_path)? {
        Ok(())
    } else {
        Err(ApiError::snapshot_conflict(abs_path))
    }
}

fn node_detail_from_generation(
    info: NodeSummary,
    node: MdocNode,
    formalization: FormalizationStatus,
) -> NodeDetail {
    NodeDetail {
        fnode: info.fnode,
        title: info.title,
        rel_path: info.rel_path,
        broken: info.broken,
        depth: info.depth,
        depens: node.depens,
        blocks: node.blocks,
        formalization,
    }
}

pub async fn node_dependency_candidates(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<DependencyCandidates>> {
    let limit = q.n.min(MAX_SEARCH_RESULTS);
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    cache.discover_workspace_changes()?;
    let (fnode, _, _) = cache
        .resolve_ref(&fnode, Some(cache.root()))
        .map_err(ApiError::from_resolve)?;
    let out = cache.dependency_candidates(&fnode, &q.q, limit)?;
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
    let srctype = validate_srctype(&srctype)?;
    let content = body.content;
    mutate_node(&state, &fnode, move |node| {
        node.upsert_source_block(srctype, content)?;
        Ok(())
    })
}

/// Delete a single srctype block from the focused node.
pub async fn node_delete_block(
    State(state): State<AppState>,
    Path((fnode, srctype)): Path<(String, String)>,
) -> ApiResult<Json<NodeDetail>> {
    let srctype = validate_srctype(&srctype)?;
    mutate_node(&state, &fnode, |node| {
        if !node.remove_source_block(srctype) {
            bail!("no '@src: {srctype}' block on this node");
        }
        Ok(())
    })
}

/// Update the @title of the focused node.
pub async fn node_put_title(
    State(state): State<AppState>,
    Path(fnode): Path<String>,
    Json(body): Json<TitleBody>,
) -> ApiResult<Json<NodeDetail>> {
    let title = body.title.trim();
    if title.is_empty() {
        bail!("@title must be non-empty");
    }
    let title = title.to_string();
    mutate_node(&state, &fnode, move |node| {
        node.set_title(title);
        Ok(())
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
fn validate_srctype(srctype: &str) -> ApiResult<&'static str> {
    crate::config::builtin_srctype(srctype).map_err(|error| ApiError::validation(error.to_string()))
}

fn mutate_node(
    state: &AppState,
    raw_ref: &str,
    mutate: impl FnOnce(&mut MdocNode) -> ApiResult<()>,
) -> ApiResult<Json<NodeDetail>> {
    with_workspace_mutation(state, |cache, mutation_lock| {
        let (fnode, _, abs_path) =
            resolve_with_cache(cache, raw_ref).map_err(ApiError::from_resolve)?;
        let (snapshot, mut node) = snapshot_node(&abs_path)?;
        if node.fnode != fnode {
            bail!("fnode mismatch when updating node");
        }
        mutate(&mut node)?;
        save_and_index(cache, mutation_lock, &node, &snapshot)?;
        Ok(committed_node_detail(cache, &node))
    })
}

/// Once persistence and indexing succeed, response construction is infallible:
/// callers receive the committed node even if optional derived metadata is unavailable.
fn committed_node_detail(cache: &mut IndCache, node: &MdocNode) -> Json<NodeDetail> {
    Json(node_detail_from_committed_cache(cache, node))
}

fn node_detail_from_committed_cache(cache: &mut IndCache, node: &MdocNode) -> NodeDetail {
    let summary = cache.node_summary(&node.fnode).ok();
    let formalization = cache.formalization_status(&node.fnode).unwrap_or_default();
    let broken = summary.as_ref().map(|item| item.broken).unwrap_or(true);
    let depth = summary.map(|item| item.depth).unwrap_or(0);
    NodeDetail {
        fnode: node.fnode.clone(),
        title: node.title.clone(),
        rel_path: to_rel_path(cache.root(), &node.path),
        broken,
        depth,
        depens: node.depens.clone(),
        blocks: node.blocks.clone(),
        formalization,
    }
}

fn snapshot_node(
    abs_path: &std::path::Path,
) -> ApiResult<(crate::workspace::FileSnapshot, MdocNode)> {
    let snapshot = crate::workspace::FileSnapshot::capture(abs_path)?;
    let content = snapshot
        .content()
        .ok_or_else(|| anyhow::anyhow!("mdoc file disappeared: {}", abs_path.display()))?;
    let node = MdocNode::load_bytes(abs_path, content)?;
    Ok((snapshot, node))
}

fn save_and_index(
    cache: &mut IndCache,
    mutation_lock: &crate::workspace::WorkspaceMutationLock,
    node: &MdocNode,
    snapshot: &crate::workspace::FileSnapshot,
) -> ApiResult<()> {
    cache
        .replace_node(mutation_lock, node, snapshot)
        .map_err(ApiError::rejected)
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
    with_workspace_mutation(&state, |cache, mutation_lock| {
        let (fnode, _, _) = resolve_with_cache(cache, &fnode).map_err(ApiError::from_resolve)?;
        let mut graph =
            crate::depgraph::DepGraph::from_ref_under_lock(cache, mutation_lock, &fnode, None)?;
        let (added, _, _) = graph
            .add_direct_dependency_ref_under_lock(mutation_lock, &body.dep_fnode, None)
            .map_err(ApiError::rejected)?;
        if added.is_empty() {
            bail!("dependency already present or equals self");
        }
        let node = graph.root_node().clone();
        drop(graph);
        Ok(committed_node_detail(cache, &node))
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
    with_workspace_mutation(&state, |cache, mutation_lock| {
        if body.dep_fnodes.is_empty() {
            bail!("dep_fnodes must be non-empty");
        }
        let (fnode, _, _) = resolve_with_cache(cache, &fnode).map_err(ApiError::from_resolve)?;
        let mut graph =
            crate::depgraph::DepGraph::from_ref_under_lock(cache, mutation_lock, &fnode, None)?;
        let removed = graph
            .remove_direct_dependencies_under_lock(mutation_lock, body.dep_fnodes)
            .map_err(ApiError::rejected)?;
        if removed.is_empty() {
            bail!("none of the given fnodes are direct dependencies");
        }
        let node = graph.root_node().clone();
        drop(graph);
        Ok(committed_node_detail(cache, &node))
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
    with_workspace_mutation(&state, |cache, mutation_lock| {
        let title = body.title.trim();
        if title.is_empty() {
            bail!("title must be non-empty");
        }
        let file_path = body.file.as_deref().unwrap_or(".").trim();

        if let Some(parent) = &body.parent_fnode {
            // Resolve parent first so we can produce a clear error before write.
            let (parent_fnode, _, _) =
                resolve_with_cache(cache, parent).map_err(ApiError::from_resolve)?;
            let mut graph = crate::depgraph::DepGraph::from_ref_under_lock(
                cache,
                mutation_lock,
                &parent_fnode,
                None,
            )?;
            let new_node = graph
                .prepare_new_dependency_node(file_path, title, None)
                .map_err(ApiError::rejected)?;
            graph
                .create_and_add_dependency_under_lock(mutation_lock, new_node)
                .map_err(ApiError::rejected)?;
            // Return the parent (the user is editing the parent and just added a
            // dep — they want to see it appear in the children column).
            let node = graph.root_node().clone();
            drop(graph);
            Ok(committed_node_detail(cache, &node))
        } else {
            // Standalone new node, no parent.
            let graph = crate::depgraph::DepGraph::create_root_under_lock(
                cache,
                mutation_lock,
                file_path,
                title,
                None,
            )
            .map_err(ApiError::rejected)?;
            let node = graph.root_node().clone();
            drop(graph);
            Ok(committed_node_detail(cache, &node))
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
        let mut node = MdocNode::new_at_path(&path, "Original");
        node.upsert_source_block("latex", "original block".to_string())
            .unwrap();
        let fnode = node.fnode.clone();
        std::fs::write(&path, node.render().unwrap()).unwrap();

        let mut cache = IndCache::open(root.clone()).unwrap();
        cache.refresh_all().unwrap();
        let state = AppState::new(cache);
        (dir, state, path, fnode)
    }

    #[test]
    fn title_block_and_delete_conflicts_preserve_external_edit_and_index() {
        for operation in ["title", "block", "delete"] {
            let (_dir, state, path, fnode) = setup_state();
            let mut cache = state.cache.lock().unwrap();
            let mutation_lock = cache.acquire_mutation_lock().unwrap();
            let (snapshot, mut desired) = snapshot_node(&path).unwrap();
            match operation {
                "title" => desired.title = "Requested title".to_string(),
                "block" => desired.blocks[0].content = "requested block".to_string(),
                "delete" => desired.blocks.clear(),
                _ => unreachable!(),
            }

            // Deterministic failpoint: another writer commits after our parse but
            // before replacement.
            let mut external = MdocNode::load(&path).unwrap();
            external.title = format!("External edit during {operation}");
            std::fs::write(&path, external.render().unwrap()).unwrap();
            let external_bytes = std::fs::read(&path).unwrap();

            let error =
                save_and_index(&mut cache, &mutation_lock, &desired, &snapshot).unwrap_err();
            assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
            assert_eq!(std::fs::read(&path).unwrap(), external_bytes);

            let summary = cache.node_summary(&fnode).unwrap();
            assert_eq!(summary.title, "Original");
        }
    }

    #[test]
    fn load_node_generation_rejects_a_different_loaded_fnode() {
        let (_dir, state, path, fnode) = setup_state();
        let mut cache = state.cache.lock().unwrap();
        std::fs::write(&path, "@fnode: replacement-node\n@title: Replacement\n").unwrap();

        let error = load_node_generation(&mut cache, &fnode, &path).unwrap_err();
        assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
    }

    #[test]
    fn structured_recovery_errors_keep_http_classification() {
        let path = std::path::Path::new("node.mdoc");
        let conflict = crate::workspace::PersistenceRecoveryError::new(
            "index failed and rollback conflicted".to_string(),
            anyhow::anyhow!("index failed"),
            Some(crate::workspace::FileConflict::new(path).into()),
            None,
        );
        assert_eq!(
            ApiError::rejected(conflict.into()).into_response().status(),
            StatusCode::CONFLICT
        );

        let repair_failure = crate::workspace::PersistenceRecoveryError::new(
            "validation failed and index repair failed".to_string(),
            anyhow::anyhow!("validation failed"),
            None,
            Some(rusqlite::Error::InvalidQuery.into()),
        );
        assert_eq!(
            ApiError::rejected(repair_failure.into())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let conflict_and_repair_failure = crate::workspace::PersistenceRecoveryError::new(
            "rollback conflicted and index repair failed".to_string(),
            anyhow::anyhow!("index update failed"),
            Some(crate::workspace::FileConflict::new(path).into()),
            Some(rusqlite::Error::InvalidQuery.into()),
        );
        assert_eq!(
            ApiError::rejected(conflict_and_repair_failure.into())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let rollback_io = crate::workspace::PersistenceRecoveryError::new(
            "validation failed and rollback I/O failed".to_string(),
            anyhow::anyhow!("validation failed"),
            Some(std::io::Error::other("rollback failed").into()),
            None,
        );
        assert_eq!(
            ApiError::rejected(rollback_io.into())
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            ApiError::rejected(anyhow::anyhow!("invalid dependency"))
                .into_response()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn waiting_for_workspace_lock_does_not_hold_cache_mutex() {
        let (_dir, state, _path, _fnode) = setup_state();
        let root = state.cache.lock().unwrap().root().to_path_buf();
        let external_lock = crate::workspace::WorkspaceMutationLock::acquire(&root).unwrap();
        let worker_state = state.clone();
        let worker = std::thread::spawn(move || {
            with_workspace_mutation(&worker_state, |_cache, _mutation_lock| Ok(()))
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match state.mutation_lock.try_lock() {
                Err(std::sync::TryLockError::WouldBlock) => break,
                Err(std::sync::TryLockError::Poisoned(_)) => panic!("mutation mutex poisoned"),
                Ok(guard) => drop(guard),
            }
            assert!(std::time::Instant::now() < deadline, "writer did not start");
            std::thread::yield_now();
        }

        let cache_available = loop {
            match state.cache.try_lock() {
                Ok(guard) => {
                    drop(guard);
                    break true;
                }
                Err(std::sync::TryLockError::Poisoned(_)) => panic!("cache mutex poisoned"),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::yield_now();
        };

        drop(external_lock);
        worker.join().unwrap().unwrap();
        assert!(cache_available, "workspace lock wait held the cache mutex");
    }

    #[test]
    fn dependency_mutation_updates_the_existing_shared_cache() {
        let (_dir, state, path, fnode) = setup_state();
        let mut cache = state.cache.lock().unwrap();
        let root = cache.root().to_path_buf();
        let target_path = root.join("target.mdoc");
        let target = MdocNode::new_at_path(&target_path, "Target");
        let target_fnode = target.fnode.clone();
        std::fs::write(&target_path, target.render().unwrap()).unwrap();

        let mut graph = crate::depgraph::DepGraph::from_ref(&mut cache, &fnode, None).unwrap();
        graph
            .add_direct_dependency_ref(&target_fnode, None)
            .unwrap();
        let node = graph.root_node().clone();
        drop(graph);
        let detail = committed_node_detail(&mut cache, &node).0;

        assert_eq!(detail.depens, vec![target_fnode.clone()]);
        assert_eq!(MdocNode::load(&path).unwrap().depens, vec![target_fnode]);
        assert_eq!(cache.root(), root);
    }
}
