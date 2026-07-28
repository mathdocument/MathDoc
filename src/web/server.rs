use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use tower_http::trace::TraceLayer;

#[cfg(feature = "dev-web")]
use tower_http::services::ServeDir;

use crate::indcache::IndCache;

use super::api;
#[cfg(not(feature = "dev-web"))]
use super::assets;
use super::AppState;

/// Start the `mdc serve` HTTP server.
///
/// `bind` — `host:port`; port `0` picks a free port.
/// `open_browser` — if true, open the default browser once listening.
pub async fn serve(
    cache: IndCache,
    bind: &str,
    open_browser: bool,
    initial_fnode: Option<&str>,
) -> Result<()> {
    let mdcroot = cache.root().to_path_buf();
    let state = AppState::new(cache);
    let app = router(state);

    let bind_addr = validate_bind(bind)?;
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;
    let addr = listener.local_addr()?;

    let url = format!("http://{addr}");
    let browser_url = initial_fnode
        .map(|fnode| format!("{url}/#ref={}", encode_fragment_value(fnode)))
        .unwrap_or_else(|| url.clone());
    eprintln!("mdc serve  →  {browser_url}");
    eprintln!("  workspace: {}", mdcroot.display());
    #[cfg(feature = "dev-web")]
    eprintln!("  (dev-web: serving from web/ — run `npm run dev` for HMR)");
    eprintln!("  Ctrl-C to stop");

    if open_browser {
        // Spawn so the server still starts even if the browser open fails.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = open::that(&browser_url);
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

pub fn router(state: AppState) -> Router {
    let api_routes = Router::new()
        .route("/graph/roots", get(api::graph_roots))
        .route("/graph/check", get(api::graph_check))
        .route("/graph/full", get(api::graph_full))
        .route("/search", get(api::search))
        .route("/resolve", get(api::resolve_ref))
        .route("/node/:fnode", get(api::node_detail))
        .route("/node/:fnode/view", get(api::node_view))
        .route("/node/:fnode/children", get(api::node_children))
        .route(
            "/node/:fnode/dep/candidates",
            get(api::node_dependency_candidates),
        )
        .route("/node/:fnode/title", put(api::node_put_title))
        .route(
            "/node/:fnode/block/:srctype",
            put(api::node_put_block).delete(api::node_delete_block),
        )
        .route("/node/:fnode/dep/add", post(api::node_add_dep))
        .route("/node/:fnode/dep/rm", post(api::node_rm_deps))
        .route("/node/new", post(api::node_new))
        .fallback(api::api_not_found)
        .layer(middleware::from_fn(api::normalize_error_response));

    let app = Router::new().nest("/api", api_routes).with_state(state);

    #[cfg(feature = "dev-web")]
    let app = {
        let web_dir = std::env::var("MDC_WEB_DIR").unwrap_or_else(|_| "web".to_string());
        with_dev_web(app, std::path::PathBuf::from(web_dir))
    };

    #[cfg(not(feature = "dev-web"))]
    let app = {
        app.fallback(get(|uri: axum::http::Uri| async move {
            assets::serve_asset(uri)
        }))
    };
    app.layer(middleware::from_fn(require_local_host))
        .layer(middleware::from_fn(disable_html_caching))
        .layer(TraceLayer::new_for_http())
}

async fn disable_html_caching(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let is_html = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if is_html {
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
    }
    response
}

async fn require_local_host(request: Request, next: Next) -> Response {
    let Some(host) = request.headers().get(axum::http::header::HOST) else {
        return (StatusCode::MISDIRECTED_REQUEST, "missing Host header").into_response();
    };
    let allowed = host
        .to_str()
        .ok()
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
        .map(|authority| authority.host().trim_matches(['[', ']']).to_string())
        .is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host.to_ascii_lowercase().ends_with(".localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
    if !allowed {
        return (StatusCode::MISDIRECTED_REQUEST, "untrusted Host header").into_response();
    }
    next.run(request).await
}

/// Parse a numeric loopback socket address. Remote binding is intentionally
/// unsupported because the Web API has no authentication layer.
pub fn validate_bind(bind: &str) -> Result<SocketAddr> {
    let addr: SocketAddr = bind
        .parse()
        .with_context(|| format!("bind address must be a numeric IP socket address: {bind}"))?;
    if !addr.ip().is_loopback() {
        bail!("refusing non-loopback bind address {addr}; mdc serve is local-only");
    }
    Ok(addr)
}

fn encode_fragment_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Attach a dev-web fallback restricted to built frontend assets.
#[cfg(feature = "dev-web")]
fn with_dev_web(app: Router, web_dir: std::path::PathBuf) -> Router {
    let dist_dir = web_dir.join("dist");
    let fallback_dir = dist_dir.clone();
    let serve = ServeDir::new(dist_dir).fallback(get(move |uri: axum::http::Uri| {
        let fallback_dir = fallback_dir.clone();
        async move { assets_spa_fallback(uri, &fallback_dir) }
    }));
    app.route(
        "/assets",
        get(|| async { (StatusCode::NOT_FOUND, "asset not found") }),
    )
    .fallback_service(serve)
}

/// SPA fallback for dev-web ServeDir — reads dist/index.html (or a stub).
#[cfg(feature = "dev-web")]
fn assets_spa_fallback(uri: axum::http::Uri, dist_dir: &std::path::Path) -> Response {
    use axum::http::header;
    use axum::http::HeaderValue;
    let request_path = uri.path().trim_start_matches('/');
    if request_path == "assets"
        || request_path.starts_with("assets/")
        || request_path
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.contains('.'))
    {
        return (StatusCode::NOT_FOUND, "asset not found").into_response();
    }
    let path = dist_dir.join("index.html");
    match std::fs::read_to_string(&path) {
        Ok(body) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            body,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "dev-web: web/dist/index.html not found — run `npm run build` or use Vite",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_fragment_value, validate_bind};

    #[test]
    fn fragment_values_are_percent_encoded() {
        assert_eq!(
            encode_fragment_value("node / 数学"),
            "node%20%2F%20%E6%95%B0%E5%AD%A6"
        );
    }

    #[test]
    fn bind_address_must_be_numeric_loopback() {
        assert!(validate_bind("127.0.0.1:0").is_ok());
        assert!(validate_bind("[::1]:7878").is_ok());

        for bind in [
            "0.0.0.0:0",
            "[::]:0",
            "192.168.1.10:7878",
            "8.8.8.8:7878",
            "localhost:7878",
        ] {
            assert!(validate_bind(bind).is_err(), "unexpectedly accepted {bind}");
        }
    }

    #[cfg(feature = "dev-web")]
    #[tokio::test]
    async fn dev_web_only_serves_dist_assets() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let dir = tempfile::TempDir::new().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(dist.join("assets")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/lib")).unwrap();
        std::fs::write(dist.join("index.html"), "<html>safe</html>").unwrap();
        std::fs::write(dist.join("assets/app.js"), "safe").unwrap();
        std::fs::write(dir.path().join("package.json"), "secret").unwrap();
        std::fs::write(dir.path().join("src/lib/api.ts"), "secret").unwrap();
        std::fs::write(dir.path().join(".env"), "secret").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "secret").unwrap();

        let app = super::with_dev_web(axum::Router::new(), dir.path().to_path_buf());
        for (path, expected) in [
            ("/assets/app.js", StatusCode::OK),
            ("/client/route", StatusCode::OK),
            ("/package.json", StatusCode::NOT_FOUND),
            ("/src/lib/api.ts", StatusCode::NOT_FOUND),
            ("/.env", StatusCode::NOT_FOUND),
            ("/%2e%2e/secret.txt", StatusCode::NOT_FOUND),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path}");
        }
    }
}
