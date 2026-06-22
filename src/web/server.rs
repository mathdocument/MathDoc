use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::routing::{get, post, put};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

#[cfg(feature = "dev-web")]
use axum::http::StatusCode;
#[cfg(feature = "dev-web")]
use tower_http::services::ServeDir;

use crate::indcache::IndCache;

use super::api;
use super::assets;
use super::AppState;

/// Start the `mdc serve` HTTP server.
///
/// `mdcroot` — workspace root (already validated to contain `.mdc/`).
/// `bind` — `host:port`; port `0` picks a free port.
/// `open_browser` — if true, open the default browser once listening.
pub async fn serve(
    mdcroot: PathBuf,
    cache: IndCache,
    bind: &str,
    open_browser: bool,
) -> Result<()> {
    let state = AppState::new(mdcroot.clone(), cache);

    let api_routes = Router::new()
        .route("/graph/roots", get(api::graph_roots))
        .route("/graph/check", get(api::graph_check))
        .route("/graph/full", get(api::graph_full))
        .route("/search", get(api::search))
        .route("/resolve", get(api::resolve_ref))
        .route("/node/:fnode", get(api::node_detail))
        .route("/node/:fnode/referrers", get(api::node_referrers))
        .route("/node/:fnode/children", get(api::node_children))
        .route("/node/:fnode/title", put(api::node_put_title))
        .route(
            "/node/:fnode/block/:srctype",
            put(api::node_put_block).delete(api::node_delete_block),
        )
        .route("/node/:fnode/dep/add", post(api::node_add_dep))
        .route("/node/:fnode/dep/rm", post(api::node_rm_deps))
        .route("/node/new", post(api::node_new));

    let app = Router::new()
        .nest("/api", api_routes)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    #[cfg(feature = "dev-web")]
    let app = {
        let web_dir = std::env::var("MDC_WEB_DIR").unwrap_or_else(|_| "web".to_string());
        let serve = ServeDir::new(web_dir).fallback(get(|| async { assets_spa_fallback() }));
        app.fallback_service(serve)
    };

    #[cfg(not(feature = "dev-web"))]
    let app = {
        app.fallback(get(|uri: axum::http::Uri| async move {
            assets::serve_asset(uri)
        }))
    };

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;
    let addr = listener.local_addr()?;

    let url = format!("http://{addr}");
    eprintln!("mdc serve  →  {url}");
    eprintln!("  workspace: {}", mdcroot.display());
    #[cfg(feature = "dev-web")]
    eprintln!("  (dev-web: serving from web/ — run `npm run dev` for HMR)");
    eprintln!("  Ctrl-C to stop");

    if open_browser {
        // Spawn so the server still starts even if the browser open fails.
        let url = url.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = open::that(&url);
        });
    }

    let shutdown = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("install ctrl-c handler");
        };
        #[cfg(unix)]
        let sigterm = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler")
                .recv()
                .await;
        };
        #[cfg(not(unix))]
        let sigterm = std::future::pending::<()>();
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm => {}
        }
        eprintln!("\nshutting down…");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// SPA fallback for dev-web ServeDir — reads web/index.html (or a stub).
#[cfg(feature = "dev-web")]
fn assets_spa_fallback() -> Response {
    use axum::http::header;
    use axum::http::HeaderValue;
    use axum::response::IntoResponse;
    let web_dir = std::env::var("MDC_WEB_DIR").unwrap_or_else(|_| "web".to_string());
    let path = format!("{web_dir}/index.html");
    match std::fs::read_to_string(&path) {
        Ok(body) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            )],
            body,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "dev-web: web/index.html not found — run `npm run dev` or build the frontend",
        )
            .into_response(),
    }
}

#[cfg(feature = "dev-web")]
type Response = axum::response::Response;

/// Pick a free port by binding to :0 and returning the assigned port.
/// Used only for tests; not currently wired in production code.
#[allow(dead_code)]
pub fn pick_free_port(host: &str) -> Result<SocketAddr> {
    let listener = std::net::TcpListener::bind((host, 0))?;
    Ok(listener.local_addr()?)
}
