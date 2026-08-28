use std::net::SocketAddr;

use anyhow::{bail, Context, Result};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;

use crate::indcache::WorkspaceStore;

use super::api;
use super::assets;
use super::AppState;

/// Start the `mdc serve` HTTP server.
///
/// `bind` — `host:port`; port `0` picks a free port.
pub(crate) async fn serve(
    cache: WorkspaceStore,
    bind: &str,
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

    let base_url = format!("http://{addr}");
    let url = initial_fnode
        .map(|fnode| format!("{base_url}/#ref={}", encode_fragment_value(fnode)))
        .unwrap_or(base_url);
    eprintln!("mdc serve  →  {}", crate::core::escape_terminal(&url));
    eprintln!(
        "  workspace: {}",
        crate::core::escape_terminal(&mdcroot.to_string_lossy())
    );
    eprintln!("  Ctrl-C to stop");

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
        .route("/workspace/refresh", post(api::workspace_refresh))
        .route("/search", get(api::search))
        .route("/resolve", get(api::resolve_ref))
        .route("/node/:fnode/view", get(api::node_view))
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

    Router::new()
        .nest("/api", api_routes)
        .with_state(state)
        .fallback(get(|uri: axum::http::Uri| async move {
            assets::serve_asset(uri)
        }))
        .layer(middleware::from_fn(require_local_host))
        .layer(middleware::from_fn(disable_html_caching))
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
fn validate_bind(bind: &str) -> Result<SocketAddr> {
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
}
