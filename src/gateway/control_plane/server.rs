//! HTTP Server for `ControlPlane`
//!
//! Provides HTTP routes for serving `ControlPlane` static assets.

use axum::{
    extract::Path as AxumPath,
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use tower_http::compression::CompressionLayer;

use super::assets::ControlPlaneAssets;

/// Create the `ControlPlane` router
pub fn create_control_plane_router() -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/{*path}", get(serve_static_or_index))
        // Gzip the large panel payloads on the wire (the WASM alone is ~15.5 MB
        // uncompressed → ~3.7 MB gzipped). 304 revalidations carry no body, so
        // compression only runs on an actual full transfer.
        .layer(CompressionLayer::new())
}

/// Serve the index.html file
async fn serve_index() -> Response {
    match ControlPlaneAssets::get_index_html() {
        // index.html is the tiny entry point and references the JS/WASM by a
        // stable name, so it must always be revalidated — never serve a stale
        // entry point after a deploy.
        Some(content) => ([(header::CACHE_CONTROL, "no-cache")], Html(content)).into_response(),
        None => (StatusCode::NOT_FOUND, "ControlPlane index.html not found").into_response(),
    }
}

/// Serve static assets or index.html for SPA routing
pub async fn serve_static_asset(headers: HeaderMap, AxumPath(path): AxumPath<String>) -> Response {
    serve_static_or_index(headers, AxumPath(path)).await
}

/// Serve static assets or index.html for SPA routing (internal)
async fn serve_static_or_index(headers: HeaderMap, AxumPath(path): AxumPath<String>) -> Response {
    // If path is empty, just "/", or ends with "/", serve index.html
    if path.is_empty() || path == "/" || path.ends_with('/') {
        return serve_index().await;
    }

    // Try to serve as static asset first
    match ControlPlaneAssets::get(&path) {
        Some(content) => {
            // Content-hash ETag: changes exactly when the embedded asset changes
            // (i.e. on a new build), so the client can cache the asset across
            // opens and revalidate cheaply. Weak because the wire representation
            // varies with Content-Encoding (gzip vs identity).
            let etag = format!("W/\"{}\"", hex::encode(content.metadata.sha256_hash()));

            // Revalidation hit: the client already holds this exact asset → 304
            // with no body. This turns a repeat open from a multi-MB download
            // into a tiny round-trip.
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|inm| inm.split(',').any(|t| t.trim() == etag))
            {
                return (
                    StatusCode::NOT_MODIFIED,
                    [
                        (header::ETAG, etag.as_str()),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
                )
                    .into_response();
            }

            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.as_ref()),
                    // Cacheable but must revalidate via ETag before reuse: always
                    // fresh after a deploy, never re-transfers unchanged bytes.
                    (header::CACHE_CONTROL, "no-cache"),
                    (header::ETAG, etag.as_str()),
                ],
                content.data,
            )
                .into_response()
        }
        None => {
            // For SPA routing, return index.html for non-file paths
            if !path.contains('.') {
                return serve_index().await;
            }
            (StatusCode::NOT_FOUND, "Not Found").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_router() {
        let _router = create_control_plane_router();
        // Just check that it compiles
    }

    #[tokio::test]
    async fn static_asset_sets_etag_and_revalidates() {
        // The dist/ folder is committed, so an embedded asset normally exists;
        // skip gracefully in any build where the UI was not embedded.
        let Some(name) = ControlPlaneAssets::iter().find(|n| n.contains('.')) else {
            return;
        };
        let path = name.to_string();

        // First request (no validators): 200 with an ETag and revalidate policy.
        let resp = serve_static_or_index(HeaderMap::new(), AxumPath(path.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CACHE_CONTROL).unwrap(), "no-cache");
        let etag = resp.headers().get(header::ETAG).expect("etag set").clone();

        // Re-request with the matching validator: 304, no body re-transferred.
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.clone());
        let resp = serve_static_or_index(headers, AxumPath(path)).await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(resp.headers().get(header::ETAG).unwrap(), &etag);
    }
}
