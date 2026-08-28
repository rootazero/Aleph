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
        // This layer earns its place twice, and neither reason is "assets
        // without a sibling".
        //
        // First: a client that does not advertise `br` at all. The large
        // payloads — the ~22 MB WASM above all — are served from committed
        // `.br` files by `serve_static_or_index` when brotli IS advertised,
        // and this layer passes those through untouched because they already
        // carry a `Content-Encoding`. Measured 2026-08-22 on the dist
        // committed in `abc24d614`: wasm 21,914,484 B identity → 3,360,760 B
        // via `.br` → 5,089,074 B via this layer's runtime gzip. The choice is
        // per request, not per asset.
        //
        // Those bytes are a snapshot and go stale on the next `just wasm` —
        // an earlier copy of this comment still read "measured on this build"
        // three digits off, because the dist was rebuilt and the comment was
        // not. Date and commit are named so the next reader can tell a stale
        // figure from a wrong one. The live numbers are in the spec's §5.1
        // table, and `node scripts/precompress_dist.mjs` reprints them.
        //
        // Second: an asset below the 4 KiB floor that
        // scripts/precompress_dist.mjs compresses above. All four embedded
        // assets are currently over that floor, so all four have a sibling;
        // one added below it would have none, and `serve_static_or_index`
        // would fall through to identity for it — this layer is what still
        // compresses it.
        //
        // 304 revalidations carry no body, so nothing runs on a cache hit.
        .layer(CompressionLayer::new())
}

/// Does this request advertise brotli?
///
/// Per RFC 9110 §12.5.3, a `q=0` weight on a coding is an explicit refusal
/// of it — not merely "prefers something else". Reading it as acceptance
/// would fail silently, and lands on whoever is least expecting it: someone
/// probing with `curl -H 'Accept-Encoding: br;q=0'` to force identity bytes
/// would instead receive brotli. Honoring it costs nothing in the common
/// case (no `q` parameter, or a positive one, are both acceptance) and
/// fails safe in the refusal case (identity is always readable). A qvalue
/// that fails to parse is NOT read as a refusal — only a weight that parses
/// to zero or below is, checked numerically so `br;q=0.5` and `br;q=0.001`
/// are correctly still acceptance rather than being caught by a textual
/// match on "q=0".
///
/// Four malformed shapes stay explicitly decided rather than falling out
/// of how the parse happens to be written. No conformant client emits any
/// of them, but each one's accidental reading was *acceptance* — brotli
/// sent to a client that refused it — so each resolves toward identity,
/// the representation every client can read:
///
/// * **A refusal anywhere wins.** The scan does not stop at the first
///   accepting token, so `br;q=0.9, br;q=0` refuses. Stopping early would
///   make the answer depend on header order, and would disagree with the
///   duplicate-`q=` case one level down, where any zero already wins. Two
///   halves of one function disagreeing is how a rule nobody wrote gets
///   inferred later.
/// * **Token and parameter names match case-insensitively**, as RFC 9110
///   requires of both. Matched case-sensitively, `br;Q=0` reads as an
///   acceptance.
/// * **Whitespace around the parameter `=` is tolerated**, so `br;q = 0`
///   refuses rather than being read as a `br` with no weight at all.
/// * **A negative weight is not a positive preference**, so the test is
///   `<= 0.0` rather than `== 0.0` and `br;q=-1` refuses. Unlike the three
///   above this one was not a reported gap; it is here because the same
///   question — "is this a client asking for brotli?" — has only one
///   defensible answer for a weight below zero.
///
/// This still answers exactly one question — "precompressed sibling or
/// not" for the `br` token specifically — not full content negotiation:
/// `*` is deliberately never consulted, so `*;q=0` does not disable `br`
/// and a bare `*` does not enable it either. A weighted decision across
/// multiple codings is a different function, not a widening of this one.
fn accepts_brotli(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };

    let mut advertised = false;
    for token in value.split(',') {
        let mut segments = token.split(';');
        if !segments
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("br")
        {
            continue;
        }
        advertised = true;
        // A qvalue of zero (`0`, `0.0`, `0.000`, …) is an explicit refusal,
        // as is a negative one — neither is a positive preference. Anything
        // else — a positive weight, no `q` parameter, or one that fails to
        // parse — is acceptance.
        let refused = segments.any(|param| {
            let Some((name, weight)) = param.split_once('=') else {
                return false;
            };
            name.trim().eq_ignore_ascii_case("q")
                && weight.trim().parse::<f64>().is_ok_and(|q| q <= 0.0)
        });
        if refused {
            return false;
        }
    }
    advertised
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

    /// The one claim in this module that no other test can make: a response
    /// that already carries `Content-Encoding: br` passes through
    /// `CompressionLayer` untouched.
    ///
    /// Every other test here calls `serve_static_or_index` DIRECTLY, so the
    /// layer is never in the path — including the one whose message says "let
    /// CompressionLayer decide". The whole reason the 22 MB wasm is cheap is
    /// that the layer declines to re-encode it, and until now that rested on a
    /// live measurement plus a reading of tower-http's source. A dependency
    /// bump could reintroduce double-encoding with the suite green.
    ///
    /// So this one goes through `create_control_plane_router()`, which is the
    /// only way the layer is exercised at all.
    #[tokio::test]
    async fn the_compression_layer_passes_through_a_precompressed_response() {
        use tower::ServiceExt;

        // Discover the precompressed asset rather than naming
        // `aleph_panel_bg.wasm`: `interfaces/webchat/dist/` is gitignored and
        // produced by `just wasm`, so on a bare checkout — including this
        // crate's own CI job, which runs `cargo test` with no WASM toolchain —
        // that path 404s and the hard-coded version was red for everyone who
        // had not built the Panel. Its two siblings below
        // (`brotli_is_served_when_the_client_accepts_it`,
        // `identity_is_served_when_brotli_is_not_accepted`) already skip on the
        // same condition; this one was the odd one out in its own module. The
        // guard for "dist should exist at all" is `check_panel_dist.mjs`.
        let Some(name) = ControlPlaneAssets::iter()
            .find(|n| ControlPlaneAssets::get(&format!("{n}.br")).is_some())
        else {
            return;
        };

        let request = axum::http::Request::builder()
            .uri(format!("/{name}"))
            .header(header::ACCEPT_ENCODING, "br, gzip")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = create_control_plane_router()
            .oneshot(request)
            .await
            .expect("route");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok()),
            Some("br"),
            "the precompressed sibling must survive the layer as `br`; `gzip` here \
             means the layer re-encoded 22 MB it was supposed to pass through"
        );

        // A single encoding, not a stacked list — `br, gzip` in this header is
        // the wire signature of double-encoding, and it is the failure this
        // test exists for.
        let raw = response
            .headers()
            .get_all(header::CONTENT_ENCODING)
            .iter()
            .count();
        assert_eq!(
            raw, 1,
            "double-encoded: more than one Content-Encoding header"
        );
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
        assert_eq!(
            resp.headers().get(header::VARY).unwrap(),
            "accept-encoding",
            "a 304 without Vary lets a shared cache reuse this validation across \n             encodings — the same hole Vary closes on the 200s"
        );
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
        assert_eq!(
            resp.headers().get(header::VARY).unwrap(),
            "accept-encoding",
            "the identity arm needs Vary as much as the brotli arm: without it a \n             cache can hand these bytes to a client that would have got `br`"
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

    // --- accepts_brotli: RFC 9110 §12.5.3 qvalue handling -----------------
    //
    // The trap a textual match on "q=0" falls into: `br;q=0.5` and
    // `br;q=0.001` contain the substring "q=0" but are nonzero, positive
    // weights — acceptance, not refusal. These are unit tests on the
    // helper directly so they run regardless of whether dist/ is embedded.

    #[test]
    fn accepts_brotli_rejects_every_spelling_of_an_explicit_zero_qvalue() {
        for spelling in ["br;q=0", "br;q=0.0", "br;q=0.00", "br;q=0.000"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, spelling.parse().unwrap());
            assert!(
                !accepts_brotli(&headers),
                "{spelling} is an explicit refusal (RFC 9110 §12.5.3) and must not be read \
                 as acceptance"
            );
        }
    }

    #[test]
    fn accepts_brotli_accepts_a_low_but_nonzero_qvalue() {
        for spelling in ["br;q=0.5", "br;q=0.001", "br;q=1", "br;q=1.0", "br"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, spelling.parse().unwrap());
            assert!(
                accepts_brotli(&headers),
                "{spelling} is a nonzero weight (or no weight at all) — the trap a textual \
                 match on \"q=0\" would fall into by rejecting 0.5 and 0.001 as substrings"
            );
        }
    }

    #[test]
    fn accepts_brotli_does_not_read_a_malformed_qvalue_as_refusal() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "br;q=abc".parse().unwrap());
        assert!(
            accepts_brotli(&headers),
            "an unparseable qvalue is not a refusal; only a weight that parses to zero \
             or below is"
        );
    }

    #[test]
    fn accepts_brotli_lets_a_refusal_win_over_a_duplicate_accepting_token() {
        // Self-contradictory and outside the ABNF, so no real client sends
        // it — but whichever way it is answered must not depend on which
        // token came first, and the within-token duplicate-`q=` case already
        // lets any zero win.
        for spelling in ["br;q=0.9, br;q=0", "br;q=0, br;q=0.9"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, spelling.parse().unwrap());
            assert!(
                !accepts_brotli(&headers),
                "{spelling}: a refusal anywhere in the header must win, in either order — \
                 otherwise the answer depends on token order and disagrees with how a \
                 repeated `q=` inside one token is already resolved"
            );
        }
    }

    #[test]
    fn accepts_brotli_matches_the_token_and_the_q_parameter_case_insensitively() {
        // RFC 9110 makes both the content-coding token and the parameter
        // name case-insensitive. `br;Q=0` matched case-sensitively would be
        // read as acceptance — brotli sent to a client that refused it.
        let mut upper_token = HeaderMap::new();
        upper_token.insert(header::ACCEPT_ENCODING, "BR".parse().unwrap());
        assert!(
            accepts_brotli(&upper_token),
            "`BR` is the same content coding as `br`"
        );

        let mut upper_param = HeaderMap::new();
        upper_param.insert(header::ACCEPT_ENCODING, "br;Q=0".parse().unwrap());
        assert!(
            !accepts_brotli(&upper_param),
            "`Q=0` is the same refusal as `q=0`; reading it as acceptance is the unsafe \
             direction"
        );
    }

    #[test]
    fn accepts_brotli_tolerates_whitespace_around_the_parameter_equals() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "br;q = 0".parse().unwrap());
        assert!(
            !accepts_brotli(&headers),
            "`br;q = 0` is outside the ABNF, but reading it as a `br` with no weight at \
             all serves brotli to a client that refused it"
        );
    }

    #[test]
    fn accepts_brotli_does_not_read_a_negative_weight_as_a_preference() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "br;q=-1".parse().unwrap());
        assert!(
            !accepts_brotli(&headers),
            "a negative weight is not a positive preference by any reading; identity is \
             the safe answer"
        );
    }

    #[test]
    fn accepts_brotli_does_not_widen_to_the_wildcard() {
        // Scope is exactly the explicit `br` token — `*` is never consulted,
        // in either direction.
        let mut refuses_only_br = HeaderMap::new();
        refuses_only_br.insert(header::ACCEPT_ENCODING, "gzip, *;q=0".parse().unwrap());
        assert!(
            !accepts_brotli(&refuses_only_br),
            "no `br` token is present"
        );

        let mut wildcard_only = HeaderMap::new();
        wildcard_only.insert(header::ACCEPT_ENCODING, "*;q=1".parse().unwrap());
        assert!(
            !accepts_brotli(&wildcard_only),
            "a bare wildcard must not enable brotli — that would be a widening of this \
             function into full content negotiation"
        );
    }

    /// The corrected trap, exercised through the response: an explicit
    /// refusal must fall through to identity, not be treated as bare
    /// "does the token appear" acceptance.
    #[tokio::test]
    async fn identity_is_served_when_brotli_is_explicitly_refused_via_qzero() {
        let Some(name) = ControlPlaneAssets::iter()
            .find(|n| ControlPlaneAssets::get(&format!("{n}.br")).is_some())
        else {
            return;
        };
        let path = name.to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "br;q=0".parse().unwrap());
        let resp = serve_static_or_index(headers, AxumPath(path)).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().get(header::CONTENT_ENCODING).is_none(),
            "br;q=0 is an explicit refusal (RFC 9110 §12.5.3) and must get identity bytes"
        );
    }

    /// A low-but-nonzero weight is still acceptance on the wire, not the
    /// trap a naive textual match on "q=0" would fall into.
    #[tokio::test]
    async fn brotli_is_served_for_a_low_but_nonzero_qvalue() {
        let Some(name) = ControlPlaneAssets::iter()
            .find(|n| ControlPlaneAssets::get(&format!("{n}.br")).is_some())
        else {
            return;
        };
        let path = name.to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "br;q=0.001".parse().unwrap());
        let resp = serve_static_or_index(headers, AxumPath(path)).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br",
            "a nonzero weight, however low, is still acceptance"
        );
    }
}
