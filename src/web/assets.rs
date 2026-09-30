//! Serves the WebUI files embedded by `build.rs`.

use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

include!(concat!(env!("OUT_DIR"), "/webui_assets.rs"));

fn find(path: &str) -> Option<(&'static str, &'static [u8])> {
    ASSETS
        .iter()
        .find(|(name, _, _)| *name == path)
        .map(|(_, content_type, bytes)| (*content_type, *bytes))
}

fn respond(content_type: &'static str, bytes: &'static [u8], cache: &'static str) -> Response {
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// Fallback handler: a file from `webui/dist`, the app for any other page
/// path, and a JSON 404 for unknown API paths.
pub(crate) async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return super::error::ApiError::not_found().into_response();
    }
    if !path.is_empty() && path != "index.html" {
        if let Some((content_type, bytes)) = find(path) {
            // Vite puts a content hash in every file name under assets/.
            let cache = if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            return respond(content_type, bytes, cache);
        }
        if path.starts_with("assets/") {
            return (StatusCode::NOT_FOUND, "Not found").into_response();
        }
    }
    match find("index.html") {
        Some((content_type, bytes)) => respond(content_type, bytes, "no-cache"),
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_page_is_always_embedded() {
        let (content_type, bytes) = find("index.html").expect("index.html is embedded");
        assert!(content_type.starts_with("text/html"));
        assert!(!bytes.is_empty());
        assert!(ASSETS.len() > 1 || !WEBUI_BUILT);
    }
}
