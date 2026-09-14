//! Local web service: every client uses the same versioned database transactions.
use crate::lean::editor::{Message as EditorMessage, Observer};
use crate::store::{Block, Database, LeanProject, Node, Snapshot, BLOCK_TYPES};
use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
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
    token: String,
    stop: tokio::sync::Notify,
}
impl Service {
    pub async fn open(db: Database) -> Result<Arc<Self>> {
        let lean = crate::lean::LeanService::new(db.cache_path()?)?;
        let snapshot = db.load().await?;
        Ok(Arc::new(Self {
            db,
            snapshot: Mutex::new(snapshot),
            lean,
            editors: Mutex::new(HashMap::new()),
            editor_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            token: uuid::Uuid::new_v4().to_string(),
            stop: tokio::sync::Notify::new(),
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

pub struct ApiError(pub(crate) StatusCode, pub(crate) String);
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
    value["formalization"] = json!({"lean":lean.formal_status(node, &snapshot.lean_keys[&node.fnode]),
        "rocq":if node.source("rocq").is_some(){"unverified"}else{"no_code"}});
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
        .route("/service/stop", post(stop_service))
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
        .route("/lean/session/:id", get(editor_info).delete(close_editor))
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
        .with_state(service.clone())
        .fallback(get(|uri: axum::http::Uri| async move {
            crate::web::assets::serve_asset(uri)
        }))
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(service, local_origin))
}

async fn stop_service(State(service): State<Arc<Service>>, headers: HeaderMap) -> Response {
    if headers.get("x-mdc-service").and_then(|h| h.to_str().ok()) != Some(&service.token) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"use mdc stop DATABASE/BRANCH"})),
        )
            .into_response();
    }
    service.stop.notify_one();
    Json(json!({"stopping":true})).into_response()
}

async fn local_origin(
    State(service): State<Arc<Service>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    // A port can be reused after a restart. Never send a CLI write to its new owner.
    if request
        .headers()
        .get("x-mdc-service")
        .is_some_and(|token| token != service.token.as_str())
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"service changed; retry the command"})),
        )
            .into_response();
    }
    if !allowed_origin(request.headers(), None) {
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

pub(crate) fn allowed_origin(headers: &HeaderMap, public_origin: Option<&str>) -> bool {
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let local = host
        .parse::<axum::http::uri::Authority>()
        .ok()
        .is_some_and(|h| {
            h.host() == "localhost"
                || h.host()
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
    let local_origin = format!("http://{host}");
    let expected = public_origin
        .filter(|origin| {
            origin
                .split_once("://")
                .is_some_and(|(_, authority)| authority == host)
        })
        .or_else(|| local.then_some(local_origin.as_str()));
    let Some(expected) = expected else {
        return false;
    };
    headers.get("origin").is_none_or(|origin| {
        origin
            .to_str()
            .ok()
            .and_then(|v| reqwest::Url::parse(v).ok())
            .is_some_and(|u| u.origin().ascii_serialization() == expected)
    })
}

pub(crate) fn acquire_lease(root: &std::path::Path) -> Result<std::fs::File> {
    use std::os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    std::fs::create_dir_all(root)?;
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(root.join("service.lock"))?;
    if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("this service cache is already owned by another service");
    }
    lease.set_len(0)?;
    lease.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(lease)
}

pub(crate) async fn serve(db: Database, port: u16) -> Result<()> {
    // Bind the requested port itself: probing then rebinding races other processes.
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| format!("cannot bind port {port} (it may already be in use)"))?;
    let service = Service::open(db).await?;
    let info = RunningService {
        port: listener.local_addr()?.port(),
        pid: std::process::id(),
        token: service.token.clone(),
        public_origin: None,
    };
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    std::fs::write(
        service.db.cache_path()?.join("service.lock"),
        serde_json::to_vec(&info)?,
    )?;
    // The parent returns only after the branch, lease and listener are ready.
    use std::io::Write;
    writeln!(std::io::stdout(), "{}", serde_json::to_string(&info)?)?;
    let stopping = service.clone();
    axum::serve(listener, router(service.clone()))
        .with_graceful_shutdown(async move {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{},_=stopping.stop.notified()=>{}}
            stopping.editor_slots.close();
            let closing = std::mem::take(&mut *stopping.editors.lock().await);
            for session in closing.values() {
                let _ = session._cancel.send(());
            }
            for session in closing.values() {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    session._cancel.closed(),
                )
                .await;
            }
        })
        .await?;
    service.lean.shutdown().await;
    Ok(())
}

/// A leftover address alone is not evidence of a live service. The existing
/// branch lease is held for the service lifetime and released by the OS on crash.
pub(crate) fn running_service(root: &std::path::Path) -> Result<Option<RunningService>> {
    use std::{
        io::{ErrorKind, Read},
        os::fd::AsRawFd,
    };
    let mut file = match std::fs::File::open(root.join("service.lock")) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        return Ok(None);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() != ErrorKind::WouldBlock {
        return Err(error.into());
    }
    let mut address = String::new();
    file.read_to_string(&mut address)?;
    if address.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&address).context(
        "invalid service record; restart the service after upgrading mdc",
    )?))
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RunningService {
    pub port: u16,
    pub pid: u32,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_origin: Option<String>,
}
impl RunningService {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub fn browser_url(&self) -> String {
        self.public_origin.clone().unwrap_or_else(|| self.url())
    }
}

pub(crate) async fn delete_branch(project: &str) -> Result<Value> {
    let (database, branch) = crate::config::project_parts(project)?;
    let db = Database::from_env(database.into(), branch.into())?;
    let root = db.cache_path()?;
    // Use the service's exclusive lease, including during startup before a port is recorded.
    let _lease = crate::lean::LeanService::new(root.clone()).map_err(|error| {
        anyhow::anyhow!("cannot lock {project}: {error}; run mdc stop {project} before deleting")
    })?;
    db.version().await?;
    // Clean disposable files first so a filesystem error leaves branch data intact.
    // Keep the lock inode: unlinking it would let a concurrent start bypass our lease.
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if entry.file_name() == "service.lock" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    db.delete_branch().await?;
    Ok(json!({"project":project,"deleted":true}))
}

pub(crate) async fn start(project: Option<&str>, port: Option<u16>) -> Result<Option<Value>> {
    use std::{
        io::{Read, Write},
        os::{fd::AsFd, unix::net::UnixStream},
        time::Duration,
    };
    // Node.js also uses anonymous sockets for ordinary child stdin. Only the
    // marker queued before spawn identifies our child; never wait for user stdin.
    // Re-exec keeps Tokio initialization out of the post-fork child.
    if let Ok(fd) = std::io::stdin().as_fd().try_clone_to_owned() {
        let mut channel = UnixStream::from(fd);
        if channel
            .local_addr()
            .is_ok_and(|address| address.is_unnamed())
        {
            channel.set_nonblocking(true)?;
            let mut marker = [0; 9];
            if channel.read_exact(&mut marker).is_ok() && &marker == b"mdc-start" {
                let mut port_bytes = [0; 2];
                channel.read_exact(&mut port_bytes)?;
                let internal_port = u16::from_be_bytes(port_bytes);
                drop(channel);
                let result = async {
                    if let Some(project) = project {
                        let (database, branch) = crate::config::project_parts(project)?;
                        serve(
                            Database::from_env(database.into(), branch.into())?,
                            internal_port,
                        )
                        .await
                    } else {
                        crate::gateway::serve(internal_port).await
                    }
                }
                .await;
                if let Err(error) = &result {
                    let _ = writeln!(
                        std::io::stdout(),
                        "{}",
                        json!({"error":format!("{error:#}")})
                    );
                }
                return result.map(|_| None);
            }
        }
    }
    let settings = crate::config::Settings::load()?;
    if let Some(project) = project {
        if running_service(&settings.project_cache(project)?)?.is_some() {
            bail!("{project} is already running");
        }
        let (database, branch) = crate::config::project_parts(project)?;
        Database::from_env(database.into(), branch.into())?
            .version()
            .await?;
    }
    let gateway_root = settings.gateway_cache()?;
    let gateway = if let Some(info) = running_service(&gateway_root)? {
        if port.is_some_and(|port| port != info.port) {
            bail!(
                "entry server is already using port {}; run mdc stop before changing its port",
                info.port
            );
        }
        info
    } else {
        let requested = port.unwrap_or(settings.gateway_port()?);
        match spawn_background(None, requested, &gateway_root).await {
            Ok(info) => info,
            Err(error) => {
                // Concurrent branch starts may race to launch the single entry server.
                let current = tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if let Some(info) = running_service(&gateway_root)? {
                            if info.port == requested {
                                return Ok::<_, anyhow::Error>(info);
                            }
                            bail!("entry server started on a different port");
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await;
                match current {
                    Ok(Ok(info)) => info,
                    _ => return Err(error),
                }
            }
        }
    };
    let Some(project) = project else {
        return Ok(Some(
            json!({"port":gateway.port,"pid":gateway.pid,"url":gateway.browser_url(),"log":gateway_root.join("service.log")}),
        ));
    };
    let root = settings.project_cache(project)?;
    let info = spawn_background(Some(project), 0, &root).await?;
    Ok(Some(
        json!({"project":project,"port":gateway.port,"pid":info.pid,
        "url":format!("{}/p/{project}/", gateway.browser_url()),"log":root.join("service.log")}),
    ))
}

async fn spawn_background(
    project: Option<&str>,
    port: u16,
    root: &std::path::Path,
) -> Result<RunningService> {
    use std::{
        io::Write,
        os::unix::{fs::OpenOptionsExt, net::UnixStream},
        process::Stdio,
        time::Duration,
    };
    use tokio::io::{AsyncBufReadExt, BufReader};
    std::fs::create_dir_all(&root)?;
    let log_path = root.join("service.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    let (mut bootstrap, child_bootstrap) = UnixStream::pair()?;
    bootstrap.write_all(b"mdc-start")?;
    bootstrap.write_all(&port.to_be_bytes())?;
    let child_bootstrap: std::os::fd::OwnedFd = child_bootstrap.into();
    command
        .arg("start")
        .current_dir(&root)
        .stdin(Stdio::from(child_bootstrap))
        .stdout(Stdio::piped())
        .stderr(log);
    if let Some(project) = project {
        command.arg(project);
    }
    if let Some(path) = std::env::var_os("MDC_CONFIG") {
        command.env("MDC_CONFIG", std::fs::canonicalize(path)?);
    }
    // setsid detaches the child from the launching terminal; no shell or wrapper.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("start mdc service process")?;
    let ready: Result<RunningService> = async {
        let mut output = BufReader::new(child.stdout.take().context("service startup pipe")?);
        let mut line = String::new();
        tokio::select! {
            read = tokio::time::timeout(Duration::from_secs(180), output.read_line(&mut line)) => {
                read.context("service startup timed out")??;
            }
            _ = tokio::signal::ctrl_c() => { bail!("service startup interrupted"); }
        }
        if line.is_empty() {
            bail!("service exited before startup; see {}", log_path.display());
        }
        let ready: Value = serde_json::from_str(&line)?;
        if let Some(error) = ready["error"].as_str() {
            bail!("{error}");
        }
        Ok(serde_json::from_value(ready)?)
    }
    .await;
    drop(bootstrap);
    let info = match ready {
        Ok(info) => info,
        Err(error) => {
            let _ = child.kill().await;
            return Err(error);
        }
    };
    Ok(info)
}

pub(crate) async fn stop(project: Option<&str>) -> Result<Value> {
    use std::time::Duration;
    let settings = crate::config::Settings::load()?;
    let root = match project {
        Some(project) => settings.project_cache(project)?,
        None => settings.gateway_cache()?,
    };
    let name = project.unwrap_or("entry server");
    let info = running_service(&root)?.with_context(|| format!("{name} is not running"))?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()?
        .post(format!("{}/api/service/stop", info.url()))
        .header("x-mdc-service", &info.token)
        .send()
        .await?
        .error_for_status()?;
    response.bytes().await?;
    // Wait for this lease to end, not for a PID that the OS could recycle.
    tokio::time::timeout(Duration::from_secs(30), async {
        while running_service(&root)?.is_some_and(|current| current.token == info.token) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("service is still shutting down; inspect service.log")??;
    Ok(match project {
        Some(project) => json!({"project":project,"stopped":true}),
        None => json!({"stopped":true}),
    })
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
    Ok(Json(json!({"nodes":snapshot.nodes.values().map(|n| {
            let mut value = summary(&snapshot, n);
            value["lean"] = json!(s.lean.formal_status(n, &snapshot.lean_keys[&n.fnode]));
            value
        }).collect::<Vec<_>>(),"edges":edges})))
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
    let _ = s
        .lean
        .cached_or_load(
            n,
            &snapshot.lean_keys[&n.fnode],
            &snapshot.project_key,
            false,
        )
        .await;
    Ok(Json(json!({"node":detail(&snapshot,n,&s.lean),
        "referrers":snapshot.referrers.get(&n.fnode).into_iter().flatten().map(|id|summary(&snapshot,&snapshot.nodes[id])).collect::<Vec<_>>(),
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
    let mut node = Node::new(body.title)?;
    node.module = format!(
        "{}.N_{}",
        snapshot.project.module_root(),
        node.fnode.replace('-', "")
    );
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
    if !BLOCK_TYPES.contains(&language.as_str()) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "supported types: text, lean, rocq, latex".into(),
        ));
    }
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
    let removed: BTreeSet<_> = body
        .dep_fnodes
        .iter()
        .map(|reference| snapshot.resolve(reference).map(|node| node.fnode.clone()))
        .collect::<Result<_>>()?;
    node.depens.retain(|id| !removed.contains(id));
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
    let matches: Vec<_> = snapshot
        .nodes
        .values()
        .filter(|n| n.title.to_lowercase().contains(&q) || n.fnode.contains(&q))
        .collect();
    let source = matches.iter().filter(|n| n.fnode == node.fnode).count();
    let dependencies: HashSet<_> = node.depens.iter().collect();
    let existing = matches
        .iter()
        .filter(|n| dependencies.contains(&n.fnode))
        .count();
    let available = matches.len() - source - existing;
    let nodes: Vec<_> = matches
        .into_iter()
        .filter(|n| n.fnode != node.fnode && !dependencies.contains(&n.fnode))
        .take(query.n.unwrap_or(50).min(200))
        .map(|n| summary(&snapshot, n))
        .collect();
    let empty = if !nodes.is_empty() {
        Value::Null
    } else if available > 0 {
        json!({"kind":"result_limit","available":available})
    } else if source + existing > 0 {
        json!({"kind":"excluded","source":source,"existing_dependencies":existing,"invalid_or_duplicate":0})
    } else {
        json!({"kind":"no_match"})
    };
    Ok(Json(json!({"nodes":nodes,"empty":empty})))
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum TraversalMode {
    Show,
    Refs,
    Leaf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Traversal {
    mode: TraversalMode,
    depth: i32,
}
async fn traverse(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Query(query): Query<Traversal>,
) -> ApiResult<Json<Value>> {
    let snapshot = s.read().await?;
    if query.depth < -1 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "depth must be -1 (unlimited) or nonnegative".into(),
        ));
    }
    let root = snapshot.resolve(&id)?;
    let mut seen = HashSet::from([root.fnode.clone()]);
    let mut queue = std::collections::VecDeque::from([(root.fnode.clone(), 0i32)]);
    let mut nodes = vec![];
    while let Some((id, depth)) = queue.pop_front() {
        let node = &snapshot.nodes[&id];
        if depth > 0 && (query.mode != TraversalMode::Leaf || node.depens.is_empty()) {
            let mut item = summary(&snapshot, node);
            item["depth"] = json!(depth);
            nodes.push(item);
        }
        if query.depth >= 0 && depth >= query.depth {
            continue;
        }
        let next = if query.mode == TraversalMode::Refs {
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
    project.validate_modules(snapshot.nodes.values().map(AsRef::as_ref))?;
    let version = s.db.put_project(&project, &snapshot.version).await?;
    snapshot.version = version;
    snapshot.project = project.into();
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
    pub project: LeanProject,
}
async fn import(State(s): State<Arc<Service>>, Json(body): Json<Import>) -> ApiResult<Json<Value>> {
    let mut snapshot = s.read().await?;
    if !snapshot.nodes.is_empty() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "graph import requires an empty branch".into(),
        ));
    }
    // Project configuration is validated before any data is imported.
    body.project.validate()?;
    body.project.validate_modules(body.nodes.iter())?;
    snapshot.validate_changes(&body.nodes)?;
    let version =
        s.db.put_bundle(
            &body.nodes,
            Some(&body.project),
            &snapshot.version,
            "Import graph",
        )
        .await?;
    snapshot.project = body.project.into();
    snapshot.nodes = body
        .nodes
        .into_iter()
        .map(|n| (n.fnode.clone(), Arc::new(n)))
        .collect();
    snapshot.version = version;
    snapshot.recompute();
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
    input: Option<crate::lean::Input>,
    document: Value,
    _cancel: tokio::sync::watch::Sender<()>,
    _slot: tokio::sync::OwnedSemaphorePermit,
}
fn editor_document(input: &crate::lean::Input) -> Result<Value> {
    let node = &input.chain.last().context("no Lean target")?.0;
    Ok(json!({
        "fnode": node.fnode,
        "filename": format!("/project/{}", crate::store::module_file(&node.module, "lean")?.display()),
        "source": node.source("lean").context("node has no Lean block")?,
        "revision": node.revision(),
        // Source edits reuse the worker; changed imports/dependencies must reload its environment.
        "environment_key": input.environment_key()?,
    }))
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
    let document = editor_document(&input)?;
    let slot = s.editor_slots.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "all Lean editor slots are in use; close an idle editor".into(),
        )
    })?;
    let id = uuid::Uuid::new_v4().to_string();
    s.editors.lock().await.insert(
        id.clone(),
        EditorSession {
            input: Some(input),
            document: document.clone(),
            _cancel: tokio::sync::watch::channel(()).0,
            _slot: slot,
        },
    );
    // A browser can disappear before connecting. Expire its reservation even if
    // no other request arrives; environment preparation starts only on connection.
    let service = Arc::downgrade(&s);
    let pending = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        if let Some(s) = service.upgrade() {
            let mut editors = s.editors.lock().await;
            if editors.get(&pending).is_some_and(|e| e.input.is_some()) {
                editors.remove(&pending);
            }
        }
    });
    let mut response = document;
    response["id"] = json!(id);
    Ok(Json(response))
}
async fn close_editor(State(s): State<Arc<Service>>, Path(id): Path<String>) -> StatusCode {
    // Release the slot before waiting for native shutdown or filesystem cleanup.
    let session = s.editors.lock().await.remove(&id);
    if let Some(session) = session {
        let cancel = session._cancel.clone();
        drop(session);
        let _ = cancel.send(());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), cancel.closed()).await;
    }
    StatusCode::NO_CONTENT
}

async fn editor_info(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let editors = s.editors.lock().await;
    let e = editors
        .get(&id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "editor session expired".into()))?;
    Ok(Json(e.document.clone()))
}
async fn editor_socket(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> ApiResult<Response> {
    let (input, mut cancelled) = {
        let mut editors = s.editors.lock().await;
        let session = editors
            .get_mut(&id)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "editor session expired".into()))?;
        let input = session.input.take().ok_or_else(|| {
            ApiError(
                StatusCode::CONFLICT,
                "editor session already connected".into(),
            )
        })?;
        (input, session._cancel.subscribe())
    };
    let failed_service = s.clone();
    let failed_id = id.clone();
    Ok(ws
        .max_message_size(32 * 1024 * 1024)
        .on_failed_upgrade(move |_| {
            tokio::spawn(async move { failed_service.editors.lock().await.remove(&failed_id); });
        })
        .on_upgrade(move |socket| async move {
            let prepared = tokio::select! {
                biased;
                _ = cancelled.changed() => Ok(None),
                result = async {
                    Ok::<_, anyhow::Error>(Some(tokio::time::timeout(std::time::Duration::from_secs(30), s.lean.editor_project(&input)).await.context("editor environment preparation timed out")??))
                } => result,
            };
            let result = match prepared {
                Ok(Some(directory)) => {
                    let result = bridge_editor(socket, directory.path(), &input, &s, &mut cancelled).await;
                    tokio::task::spawn_blocking(move || drop(directory));
                    result
                },
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            };
            s.editors.lock().await.remove(&id);
            if let Err(error) = result {
                eprintln!("Lean editor: {error:#}");
            }
        }))
}
/// Validate with serde's iterative skip parser and rewrite individual string
/// values, without constructing, serializing or dropping a deeply nested tree.
fn rewrite_uris(text: &str, from: &str, to: &str) -> Result<String> {
    let _: serde::de::IgnoredAny = serde_json::from_str(text)?;
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let (mut index, mut copied) = (0, 0);
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while bytes[index] != b'"' {
            if bytes[index] == b'\\' {
                index += 1;
            }
            index += 1;
        }
        index += 1;
        // Object keys are not URI values. Escaped quotes inside source text are
        // consumed with their enclosing string, so source literals stay intact.
        if text[index..].trim_start().starts_with(':') {
            continue;
        }
        let value: String = serde_json::from_str(&text[start..index])?;
        if let Some(suffix) = value
            .strip_prefix(from)
            .filter(|s| s.is_empty() || s.starts_with('/'))
        {
            output.push_str(&text[copied..start]);
            output.push_str(&serde_json::to_string(&format!("{to}{suffix}"))?);
            copied = index;
        }
    }
    output.push_str(&text[copied..]);
    Ok(output)
}

#[derive(Deserialize)]
struct DocumentChange {
    params: DocumentChangeParams,
}
#[derive(Deserialize)]
struct DocumentChangeParams {
    #[serde(rename = "textDocument")]
    document: DocumentUri,
}
#[derive(Deserialize)]
struct DocumentUri {
    uri: String,
}
async fn bridge_editor(
    socket: axum::extract::ws::WebSocket,
    root: &std::path::Path,
    input: &crate::lean::Input,
    service: &Service,
    cancelled: &mut tokio::sync::watch::Receiver<()>,
) -> Result<()> {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};
    let actual = crate::lean::file_uri(root)?;
    let mut sources: crate::lean::SourceState = input
        .chain
        .iter()
        .map(|(node, _)| (node.fnode.clone(), node.clone()))
        .collect();
    let document = editor_document(input)?;
    let mut allowed = HashSet::from([crate::lean::file_uri(std::path::Path::new(
        document["filename"].as_str().unwrap(),
    ))?]);
    let mut observer = Observer::default();
    observer.prepare(allowed.iter().next().unwrap().clone(), input.clone());
    let observer = std::sync::Mutex::new(observer);
    let mut server = crate::lean::Server::start(root)?;
    let (mut send, mut receive) = socket.split();
    let result = {
        let native_input = server.sender();
        let (replies, mut outgoing) = tokio::sync::mpsc::channel(32);
        // Neither direction may wait for the other to drain its bounded queue.
        // Native output also keeps flowing while a custom request prepares files.
        let from_client = async {
            while let Some(message) = receive.next().await {
                match message? {
                    Message::Text(text) => {
                        let message: EditorMessage = serde_json::from_str(&text)?;
                        let method = message.method.as_deref().unwrap_or("");
                        if matches!(
                            method,
                            "mdc/selectNode" | "mdc/validationState" | "mdc/certify"
                        ) {
                            let value: Value = serde_json::from_str(&text)?;
                            let selected: Result<Value> = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                                let id = value["params"]["fnode"].as_str().context("node required")?;
                                let revision = value["params"]["revision"].as_str().context("revision required")?;
                                let next = {
                                    let snapshot = service.read().await?;
                                    if snapshot.resolve(id)?.revision() != revision { bail!("node changed; reload and retry"); }
                                    crate::lean::Input::capture(&snapshot, id)?
                                };
                                if next.project_key != input.project_key { bail!("Lean project changed; reload environment"); }
                                let document = editor_document(&next)?;
                                let uri = crate::lean::file_uri(std::path::Path::new(document["filename"].as_str().unwrap()))?;
                                if method == "mdc/selectNode" {
                                    crate::lean::refresh_editor_sources(root, &next, &mut sources).await?;
                                    allowed.insert(uri.clone());
                                    observer.lock().unwrap().prepare(uri, next);
                                    Ok(document)
                                } else if method == "mdc/validationState" {
                                    let version = observer.lock().unwrap().document(&uri, &next)?.version;
                                    Ok(json!({"uri":uri,"version":version,"module":{"name":next.chain.last().unwrap().0.module,"uri":uri}}))
                                } else {
                                    let version = value["params"]["version"].as_u64().context("Lean version required")?;
                                    let (diagnostics, imports) = observer.lock().unwrap().evidence(&uri, &next, version)?;
                                    // Serialize publication with graph mutations. An older
                                    // editor can never certify newly saved dependencies.
                                    let snapshot = service.read().await?;
                                    let target = next.chain.last().unwrap();
                                    if snapshot.lean_keys.get(id) != Some(&target.1) { bail!("Lean inputs changed during checking"); }
                                    let result = service.lean.record_editor_check(root, &next, diagnostics, imports).await?;
                                    Ok(json!(result))
                                }
                            }).await.context("Lean editor request timed out")?;
                            let reply = match selected {
                                Ok(document) => {
                                    json!({"jsonrpc":"2.0","id":value["id"],"result":document})
                                }
                                Err(error) => {
                                    json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32000,"message":error.to_string()}})
                                }
                            };
                            replies
                                .send(Message::Text(serde_json::to_string(&reply)?))
                                .await?;
                            continue;
                        }
                        if matches!(
                            method,
                            "textDocument/didOpen"
                                | "textDocument/didChange"
                                | "textDocument/didSave"
                        ) {
                            let change: DocumentChange = serde_json::from_str(&text)?;
                            if !allowed.contains(&change.params.document.uri) {
                                bail!("editor may only change its selected draft modules");
                            }
                        }
                        observer.lock().unwrap().client(&text, &message)?;
                        let text = rewrite_uris(&text, "file:///project", &actual)?;
                        native_input
                            .send(text)
                            .await
                            .context("Lean server closed its input")?;
                    }
                    Message::Close(_) => break,
                    Message::Ping(data) => replies.send(Message::Pong(data)).await?,
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        let to_client = async {
            loop {
                let message = tokio::select! {
                    message = outgoing.recv() => match message {
                        Some(message) => message,
                        None => return Ok::<_, anyhow::Error>(()),
                    },
                    message = server.receive() => {
                        let text = rewrite_uris(&message?, &actual, "file:///project")?;
                        let text = observer.lock().unwrap().server(&text)?.unwrap_or(text);
                        Message::Text(text)
                    }
                };
                tokio::time::timeout(std::time::Duration::from_secs(30), send.send(message))
                    .await
                    .context("Lean editor client stopped reading messages")??;
            }
        };
        tokio::select! {
            _ = cancelled.changed() => Ok(()),
            result = from_client => result,
            result = to_client => result,
        }
    };
    server.shutdown().await;
    result
}

#[cfg(test)]
mod editor_protocol_tests {
    use super::*;

    #[test]
    fn deep_rpc_payloads_and_escaped_source_survive_uri_translation() {
        let deep = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{}"file:///project/Lib/A.lean"{}}}"#,
            "[".repeat(2048),
            "]".repeat(2048)
        );
        assert!(serde_json::from_str::<Value>(&deep).is_err());
        let translated = rewrite_uris(&deep, "file:///project", "file:///tmp/draft").unwrap();
        assert_eq!(
            translated,
            deep.replace("file:///project/", "file:///tmp/draft/")
        );
        let _: EditorMessage = serde_json::from_str(&translated).unwrap();
        let source = r#"def path := \"file:///project/Lib/A.lean\""#;
        let message = json!({"params":{"uri":"file:///project/Lib/A.lean","text":source}, "prefix":"file:///project-other/A.lean", "file:///project/key":"untouched"});
        let translated: Value = serde_json::from_str(
            &rewrite_uris(&message.to_string(), "file:///project", "file:///tmp/draft").unwrap(),
        )
        .unwrap();
        assert_eq!(translated["params"]["text"], source);
        assert_eq!(translated["params"]["uri"], "file:///tmp/draft/Lib/A.lean");
        assert_eq!(translated["prefix"], message["prefix"]);
        assert_eq!(translated["file:///project/key"], "untouched");
        assert!(rewrite_uris(r#"{"bad": [}"#, "a", "b").is_err());
    }
}
