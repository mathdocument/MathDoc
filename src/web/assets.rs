use axum::http::header;
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

/// Embedded frontend assets (release builds).
///
/// In `dev-web` builds this module is unused — `tower-http ServeDir` serves
/// `web/` directly so Vite hot-reload works. The struct still needs to compile,
/// so we point rust-embed at `web/dist` which always exists (a placeholder
/// index.html is committed for fresh clones before the frontend is built).
#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
#[prefix = ""]
pub struct WebAssets;

/// Serve an embedded asset by path. Falls back to `index.html` for any
/// unknown path so the SPA can do client-side routing.
pub fn serve_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Direct file match first.
    if let Some(file) = WebAssets::get(path) {
        return asset_response(path, file);
    }

    // SPA fallback: serve index.html for non-API, non-file paths.
    if let Some(file) = WebAssets::get("index.html") {
        return asset_response("index.html", file);
    }

    (StatusCode::NOT_FOUND, "frontend not built").into_response()
}

fn asset_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap())],
        file.data,
    )
        .into_response();
    if path != "index.html" {
        // Cache hashed assets aggressively; never cache index.html.
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    resp
}
