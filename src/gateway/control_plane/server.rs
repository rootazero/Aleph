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
        // Runtime gzip for a client that does not advertise `br` — not "no
        // sibling": every embedded asset currently has one (precompressed at
        // build time by scripts/precompress_dist.mjs). The large payloads —
        // the ~22 MB WASM above all — are served from those committed `.br`
        // files by `serve_static_or_index`, and this layer passes them
        // through untouched because they already carry a `Content-Encoding`.
        // Measured on this build: wasm 21,882,715 B identity → 3,363,082 B
        // via `.br` → 5,089,368 B via this layer's runtime gzip fallback.
        // 304 revalidations carry no body, so nothing runs on a cache hit.
        .layer(CompressionLayer::new())
}

/// Does this request advertise brotli?
///
/// A deliberately simple token scan rather than full q-value negotiation: the
/// only decision here is "precompressed sibling or not", and a client that
/// sends `br;q=0` while also sending it as a token is not a real client. If a
/// weighted decision is ever needed, that is a different function, not a
/// widening of this one.
fn accepts_brotli(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.split(';').next().unwrap_or_default().trim() == "br")
        })
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
            // Content-hash ETag over the IDENTITY representation — never over
            // whichever encoding we happen to serve. An encoding-dependent
            // validator lets a client that switched `Accept-Encoding` take a
            // 304 and then decode brotli bytes as identity. Weak because the
            // wire representation varies with Content-Encoding, which is
            // exactly what `Vary` announces.
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
                        (header::VARY, "accept-encoding"),
                    ],
                )
                    .into_response();
            }

            let mime = mime_guess::from_path(&path).first_or_octet_stream();

            // Precompressed sibling, produced by `just wasm` and committed
            // alongside dist/ (scripts/precompress_dist.mjs). Serving it sets
            // `Content-Encoding`, which makes tower-http's CompressionLayer
            // pass the response straight through — so the 22 MB WASM is neither
            // gzipped at request time nor sent uncompressed. Assets without a
            // sibling fall through to the layer's gzip exactly as before.
            let brotli = accepts_brotli(&headers)
                .then(|| ControlPlaneAssets::get(&format!("{path}.br")))
                .flatten();

            match brotli {
                Some(compressed) => (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, mime.as_ref()),
                        (header::CONTENT_ENCODING, "br"),
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::ETAG, etag.as_str()),
                        (header::VARY, "accept-encoding"),
                    ],
                    compressed.data,
                )
                    .into_response(),
                None => (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, mime.as_ref()),
                        // Cacheable but must revalidate via ETag before reuse:
                        // always fresh after a deploy, never re-transfers
                        // unchanged bytes.
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::ETAG, etag.as_str()),
                        (header::VARY, "accept-encoding"),
                    ],
                    content.data,
                )
                    .into_response(),
            }
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
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );
        let etag = resp.headers().get(header::ETAG).expect("etag set").clone();

        // Re-request with the matching validator: 304, no body re-transferred.
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.clone());
        let resp = serve_static_or_index(headers, AxumPath(path)).await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(resp.headers().get(header::ETAG).unwrap(), &etag);
    }

    /// The wire fact the whole precompression design rests on.
    #[tokio::test]
    async fn brotli_is_served_when_the_client_accepts_it() {
        let Some(name) = ControlPlaneAssets::iter()
            .find(|n| ControlPlaneAssets::get(&format!("{n}.br")).is_some())
        else {
            // No precompressed asset embedded in this build (dist not built);
            // skip rather than fail — the guard for that is check_panel_dist.
            return;
        };
        let path = name.to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "br, gzip".parse().unwrap());
        let resp = serve_static_or_index(headers, AxumPath(path.clone())).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br",
            "a client advertising br must receive the precompressed sibling"
        );
        assert_eq!(
            resp.headers().get(header::VARY).unwrap(),
            "accept-encoding",
            "without Vary a shared cache would hand brotli bytes to an identity client"
        );
    }

    /// A client that does not advertise brotli keeps the old behaviour.
    #[tokio::test]
    async fn identity_is_served_when_brotli_is_not_accepted() {
        let Some(name) = ControlPlaneAssets::iter()
            .find(|n| ControlPlaneAssets::get(&format!("{n}.br")).is_some())
        else {
            return;
        };
        let path = name.to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "gzip".parse().unwrap());
        let resp = serve_static_or_index(headers, AxumPath(path)).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().get(header::CONTENT_ENCODING).is_none(),
            "a gzip-only client must get identity bytes and let CompressionLayer decide"
        );
    }

    /// The trap: the validator must describe the RESOURCE, not the encoding.
    #[tokio::test]
    async fn the_etag_does_not_change_with_the_accepted_encoding() {
        let Some(name) = ControlPlaneAssets::iter()
            .find(|n| ControlPlaneAssets::get(&format!("{n}.br")).is_some())
        else {
            return;
        };
        let path = name.to_string();

        let mut br = HeaderMap::new();
        br.insert(header::ACCEPT_ENCODING, "br".parse().unwrap());
        let with_br = serve_static_or_index(br, AxumPath(path.clone())).await;

        let plain = serve_static_or_index(HeaderMap::new(), AxumPath(path)).await;

        assert_eq!(
            with_br.headers().get(header::ETAG),
            plain.headers().get(header::ETAG),
            "an encoding-dependent ETag lets a client that switched Accept-Encoding \
             take a 304 and then decode brotli bytes as identity"
        );
    }
}
