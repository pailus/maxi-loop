use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/dist"]
struct WebAssets;

pub async fn serve_spa(uri: Uri) -> Response {
    let raw_path = uri.path();
    if raw_path.contains("..")
        || raw_path.contains('\\')
        || raw_path.to_ascii_lowercase().contains("%2e%2e")
    {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }

    let trimmed = raw_path.trim_start_matches('/');
    let requested = if trimmed.is_empty() {
        "index.html"
    } else {
        trimmed
    };

    if let Some(file) = WebAssets::get(requested) {
        return asset_response(requested, file.data.into_owned());
    }

    // SPA fallback to index.html for client-side routes without file extensions.
    if !requested.contains('.')
        && let Some(index) = WebAssets::get("index.html")
    {
        return asset_response("index.html", index.data.into_owned());
    }

    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn asset_response(path: &str, body: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.as_ref().to_owned()),
            (header::CACHE_CONTROL, cache_control.to_owned()),
        ],
        body,
    )
        .into_response()
}
