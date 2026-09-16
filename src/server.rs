//! One HTTP listener; branch routers and Lean environments live in this process.
use crate::{
    config::Settings,
    service::{ApiError, RunningService, Service},
};
use anyhow::{Context, Result};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

struct Branch {
    service: Arc<Service>,
    router: Router,
}
struct Server {
    info: RunningService,
    // None reserves a branch while loading or stopping. Never hold this map's
    // lock over database/Lean work; other branches remain responsive.
    branches: Mutex<HashMap<String, Option<Branch>>>,
    operations: RwLock<()>,
    closing: AtomicBool,
    stop: tokio::sync::Notify,
    restore_path: Option<PathBuf>,
    remembered: Mutex<BTreeSet<String>>,
}

pub(crate) async fn status() -> Result<Value> {
    let settings = Settings::load()?;
    let server = crate::service::running_service(&settings.server_cache()?)?;
    let mut projects = serde_json::Map::new();
    for db in crate::store::Terminus::from_env()?.projects().await? {
        let project = format!("{}/{}", db.database, db.branch);
        let branch = crate::service::running_service(&db.cache_path()?)?;
        let running = server
            .as_ref()
            .zip(branch.as_ref())
            .is_some_and(|(s, b)| s.pid == b.pid && s.port == b.port);
        let url = server
            .as_ref()
            .filter(|_| running)
            .map(|s| format!("{}/p/{project}/", s.browser_url()));
        projects.insert(project, json!({"running":running,"url":url}));
    }
    Ok(json!({
        "server": {"running":server.is_some(),
            "port":server.as_ref().map(|s| s.port),
            "url":server.as_ref().map(RunningService::browser_url)},
        "projects":projects,
    }))
}

impl Server {
    async fn remember(&self, project: &str, running: bool) -> Result<()> {
        let Some(path) = &self.restore_path else {
            return Ok(());
        };
        let mut remembered = self.remembered.lock().await;
        let mut next = remembered.clone();
        if running {
            next.insert(project.into());
        } else {
            next.remove(project);
        }
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        serde_json::to_writer(&mut file, &next)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        *remembered = next;
        Ok(())
    }

    async fn start_branch(&self, project: String) -> Result<Value> {
        let _operation = self.operations.read().await;
        anyhow::ensure!(!self.closing.load(Ordering::Acquire), "server is stopping");
        let (database, name) = crate::config::project_parts(&project)?;
        {
            let mut branches = self.branches.lock().await;
            anyhow::ensure!(
                !branches.contains_key(&project),
                "{project} is already running or changing state"
            );
            branches.insert(project.clone(), None);
        }
        let opened = async {
            let service = Service::open(crate::store::Database::from_env(
                database.into(),
                name.into(),
            )?)
            .await?;
            let root = service.db.cache_path()?;
            let info = RunningService {
                port: self.info.port,
                pid: self.info.pid,
                token: service.token.clone(),
                public_origin: self.info.public_origin.clone(),
            };
            std::fs::write(root.join("service.lock"), serde_json::to_vec(&info)?)?;
            self.remember(&project, true).await?;
            Ok::<_, anyhow::Error>(Branch {
                router: crate::service::router(service.clone()),
                service,
            })
        }
        .await;
        match opened {
            Ok(branch) => {
                self.branches
                    .lock()
                    .await
                    .insert(project.clone(), Some(branch));
            }
            Err(error) => {
                self.branches.lock().await.remove(&project);
                return Err(error);
            }
        }
        Ok(
            json!({"project":project,"port":self.info.port,"pid":self.info.pid,
            "url":format!("{}/p/{project}/", self.info.browser_url()),
            "log":Settings::load()?.server_cache()?.join("service.log")}),
        )
    }

    async fn stop_branch(&self, project: String, token: String) -> Result<Value, ApiError> {
        let _operation = self.operations.read().await;
        let branch = {
            let mut branches = self.branches.lock().await;
            let slot = branches
                .get_mut(&project)
                .ok_or_else(|| stopped(&project))?;
            let branch = slot.as_ref().ok_or_else(|| stopped(&project))?;
            authorize_token(&token, &branch.service.token)?;
            self.remember(&project, false).await?;
            slot.take().unwrap()
        };
        branch.service.shutdown().await;
        drop(branch);
        self.branches.lock().await.remove(&project);
        Ok(json!({"project":project,"stopped":true}))
    }

    async fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        let _operations = self.operations.write().await;
        let branches = std::mem::take(&mut *self.branches.lock().await);
        futures_util::future::join_all(branches.into_values().flatten().map(|branch| async move {
            branch.service.shutdown().await;
        }))
        .await;
    }
}

pub(crate) async fn serve(port: u16, foreground: bool) -> Result<()> {
    let settings = Settings::load()?;
    let root = settings.server_cache()?;
    let listener = tokio::net::TcpListener::bind((settings.listen_address()?, port))
        .await
        .with_context(|| format!("cannot bind port {port} (it may already be in use)"))?;
    let _lease = crate::service::acquire_lease(&root)?;
    crate::store::Terminus::from_env()?.projects().await?;
    let restore_path = foreground.then(|| root.join("active-projects.json"));
    let remembered: BTreeSet<String> = match restore_path.as_ref().map(std::fs::read) {
        Some(Ok(bytes)) => {
            serde_json::from_slice(&bytes).context("invalid active-projects.json")?
        }
        Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error.into())
        }
        _ => BTreeSet::new(),
    };
    for project in &remembered {
        crate::config::project_parts(project)?;
    }
    let state = Arc::new(Server {
        info: RunningService {
            port: listener.local_addr()?.port(),
            pid: std::process::id(),
            token: uuid::Uuid::new_v4().to_string(),
            public_origin: settings.public_origin()?,
        },
        branches: Mutex::new(HashMap::new()),
        operations: RwLock::new(()),
        closing: AtomicBool::new(false),
        stop: tokio::sync::Notify::new(),
        restore_path,
        remembered: Mutex::new(remembered.clone()),
    });
    for project in remembered {
        if let Err(error) = state.start_branch(project.clone()).await {
            eprintln!("could not restore {project}: {error:#}");
        }
    }
    let router = Router::new()
        .route(
            "/api/status",
            get(|| async { status().await.map(Json).map_err(ApiError::from) }),
        )
        .route("/api/service/stop", post(stop))
        // Dispatch before branch path matching, retaining the native WebSocket
        // upgrade extension. There is no HTTP client or second TCP listener.
        .fallback(any(project_request))
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            check_origin,
        ));
    std::fs::write(root.join("service.lock"), serde_json::to_vec(&state.info)?)?;
    if foreground {
        writeln!(
            std::io::stdout(),
            "{}",
            json!({"port":state.info.port,"pid":state.info.pid,"url":state.info.browser_url()})
        )?;
    } else {
        writeln!(std::io::stdout(), "{}", serde_json::to_string(&state.info)?)?;
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    axum::serve(listener, router).with_graceful_shutdown(async move {
        tokio::select! { _ = state.stop.notified() => {}, _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
        state.shutdown().await;
    }).await?;
    Ok(())
}

async fn check_origin(
    State(state): State<Arc<Server>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    if state.closing.load(Ordering::Acquire) {
        return stopped("server").into_response();
    }
    if !crate::service::allowed_origin(request.headers(), state.info.public_origin.as_deref()) {
        return ApiError(
            StatusCode::FORBIDDEN,
            "same-origin requests from the configured host required".into(),
        )
        .into_response();
    }
    next.run(request).await
}

fn authorize_token(actual: &str, expected: &str) -> Result<(), ApiError> {
    if actual != expected {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "use mdc start/stop to manage services".into(),
        ));
    }
    Ok(())
}
fn token(headers: &HeaderMap) -> &str {
    headers
        .get("x-mdc-service")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
fn stopped(project: &str) -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        format!("{project} is stopped or changing state; run mdc start {project}"),
    )
}
async fn stop(
    State(state): State<Arc<Server>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize_token(token(&headers), &state.info.token)?;
    state.stop.notify_one();
    Ok(Json(json!({"stopping":true})))
}

async fn project_request(
    State(state): State<Arc<Server>>,
    mut request: Request,
) -> Result<Response, ApiError> {
    let path = request.uri().path().to_owned();
    let Some(rest) = path.strip_prefix("/p/") else {
        if path.starts_with("/api/") {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "API endpoint not found".into(),
            ));
        }
        if request.method() != Method::GET {
            return Err(ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "GET required".into(),
            ));
        }
        return Ok(crate::web::assets::serve_asset(request.uri().clone()));
    };
    let mut segments = rest.splitn(3, '/');
    let project = format!(
        "{}/{}",
        segments.next().unwrap_or(""),
        segments.next().unwrap_or("")
    );
    crate::config::project_parts(&project)?;
    let suffix = segments.next();
    if suffix.is_none() {
        let query = request
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        return Ok(Redirect::temporary(&format!("/p/{project}/{query}")).into_response());
    }
    let suffix = suffix.unwrap();
    if !suffix.starts_with("api/") {
        if request.method() != Method::GET {
            return Err(ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "GET required".into(),
            ));
        }
        return Ok(crate::web::assets::serve_asset(
            format!("/{suffix}").parse().map_err(anyhow::Error::from)?,
        ));
    }
    if matches!(suffix, "api/service/start" | "api/service/stop") {
        if request.method() != Method::POST {
            return Err(ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "POST required".into(),
            ));
        }
        let token = token(request.headers()).to_owned();
        if suffix == "api/service/start" {
            authorize_token(&token, &state.info.token)?;
            // A client disconnect must not leave a reserved branch stuck loading.
            return tokio::spawn(async move { state.start_branch(project).await })
                .await
                .map_err(anyhow::Error::from)?
                .map(Json)
                .map(IntoResponse::into_response)
                .map_err(ApiError::from);
        }
        return tokio::spawn(async move { state.stop_branch(project, token).await })
            .await
            .map_err(anyhow::Error::from)?
            .map(Json)
            .map(IntoResponse::into_response);
    }
    let router = state
        .branches
        .lock()
        .await
        .get(&project)
        .and_then(|slot| slot.as_ref())
        .map(|b| b.router.clone())
        .ok_or_else(|| stopped(&project))?;
    let route = request
        .uri()
        .path_and_query()
        .context("missing request path")?
        .as_str()
        .strip_prefix(&format!("/p/{project}"))
        .context("invalid project path")?;
    *request.uri_mut() = route.parse().map_err(anyhow::Error::from)?;
    Ok(router.oneshot(request).await.unwrap())
}
