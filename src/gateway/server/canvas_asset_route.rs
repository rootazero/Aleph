//! `GET /canvas-asset/{cap}/{canvas_id}/{asset_id}` — the whiteboard asset
//! byte route.
//!
//! # This is a second ingress
//!
//! The exact posture of [`super::artifact_route`] (read its module doc
//! first): a plain HTTP `GET` inherits none of `/ws`'s guards — no `connect`
//! handshake, no device tier, no session — so each one is restated here
//! explicitly rather than assumed:
//!
//! | Guard | `/ws` | here |
//! |---|---|---|
//! | Plaintext-remote refusal | [`refuse_insecure_remote`] | same function, 426 |
//! | Cross-origin / DNS-rebinding | [`OriginPolicy`] | same policy, 403 |
//! | Rate limiting | shared `RateLimiter` | own limiter, own bucket, 429 |
//! | Authorization | `connect` + device tier | capability in the path |
//! | Scope | session per connection | canvas *resolved from* the capability |
//!
//! The capability is a bearer secret and therefore a **path** segment, never
//! a query parameter — this router gets no `RedactQueryLayer`, and a `?cap=`
//! would survive into any access log. Nothing in this module ever logs it.
//!
//! # Why the client cannot name the scope
//!
//! The URL does carry a `canvas_id`, but it is not trusted as the scope:
//! [`CanvasCapabilities::canvas_for`] resolves the canvas server-side from
//! the capability, and the URL's segment must MATCH it — a mismatch is a
//! plain not-found. The store is then asked only within that canvas, and its
//! own asset-id parser (exactly 64 hex digits, one whitelisted extension) is
//! the traversal gate.
//!
//! # The XSS boundary
//!
//! Two asset types are documents to a browser:
//!
//! * **`text/html`** is served as `Content-Type: text/plain` — the ruling,
//!   not a fallback. HTML on a canvas renders ONLY inside the Panel's
//!   sandboxed iframe `srcdoc`; a capability URL opened directly must never
//!   become a same-origin HTML page, because that page could reach the
//!   gateway origin's storage and RPCs. Plain text renders the source
//!   harmlessly and keeps the bytes readable/debuggable.
//! * **`image/svg+xml`** keeps its type (an `<image href>` cannot render it
//!   as `text/plain`) but carries [`ARTIFACT_DOCUMENT_CSP`] — `default-src
//!   'none'` plus `sandbox`, which forces an opaque origin with scripting
//!   disabled on a direct open, while leaving subresource (image) use
//!   untouched (a CSP applies to the response as a *document*).
//!
//! Assets are content-addressed (`<sha256>.<ext>`) and therefore immutable,
//! so every success carries `Cache-Control: private, max-age=3600`: private
//! because the URL embeds a capability minted per visibility verdict, an
//! hour because the bytes under a given id can never change — only vanish.

use std::net::{IpAddr, SocketAddr};

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tracing::{debug, warn};

use crate::canvas::{CanvasError, CanvasStore};
use crate::gateway::origin_policy::OriginPolicy;
use crate::gateway::rate_limiter::{
    RateLimitConfig, RateLimitKey, RateLimitScope, RateLimiter, WindowConfig,
};
use crate::gateway::security::CanvasCapabilities;
use crate::gateway::server::byte_range::{parse_range, RangeVerdict};
use crate::gateway::trusted_proxy::resolve_client;
use crate::sync_primitives::Arc;

use super::artifact_route::ARTIFACT_DOCUMENT_CSP;
use super::handler::refuse_insecure_remote;

/// Per-minute byte reads allowed per remote IP.
///
/// A board full of images issues a burst of `<image>` requests, so this sits
/// far above the RPC buckets — same rationale and same number as the
/// artifact route's. Loopback is exempt, so the desktop App is unaffected.
const CANVAS_ASSET_READS_PER_MINUTE: u32 = 240;

/// Per-minute Range reads allowed per remote IP — the wide bucket a media
/// scrub draws from.
///
/// A ranged request is provisionally priced from this bucket (before the
/// body is read, `total` and the verdict do not exist yet to say otherwise);
/// if it turns out to be asking for the WHOLE resource it pays
/// [`CANVAS_ASSET_READS_PER_MINUTE`] too, once that is known. Same value and
/// the same reasoning as the artifact route's
/// `ARTIFACT_RANGE_READS_PER_MINUTE` — kept as a separate constant because
/// this is a separate limiter guarding a separate resource (canvas assets,
/// not artifacts), and sharing one constant across the two would couple them
/// for no reason.
const CANVAS_RANGE_READS_PER_MINUTE: u32 = 3_000;

/// Immutable-bytes cache policy (see the module doc).
const CACHE_CONTROL: &str = "private, max-age=3600";

/// Everything the byte route needs — and nothing else. Deliberately not
/// `GatewaySharedState`, for the same testability-and-least-knowledge
/// reasons as [`super::artifact_route::ArtifactRouteState`].
pub struct CanvasAssetRouteState {
    store: Arc<CanvasStore>,
    /// Private limiter, so image bursts cannot drain the bucket `chat.send`
    /// draws from. `RateLimitConfig::max_entries` bounds it.
    rate_limiter: RateLimiter,
    origin_policy: Arc<OriginPolicy>,
    trusted_proxy_enabled: bool,
    trusted_proxy_ips: Vec<IpAddr>,
    allow_insecure_remote: bool,
    tls_enabled: bool,
}

impl CanvasAssetRouteState {
    /// Build the route state from the gateway's already-resolved policy.
    #[must_use]
    pub fn new(
        store: Arc<CanvasStore>,
        origin_policy: Arc<OriginPolicy>,
        trusted_proxy_enabled: bool,
        trusted_proxy_ips: Vec<IpAddr>,
        allow_insecure_remote: bool,
        tls_enabled: bool,
    ) -> Self {
        Self {
            store,
            rate_limiter: RateLimiter::new(RateLimitConfig {
                rpc_heavy: WindowConfig {
                    max_requests: CANVAS_ASSET_READS_PER_MINUTE,
                    window_secs: 60,
                    lockout_secs: None,
                },
                // This limiter is private to the route, and `RpcRealtime` is
                // otherwise unused in it, so it carries the wide Range
                // bucket instead of widening the shared `RateLimitScope`
                // enum for one caller — same choice as the artifact route's.
                rpc_realtime: WindowConfig {
                    max_requests: CANVAS_RANGE_READS_PER_MINUTE,
                    window_secs: 60,
                    lockout_secs: None,
                },
                ..RateLimitConfig::default()
            }),
            origin_policy,
            trusted_proxy_enabled,
            trusted_proxy_ips,
            allow_insecure_remote,
            tls_enabled,
        }
    }
}

/// The byte route, ready to `merge` into the gateway router.
pub fn canvas_asset_routes(state: Arc<CanvasAssetRouteState>) -> Router {
    Router::new()
        .route(
            "/canvas-asset/{cap}/{canvas_id}/{asset_id}",
            get(serve_canvas_asset),
        )
        .with_state(state)
}

/// Every refusal that must not distinguish "wrong capability" from "no such
/// asset" — an attacker learns nothing about what exists.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "canvas asset not found").into_response()
}

/// The `Content-Type` an asset is SERVED under — the XSS ruling (module
/// doc): `text/html` downgrades to plain text; everything else keeps the
/// store's recorded type.
fn served_content_type(mime: &str) -> &str {
    if mime.eq_ignore_ascii_case("text/html") {
        "text/plain; charset=utf-8"
    } else {
        mime
    }
}

/// `GET /canvas-asset/{cap}/{canvas_id}/{asset_id}`.
async fn serve_canvas_asset(
    State(state): State<Arc<CanvasAssetRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((cap, canvas_id, asset_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    // Resolve the effective client first: behind a trusted proxy the
    // transport peer is the proxy, and every guard below keys on the real
    // client.
    let resolved = resolve_client(
        peer.ip(),
        &headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let client_ip = resolved.ip;
    let secure = state.tls_enabled || resolved.secure;

    // 1. Plaintext-remote refusal, identical to the `/ws` upgrade.
    if refuse_insecure_remote(client_ip, secure, state.allow_insecure_remote) {
        warn!(
            client = %client_ip,
            "refused canvas asset read: insecure transport to a remote client"
        );
        return (
            StatusCode::UPGRADE_REQUIRED,
            "TLS required for remote canvas asset reads",
        )
            .into_response();
    }

    // 2. Cross-origin / DNS-rebinding guard. An `<image>` load sends no
    //    `Origin` and passes; a cross-origin `fetch` is held to policy.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !state.origin_policy.is_allowed(origin, host) {
        warn!(client = %client_ip, "refused canvas asset read: disallowed origin");
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }

    // 3. Own rate limit, before any filesystem work. A ranged request is
    //    provisionally priced from the wider bucket so a media scrub is not
    //    throttled; if it turns out to be asking for the whole resource it
    //    also pays the narrow bucket at step 9, once we know that. This half
    //    cannot make that call — it runs before the read, so `total` and the
    //    verdict do not exist yet — and it is deliberately still here so a
    //    flood is refused before it costs any filesystem work.
    let has_range = headers.contains_key(header::RANGE);
    let scope = if has_range {
        RateLimitScope::RpcRealtime
    } else {
        RateLimitScope::RpcHeavy
    };
    let key = RateLimitKey::new(&client_ip.to_string(), scope);
    if let Err(e) = state.rate_limiter.check_and_record(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, e.retry_after_secs().to_string())],
            "canvas asset read rate limit exceeded",
        )
            .into_response();
    }

    // 4. Capability → canvas, resolved server-side; the URL must then name
    //    that same canvas. Unknown, expired, or mismatched is
    //    indistinguishable from a missing asset on the wire.
    let Some(authorized_canvas) = CanvasCapabilities::canvas_for(&cap) else {
        return not_found();
    };
    if authorized_canvas != canvas_id {
        return not_found();
    }

    // 5. Server-side resolution. The store validates both segments to the
    //    exact shapes its writer can mint (id charset, `<sha256>.<ext>`)
    //    before any path is built — no client-supplied path reaches the
    //    filesystem.
    let (mime, bytes) = match state.store.read_asset(&canvas_id, &asset_id).await {
        Ok(found) => found,
        Err(e @ (CanvasError::NotFound(_) | CanvasError::Invalid(_))) => {
            // A miss is the designed answer to a probe, not an operator
            // event — at `warn` a capability holder could flood the log by
            // walking ids.
            debug!(client = %client_ip, error = %e, "canvas asset read missed");
            return not_found();
        }
        Err(e) => {
            // Real trouble — an unreadable blob — still surfaces, because
            // that is a disk problem an operator can act on.
            warn!(client = %client_ip, error = %e, "canvas asset read failed");
            return not_found();
        }
    };

    // 6. Serve. `nosniff` is global (SecurityHeadersLayer); the type itself
    //    is the XSS ruling (module doc).
    let content_type = HeaderValue::from_str(served_content_type(&mime))
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));

    // 7. Representation. This runs LAST, after every gate above: a range
    //    must never be the reason a byte is reachable.
    let total = bytes.len() as u64;
    let verdict = parse_range(
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        total,
    );

    let status = match verdict {
        RangeVerdict::Whole => StatusCode::OK,
        RangeVerdict::Satisfiable { .. } => StatusCode::PARTIAL_CONTENT,
        RangeVerdict::Unsatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
    };

    // A ranged request that turns out to be pulling most of the asset is a
    // full read wearing a header, so it pays the narrow bucket too. Without
    // this, which bucket you draw from is your own choice — `bytes=0-`
    // returns everything as a 206, and a malformed `Range` returns
    // everything as a 200. The predicate is what we are about to SEND, never
    // what was asked, and it lives in [`RangeVerdict::is_bulk_read`] rather
    // than here: this route and the artifact route were carrying two copies
    // of it, and the copy is how the `bytes=1-` hole got duplicated.
    if has_range && verdict.is_bulk_read(total) {
        let narrow = RateLimitKey::new(&client_ip.to_string(), RateLimitScope::RpcHeavy);
        if let Err(e) = state.rate_limiter.check_and_record(&narrow) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, e.retry_after_secs().to_string())],
                "canvas asset read rate limit exceeded",
            )
                .into_response();
        }
    }

    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_CONTROL),
        )
        // Advertised on EVERY response, including the refusals above: this
        // is how a media element learns it may seek at all.
        .header(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    if let Some(cr) = verdict.content_range(total) {
        if let Ok(v) = HeaderValue::from_str(&cr) {
            response = response.header(header::CONTENT_RANGE, v);
        }
    }

    // SVG stays a document type for `<image href>`'s sake; the CSP is what
    // neuters a direct open (opaque origin, no scripting). A 206/416 is
    // still part of the document, so both need the same policy the 200
    // gets.
    if mime.eq_ignore_ascii_case("image/svg+xml") {
        response = response.header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(ARTIFACT_DOCUMENT_CSP),
        );
    }

    let body = match verdict {
        RangeVerdict::Whole => Body::from(bytes),
        RangeVerdict::Satisfiable { start, end } => {
            // `parse_range` guarantees start <= end < total, so these casts
            // and the slice are in range.
            let (s, e) = (start as usize, end as usize);
            Body::from(bytes[s..=e].to_vec())
        }
        RangeVerdict::Unsatisfiable => Body::empty(),
    };

    response.body(body).unwrap_or_else(|_| not_found())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tempfile::TempDir;
    use tower::ServiceExt;

    const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
    /// A non-loopback address, so rate limiting actually applies (loopback is
    /// exempt).
    const REMOTE: [u8; 4] = [203, 0, 113, 7];

    struct Fixture {
        _tmp: TempDir,
        app: Router,
        store: Arc<CanvasStore>,
    }

    /// A route wired the way a loopback desktop App reaches it.
    fn fixture() -> Fixture {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(CanvasStore::new(tmp.path().to_path_buf()));
        let state = Arc::new(CanvasAssetRouteState::new(
            store.clone(),
            Arc::new(OriginPolicy::loopback_only()),
            false,
            Vec::new(),
            true,
            false,
        ));
        Fixture {
            _tmp: tmp,
            app: canvas_asset_routes(state),
            store,
        }
    }

    /// A canvas holding one asset; returns `(canvas_id, asset_id)`.
    async fn seeded(fx: &Fixture, mime: &str, bytes: &[u8]) -> (String, String) {
        let doc = fx.store.create(None, None, None).await.expect("create");
        let asset_id = fx
            .store
            .put_asset(&doc.id, mime, bytes)
            .await
            .expect("put_asset");
        (doc.id, asset_id)
    }

    fn request(uri: &str) -> Request<Body> {
        let mut req = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("request");
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((LOOPBACK, 40000))));
        req
    }

    /// Build a request carrying a Range header, addressed from `ip`.
    fn range_request(uri: &str, ip: [u8; 4], range: &str) -> Request<Body> {
        let mut req = Request::builder()
            .uri(uri)
            .header(header::RANGE, range)
            .body(Body::empty())
            .expect("request");
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((ip, 40000))));
        req
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body")
            .to_vec()
    }

    fn header_of(response: &Response, name: header::HeaderName) -> Option<String> {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    #[tokio::test]
    async fn a_valid_capability_serves_immutable_private_bytes() {
        let fx = fixture();
        let payload = b"\x89PNG-not-really";
        let (canvas_id, asset_id) = seeded(&fx, "image/png", payload).await;
        let cap = CanvasCapabilities::mint(&canvas_id);

        let response = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap}/{canvas_id}/{asset_id}"
            )))
            .await
            .expect("route");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE).as_deref(),
            Some("image/png")
        );
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some(CACHE_CONTROL),
            "content-addressed bytes are immutable but capability-scoped: private, cacheable"
        );
        assert_eq!(body_bytes(response).await, payload);
    }

    /// The plan-named refusal matrix: an expired capability, a capability
    /// minted for a DIFFERENT canvas, and garbage all answer exactly like a
    /// missing asset (no-oracle), and a live capability does not open a
    /// sibling asset id that was never stored.
    #[tokio::test]
    async fn asset_route_refuses_expired_or_mismatched_cap() {
        let fx = fixture();
        let (canvas_a, asset_a) = seeded(&fx, "image/png", b"aaa").await;
        let (canvas_b, _asset_b) = seeded(&fx, "image/png", b"bbb").await;
        let cap_b = CanvasCapabilities::mint(&canvas_b);

        // Canvas B's capability must not read canvas A's asset — the scope
        // comes from the capability, and the URL must agree with it.
        let crossed = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap_b}/{canvas_a}/{asset_a}"
            )))
            .await
            .expect("route");
        assert_eq!(crossed.status(), StatusCode::NOT_FOUND);

        // Garbage capability.
        let garbage = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/not-a-capability/{canvas_a}/{asset_a}"
            )))
            .await
            .expect("route");
        assert_eq!(garbage.status(), StatusCode::NOT_FOUND);

        // A live capability, an asset id that parses but was never stored.
        let cap_a = CanvasCapabilities::mint(&canvas_a);
        let ghost = format!("{}.png", "0".repeat(64));
        let missing = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap_a}/{canvas_a}/{ghost}"
            )))
            .await
            .expect("route");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        // The same capability, expired: byte-identical refusal.
        let ok = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap_a}/{canvas_a}/{asset_a}"
            )))
            .await
            .expect("route");
        assert_eq!(ok.status(), StatusCode::OK, "control: live cap serves");
        CanvasCapabilities::expire_for_test(&canvas_a);
        let expired = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap_a}/{canvas_a}/{asset_a}"
            )))
            .await
            .expect("route");
        assert_eq!(expired.status(), StatusCode::NOT_FOUND);
    }

    /// The XSS boundary: an HTML asset comes back as `text/plain`, source
    /// intact — a capability URL opened directly must never become a
    /// same-origin HTML document (HTML renders only inside the Panel's
    /// sandboxed iframe srcdoc).
    #[tokio::test]
    async fn html_asset_is_served_as_plain_text() {
        let fx = fixture();
        let html = b"<html><script>fetch('/ws')</script></html>";
        let (canvas_id, asset_id) = seeded(&fx, "text/html", html).await;
        let cap = CanvasCapabilities::mint(&canvas_id);

        let response = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap}/{canvas_id}/{asset_id}"
            )))
            .await
            .expect("route");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE).as_deref(),
            Some("text/plain; charset=utf-8"),
            "text/html must be downgraded to plain text on this route"
        );
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some(CACHE_CONTROL)
        );
        assert_eq!(body_bytes(response).await, html, "the bytes stay intact");
    }

    /// SVG keeps its image type (an `<image href>` needs it) but a direct
    /// open lands in an opaque, script-less sandbox via the document CSP.
    #[tokio::test]
    async fn an_svg_asset_keeps_its_type_but_is_sandboxed() {
        let fx = fixture();
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>";
        let (canvas_id, asset_id) = seeded(&fx, "image/svg+xml", svg).await;
        let cap = CanvasCapabilities::mint(&canvas_id);

        let response = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap}/{canvas_id}/{asset_id}"
            )))
            .await
            .expect("route");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE).as_deref(),
            Some("image/svg+xml")
        );
        assert_eq!(
            header_of(&response, header::CONTENT_SECURITY_POLICY).as_deref(),
            Some(ARTIFACT_DOCUMENT_CSP),
            "a direct open must land in an opaque, script-less sandbox"
        );
    }

    #[tokio::test]
    async fn a_full_read_advertises_range_support() {
        let fx = fixture();
        let payload = b"\x89PNG-not-really";
        let (canvas_id, asset_id) = seeded(&fx, "image/png", payload).await;
        let cap = CanvasCapabilities::mint(&canvas_id);

        let response = fx
            .app
            .clone()
            .oneshot(request(&format!(
                "/canvas-asset/{cap}/{canvas_id}/{asset_id}"
            )))
            .await
            .expect("route");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::ACCEPT_RANGES).as_deref(),
            Some("bytes")
        );
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some(CACHE_CONTROL)
        );
    }

    #[tokio::test]
    async fn a_satisfiable_range_returns_exactly_that_slice() {
        let fx = fixture();
        let payload = b"0123456789ABCDEFGHIJ";
        let (canvas_id, asset_id) = seeded(&fx, "image/png", payload).await;
        let cap = CanvasCapabilities::mint(&canvas_id);

        let response = fx
            .app
            .clone()
            .oneshot(range_request(
                &format!("/canvas-asset/{cap}/{canvas_id}/{asset_id}"),
                LOOPBACK,
                "bytes=10-19",
            ))
            .await
            .expect("route");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_of(&response, header::CONTENT_RANGE).as_deref(),
            Some(format!("bytes 10-19/{}", payload.len()).as_str())
        );
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some(CACHE_CONTROL)
        );
        let body = body_bytes(response).await;
        assert_eq!(body.len(), 10);
        assert_eq!(body, &payload[10..20]);
    }

    #[tokio::test]
    async fn an_unsatisfiable_range_is_416_with_the_total() {
        let fx = fixture();
        let payload = b"short payload";
        let (canvas_id, asset_id) = seeded(&fx, "image/png", payload).await;
        let cap = CanvasCapabilities::mint(&canvas_id);

        let response = fx
            .app
            .clone()
            .oneshot(range_request(
                &format!("/canvas-asset/{cap}/{canvas_id}/{asset_id}"),
                LOOPBACK,
                "bytes=999999999-",
            ))
            .await
            .expect("route");
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            header_of(&response, header::CONTENT_RANGE).as_deref(),
            Some(format!("bytes */{}", payload.len()).as_str())
        );
        assert_eq!(
            header_of(&response, header::ACCEPT_RANGES).as_deref(),
            Some("bytes"),
            "a refusal must still say ranges are supported — otherwise a client \
             that asked badly once concludes the route cannot seek at all"
        );
    }

    #[tokio::test]
    async fn an_svg_partial_response_keeps_the_document_csp() {
        let fx = fixture();
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>";
        let (canvas_id, asset_id) = seeded(&fx, "image/svg+xml", svg).await;
        let cap = CanvasCapabilities::mint(&canvas_id);

        let response = fx
            .app
            .clone()
            .oneshot(range_request(
                &format!("/canvas-asset/{cap}/{canvas_id}/{asset_id}"),
                LOOPBACK,
                "bytes=0-4",
            ))
            .await
            .expect("route");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_of(&response, header::CONTENT_SECURITY_POLICY).as_deref(),
            Some(ARTIFACT_DOCUMENT_CSP)
        );
    }

    /// A range must never be the reason a byte is reachable. The capability
    /// gate runs first; a ranged request against a forged or mismatched
    /// capability is an ordinary not-found, not a 206 and not a 416.
    #[tokio::test]
    async fn a_range_does_not_bypass_the_capability_gate() {
        let fx = fixture();
        let (canvas_a, asset_a) = seeded(&fx, "image/png", b"aaaaaaaaaaaaaaaaaaaa").await;
        let (canvas_b, _asset_b) = seeded(&fx, "image/png", b"bbbbbbbbbbbbbbbbbbbb").await;
        let cap_b = CanvasCapabilities::mint(&canvas_b);

        // A capability minted for a DIFFERENT canvas.
        let mismatched = fx
            .app
            .clone()
            .oneshot(range_request(
                &format!("/canvas-asset/{cap_b}/{canvas_a}/{asset_a}"),
                LOOPBACK,
                "bytes=0-9",
            ))
            .await
            .expect("route");
        assert_eq!(mismatched.status(), StatusCode::NOT_FOUND);
        assert!(
            header_of(&mismatched, header::CONTENT_RANGE).is_none(),
            "a 416 would leak that the asset exists and how big it is"
        );

        // A garbage capability.
        let garbage = fx
            .app
            .clone()
            .oneshot(range_request(
                &format!("/canvas-asset/not-a-capability/{canvas_a}/{asset_a}"),
                LOOPBACK,
                "bytes=0-9",
            ))
            .await
            .expect("route");
        assert_eq!(garbage.status(), StatusCode::NOT_FOUND);
        assert!(
            header_of(&garbage, header::CONTENT_RANGE).is_none(),
            "a 416 would leak that the asset exists and how big it is"
        );
    }

    /// The wide Range bucket must never be a way to read more WHOLE assets.
    ///
    /// `bytes=0-` returns the entire body as a 206, so if the bucket were
    /// picked by "did this request carry a `Range`" — a thing the caller
    /// decides — one added header would lift a scraper from
    /// `CANVAS_ASSET_READS_PER_MINUTE` to `CANVAS_RANGE_READS_PER_MINUTE`.
    /// Both halves are asserted here: a full read dressed as a range is
    /// refused once the narrow bucket closes, and the genuine partial read
    /// the wide bucket exists for still goes through.
    #[tokio::test]
    async fn a_range_header_cannot_buy_more_whole_asset_reads() {
        let fx = fixture();
        let payload = b"0123456789ABCDEFGHIJ";
        let (canvas_id, asset_id) = seeded(&fx, "image/png", payload).await;
        let cap = CanvasCapabilities::mint(&canvas_id);
        let uri = format!("/canvas-asset/{cap}/{canvas_id}/{asset_id}");

        let mut limited = None;
        for _ in 0..=CANVAS_ASSET_READS_PER_MINUTE {
            let response = fx
                .app
                .clone()
                .oneshot(range_request(&uri, REMOTE, "bytes=0-"))
                .await
                .expect("response");
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                limited = Some(response);
                break;
            }
        }
        let limited =
            limited.expect("the narrow bucket must close on `bytes=0-` full reads eventually");
        assert!(header_of(&limited, header::RETRY_AFTER).is_some());

        let near_total = fx
            .app
            .clone()
            .oneshot(range_request(&uri, REMOTE, "bytes=1-"))
            .await
            .expect("response");
        assert_eq!(
            near_total.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "`bytes=1-` needs no knowledge of the size and returns every byte but the \
             first — a complete usable copy. An exact-coverage predicate lets it through, \
             which is a one-header bypass of this whole bucket."
        );

        // The narrow bucket is closed; a genuinely partial read must still
        // succeed — not throttling a media scrub is the entire reason the
        // wide bucket exists.
        let partial = fx
            .app
            .clone()
            .oneshot(range_request(&uri, REMOTE, "bytes=10-19"))
            .await
            .expect("response");
        assert_eq!(
            partial.status(),
            StatusCode::PARTIAL_CONTENT,
            "a real partial read must still be served once the narrow bucket has closed — \
             not throttling a media scrub is the entire reason the wide bucket exists"
        );
    }
}
