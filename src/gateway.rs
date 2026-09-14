//! One stable browser/CLI entry point; branch processes retain their own leases.
use crate::{
    config::Settings,
    service::{ApiError, RunningService},
};
use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};

struct Gateway {
    client: reqwest::Client,
    info: RunningService,
    stop: tokio::sync::Notify,
}

pub(crate) async fn status() -> Result<Value> {
    let settings = Settings::load()?;
    let server = crate::service::running_service(&settings.gateway_cache()?)?;
    let mut projects = serde_json::Map::new();
    for db in crate::store::Terminus::from_env()?.projects().await? {
        let project = format!("{}/{}", db.database, db.branch);
        let running = crate::service::running_service(&db.cache_path()?)?.is_some();
        let url = server
            .as_ref()
            .filter(|_| running)
            .map(|s| format!("{}/p/{project}/", s.browser_url()));
        projects.insert(project, json!({"running":running,"url":url}));
    }
    Ok(json!({
        "server": {
            "running":server.is_some(),
            "port":server.as_ref().map(|s| s.port),
            "url":server.as_ref().map(RunningService::browser_url),
        },
        "projects":projects,
    }))
}

pub(crate) async fn serve(port: u16) -> Result<()> {
    let settings = Settings::load()?;
    let root = settings.gateway_cache()?;
    let _lease = crate::service::acquire_lease(&root)?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| format!("cannot bind port {port} (it may already be in use)"))?;
    // Fail startup with a useful error instead of launching an unusable project list.
    crate::store::Terminus::from_env()?.projects().await?;
    let state = Arc::new(Gateway {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()?,
        info: RunningService {
            port: listener.local_addr()?.port(),
            pid: std::process::id(),
            token: uuid::Uuid::new_v4().to_string(),
            public_origin: settings.public_origin()?,
        },
        stop: tokio::sync::Notify::new(),
    });
    let router = Router::new()
        .route(
            "/api/status",
            get(|| async { status().await.map(Json).map_err(ApiError::from) }),
        )
        .route("/api/service/stop", post(stop))
        .route("/p/:database/:branch", get(project_redirect))
        .route("/p/:database/:branch/", get(project_page))
        .route("/p/:database/:branch/*path", any(project_request))
        .fallback(get(|uri: axum::http::Uri| async move {
            if uri.path().starts_with("/api/") {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":"API endpoint not found"})),
                )
                    .into_response();
            }
            crate::web::assets::serve_asset(uri)
        }))
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            check_origin,
        ));
    std::fs::write(root.join("service.lock"), serde_json::to_vec(&state.info)?)?;
    use std::io::Write;
    writeln!(std::io::stdout(), "{}", serde_json::to_string(&state.info)?)?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    axum::serve(listener, router).with_graceful_shutdown(async move {
        tokio::select! { _ = state.stop.notified() => {}, _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
    }).await?;
    Ok(())
}

async fn check_origin(
    State(state): State<Arc<Gateway>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    if !crate::service::allowed_origin(request.headers(), state.info.public_origin.as_deref()) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"same-origin requests from the configured host required"})),
        )
            .into_response();
    }
    next.run(request).await
}

async fn stop(State(state): State<Arc<Gateway>>, headers: HeaderMap) -> Response {
    if headers.get("x-mdc-service").and_then(|v| v.to_str().ok()) != Some(&state.info.token) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"use mdc stop to stop the entry server"})),
        )
            .into_response();
    }
    state.stop.notify_one();
    Json(json!({"stopping":true})).into_response()
}

fn project(params: &HashMap<String, String>) -> Result<String> {
    let name = format!("{}/{}", params["database"], params["branch"]);
    crate::config::project_parts(&name)?;
    Ok(name)
}

async fn project_redirect(
    Path(params): Path<HashMap<String, String>>,
) -> Result<Redirect, ApiError> {
    Ok(Redirect::temporary(&format!("/p/{}/", project(&params)?)))
}

async fn project_page(Path(params): Path<HashMap<String, String>>) -> Result<Response, ApiError> {
    project(&params)?;
    Ok(crate::web::assets::serve_asset("/".parse().unwrap()))
}

// RFC hop-by-hop fields describe each individual connection, not the tunnel's destination.
fn strip_hop_headers(headers: &mut HeaderMap, websocket: bool) {
    let named = headers
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    for name in named.split(',').map(str::trim) {
        if !(websocket && name.eq_ignore_ascii_case("upgrade")) {
            headers.remove(name);
        }
    }
    for name in [
        "proxy-connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
    if !websocket {
        headers.remove("connection");
        headers.remove("upgrade");
    }
}

async fn project_request(
    State(state): State<Arc<Gateway>>,
    Path(params): Path<HashMap<String, String>>,
    mut request: Request,
) -> Result<Response, ApiError> {
    let project = project(&params)?;
    let path = &params["path"];
    if !path.starts_with("api/") {
        if request.method() != axum::http::Method::GET {
            return Err(ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "GET required".into(),
            ));
        }
        return Ok(crate::web::assets::serve_asset(
            format!("/{path}").parse().map_err(anyhow::Error::from)?,
        ));
    }
    // Discover by the branch lease on every request. Starts, stops and crashes
    // take effect without reloading the gateway or interrupting other WebSockets.
    let root = Settings::load()?.project_cache(&project)?;
    let worker = crate::service::running_service(&root)?.ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{project} is stopped; run mdc start {project}"),
        )
    })?;
    if path == "api/service/stop"
        && request
            .headers()
            .get("x-mdc-service")
            .and_then(|v| v.to_str().ok())
            != Some(&worker.token)
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "use mdc stop DATABASE/BRANCH".into(),
        ));
    }
    if request
        .headers()
        .get("x-mdc-service")
        .is_some_and(|token| token != worker.token.as_str())
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "service changed; retry the command".into(),
        ));
    }
    let websocket = request
        .headers()
        .get("upgrade")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    let upgrade = websocket.then(|| hyper::upgrade::on(&mut request));
    // Preserve the encoded path/query rather than reconstructing it from decoded captures.
    let prefix = format!("/p/{project}");
    let route = request
        .uri()
        .path_and_query()
        .context("missing request path")?
        .as_str()
        .strip_prefix(&prefix)
        .context("invalid project route")?
        .to_owned();
    let (mut parts, body) = request.into_parts();
    strip_hop_headers(&mut parts.headers, websocket);
    let internal = worker.url();
    parts.headers.insert(
        "host",
        format!("127.0.0.1:{}", worker.port).parse().unwrap(),
    );
    if parts.headers.contains_key("origin") {
        parts.headers.insert("origin", internal.parse().unwrap());
    }
    // Bind even browser requests to this lease, guarding against port reuse.
    parts
        .headers
        .insert("x-mdc-service", worker.token.parse().unwrap());
    for name in [
        "authorization",
        "cookie",
        "forwarded",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-forwarded-for",
    ] {
        parts.headers.remove(name);
    }
    let upstream = state
        .client
        .request(parts.method, format!("{internal}{route}"))
        .headers(parts.headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
        .map_err(|e| {
            ApiError(
                StatusCode::BAD_GATEWAY,
                format!("branch service unavailable: {e}"),
            )
        })?;
    if upstream.status() == StatusCode::SWITCHING_PROTOCOLS {
        let incoming = upgrade.context("unexpected upstream protocol upgrade")?;
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        *response.headers_mut() = upstream.headers().clone();
        strip_hop_headers(response.headers_mut(), true);
        let mut outgoing = upstream.upgrade().await.map_err(anyhow::Error::from)?;
        tokio::spawn(async move {
            if let Ok(incoming) = incoming.await {
                let mut incoming = reqwest::Upgraded::from(incoming);
                let _ = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing).await;
            }
        });
        return Ok(response);
    }
    let upstream: axum::http::Response<reqwest::Body> = upstream.into();
    let (mut parts, body) = upstream.into_parts();
    strip_hop_headers(&mut parts.headers, false);
    Ok(Response::from_parts(parts, Body::new(body)))
}
