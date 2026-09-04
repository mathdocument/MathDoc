use axum::http::header;
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

/// Embedded frontend assets.
#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
#[prefix = ""]
pub struct WebAssets;

/// Serve an embedded asset by path. Falls back to `index.html` for any
/// unknown path so the SPA can do client-side routing.
pub(super) fn serve_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Direct file match first.
    if let Some(file) = WebAssets::get(path) {
        return asset_response(path, file);
    }

    // Missing file-like requests must be 404s. Returning the SPA shell for a
    // missing module produces a misleading 200 text/html response and a blank UI.
    if path == "assets"
        || path.starts_with("assets/")
        || path
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.contains('.'))
    {
        return (StatusCode::NOT_FOUND, "asset not found").into_response();
    }

    // SPA fallback for client-side routes.
    if let Some(file) = WebAssets::get("index.html") {
        return asset_response("index.html", file);
    }

    (StatusCode::NOT_FOUND, "frontend not built").into_response()
}

fn asset_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = HeaderValue::from_str(file.metadata.mimetype()).unwrap();
    let mut resp = (StatusCode::OK, [(header::CONTENT_TYPE, mime)], file.data).into_response();
    if path == "index.html" {
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    } else if path.starts_with("assets/") {
        // Cache hashed assets aggressively.
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0, must-revalidate"),
        );
    }
    resp
}
