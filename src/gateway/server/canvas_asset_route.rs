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

    // 3. Own rate limit, before any filesystem work.
    let key = RateLimitKey::new(&client_ip.to_string(), RateLimitScope::RpcHeavy);
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

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_CONTROL),
        );

    // SVG stays a document type for `<image href>`'s sake; the CSP is what
    // neuters a direct open (opaque origin, no scripting).
    if mime.eq_ignore_ascii_case("image/svg+xml") {
        response = response.header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(ARTIFACT_DOCUMENT_CSP),
        );
    }

    response
        .body(Body::from(bytes))
        .unwrap_or_else(|_| not_found())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tempfile::TempDir;
    use tower::ServiceExt;

    const LOOPBACK: [u8; 4] = [127, 0, 0, 1];

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
}
