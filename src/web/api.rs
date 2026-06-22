use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::core::{DependencyItem, GraphCheckReport, GraphRootItem};
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
        return Err(ApiError(::anyhow::anyhow!($($t)*)))
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

/// Wrapper that turns `anyhow::Error` into a JSON 400/500 response. The `?`
/// operator converts `anyhow::Error` into this via the blanket `From` impl.
pub struct ApiError(pub anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let msg = self.0.to_string();
        // Cycle / "would create a cycle" / "already used" etc. are user errors.
        let status = if msg.contains("cycle")
            || msg.contains("already used")
            || msg.contains("already exists")
            || msg.contains("already present")
            || msg.contains("no mdoc matched")
            || msg.contains("ambiguous")
            || msg.contains("must be")
            || msg.contains("cannot be empty")
            || msg.contains("unsupported srctype")
            || msg.contains("no '@src:")
            || msg.contains("fnode mismatch")
            || msg.contains("none of the given")
            || msg.contains("dep_fnodes must be")
        {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ── Cache helpers ─────────────────────────────────────────────────────────────

/// Lock the cache, run a closure, return the result.
fn with_cache<R>(
    state: &AppState,
    f: impl FnOnce(&mut IndCache) -> anyhow::Result<R>,
) -> ApiResult<R> {
    let mut cache = state.cache.lock().expect("cache mutex poisoned");
    Ok(f(&mut cache)?)
}

/// Resolve a ref (fnode, prefix, or path) and return (fnode, title, abs_path).
fn resolve(state: &AppState, raw: &str) -> ApiResult<(String, String, std::path::PathBuf)> {
    with_cache(state, |c| {
        c.discover_workspace_changes()?;
        c.resolve_ref(raw, Some(&state.mdcroot))
    })
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
        // Reuse the valid-node query used by global_root_items so broken /
        // duplicate / invalid nodes are excluded consistently.
        let depths = c.all_topo_depths()?;
        // Use search-like flat rows from the index, filtering out issues.
        // valid_node_rows is private to queries; reuse search("") which returns
        // all rows — but that includes invalid ones. Instead, walk mdocs and
        // filter by has_issues for each (cheap, single SQL per node).
        let all_rows = c.search("")?;
        let mut nodes: Vec<NodeInfo> = Vec::with_capacity(all_rows.len());
        for (fnode, title, rel_path) in all_rows {
            let broken = c.has_issues(&fnode)?;
            if broken {
                continue;
            }
            let depth = depths.get(&fnode).copied().unwrap_or(0);
            nodes.push(NodeInfo {
                fnode,
                title,
                rel_path,
                broken: false,
                depth,
            });
        }
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
    let rows = with_cache(&state, |c| {
        c.discover_workspace_changes()?;
        c.search(&q.q)
    })?;
    let mut out = Vec::with_capacity(rows.len().min(q.n));
    for (fnode, title, rel_path) in rows.into_iter().take(q.n) {
        out.push(node_info(&state, &fnode, &title, &rel_path)?);
    }
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
    validate_srctype(&srctype)?;
    let (fnode, _, abs_path) = resolve(&state, &fnode)?;
    let mut node = MdocNode::load(&state.mdcroot, &abs_path)?;
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
    node.save()?;
    upsert_and_discover(&state, &abs_path)?;
    Ok(Json(node_detail_from(&state, &abs_path)?))
}

/// Delete a single srctype block from the focused node.
pub async fn node_delete_block(
    State(state): State<AppState>,
    Path((fnode, srctype)): Path<(String, String)>,
) -> ApiResult<Json<NodeDetail>> {
    validate_srctype(&srctype)?;
    let (fnode, _, abs_path) = resolve(&state, &fnode)?;
    let mut node = MdocNode::load(&state.mdcroot, &abs_path)?;
    if node.fnode != fnode {
        bail!("fnode mismatch when deleting block");
    }
    let before = node.blocks.len();
    node.blocks.retain(|b| b.srctype != srctype);
    if node.blocks.len() == before {
        bail!("no '@src: {srctype}' block on this node");
    }
    node.save()?;
    upsert_and_discover(&state, &abs_path)?;
    Ok(Json(node_detail_from(&state, &abs_path)?))
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
    let (fnode, _, abs_path) = resolve(&state, &fnode)?;
    let mut node = MdocNode::load(&state.mdcroot, &abs_path)?;
    if node.fnode != fnode {
        bail!("fnode mismatch when updating title");
    }
    node.title = title.to_string();
    node.save()?;
    upsert_and_discover(&state, &abs_path)?;
    Ok(Json(node_detail_from(&state, &abs_path)?))
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
    match srctype {
        "text" | "latex" | "python" | "lean" | "rocq" => Ok(()),
        _ => bail!("unsupported srctype '{srctype}'"),
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

/// Reload the focused node after a write so callers receive the canonical
/// form.
fn node_detail_from(state: &AppState, abs_path: &std::path::Path) -> ApiResult<NodeDetail> {
    let node = MdocNode::load(&state.mdcroot, abs_path)?;
    let rel_path = to_rel_path(&state.mdcroot, abs_path);
    let info = node_info(state, &node.fnode, &node.title, &rel_path)?;
    Ok(NodeDetail {
        fnode: info.fnode,
        title: info.title,
        rel_path: info.rel_path,
        broken: info.broken,
        depth: info.depth,
        depens: node.depens.clone(),
        blocks: node.blocks,
    })
}

/// Upsert the modified file's index entry and run incremental discovery so
/// derived data (topo depth, weak components) is consistent for the next
/// request.
fn upsert_and_discover(state: &AppState, abs_path: &std::path::Path) -> ApiResult<()> {
    with_cache(state, |c| {
        c.upsert_path(abs_path)?;
        c.discover_workspace_changes()?;
        Ok(())
    })
}

// Unused for now but re-exported so future write handlers share the same DTO.
#[allow(dead_code)]
fn _dep_item_to_info(state: &AppState, item: &DependencyItem) -> ApiResult<NodeInfo> {
    node_info(state, &item.fnode, &item.title, &item.rel_path)
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
    let (fnode, _, abs_path) = resolve(&state, &fnode)?;
    let mut graph = crate::depgraph::DepGraph::new(state.mdcroot.clone(), &fnode)?;
    let (added, _, _) = graph.add_direct_dependencies(vec![body.dep_fnode.clone()])?;
    if added.is_empty() {
        bail!("dependency already present or equals self");
    }
    // Reload and refresh derived data via the cache.
    let _ = state.mdcroot.join(""); // touch
    drop(graph);
    with_cache(&state, |c| {
        c.upsert_path(&abs_path)?;
        c.discover_workspace_changes()?;
        Ok(())
    })?;
    Ok(Json(node_detail_from(&state, &abs_path)?))
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
    if body.dep_fnodes.is_empty() {
        bail!("dep_fnodes must be non-empty");
    }
    let (fnode, _, abs_path) = resolve(&state, &fnode)?;
    let mut graph = crate::depgraph::DepGraph::new(state.mdcroot.clone(), &fnode)?;
    let removed = graph.remove_direct_dependencies(body.dep_fnodes)?;
    if removed.is_empty() {
        bail!("none of the given fnodes are direct dependencies");
    }
    drop(graph);
    with_cache(&state, |c| {
        c.upsert_path(&abs_path)?;
        c.discover_workspace_changes()?;
        Ok(())
    })?;
    Ok(Json(node_detail_from(&state, &abs_path)?))
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
    let title = body.title.trim();
    if title.is_empty() {
        bail!("title must be non-empty");
    }
    let file_path = body.file.as_deref().unwrap_or(".").trim();

    if let Some(parent) = &body.parent_fnode {
        // Resolve parent first so we can produce a clear error before write.
        let (parent_fnode, _, parent_path) = resolve(&state, parent)?;
        let mut graph = crate::depgraph::DepGraph::new(state.mdcroot.clone(), &parent_fnode)?;
        let mut new_node =
            crate::mdocnode::MdocNode::new_at_path(&state.mdcroot, &state.mdcroot.join("."), title);
        let target_path = if file_path == "." {
            state.mdcroot.join(format!("{}.mdoc", &new_node.fnode))
        } else {
            let p = std::path::Path::new(file_path);
            let joined = state.mdcroot.join(p);
            let stem = joined
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid file path"))?
                .to_string_lossy()
                .into_owned();
            joined.with_file_name(format!("{stem}.mdoc"))
        };
        new_node.path = target_path;
        graph.create_and_add_dependency(new_node)?;
        drop(graph);
        // Refresh the shared cache so the next read sees the new edge.
        with_cache(&state, |c| {
            c.upsert_path(&parent_path)?;
            c.discover_workspace_changes()?;
            Ok(())
        })?;
        // Return the parent (the user is editing the parent and just added a
        // dep — they want to see it appear in the children column).
        Ok(Json(node_detail_from(&state, &parent_path)?))
    } else {
        // Standalone new node, no parent.
        let (_graph, rel) = crate::depgraph::DepGraph::create_root(
            state.mdcroot.clone(),
            file_path,
            title,
            None,
            None,
        )?;
        let abs = state.mdcroot.join(&rel);
        with_cache(&state, |c| {
            c.discover_workspace_changes()?;
            Ok(())
        })?;
        Ok(Json(node_detail_from(&state, &abs)?))
    }
}

// Keep Arc referenced for future handlers; avoids unused-import noise in the
// minimal skeleton.
#[allow(dead_code)]
type _ArcState = Arc<AppState>;
