//! `GET /artifact/{cap}/{id}/{filename}` — the artifact byte route.
//!
//! # This is a second ingress
//!
//! Every other way into this process is `/ws`, behind the `connect` handshake,
//! the device tier, the JSON-RPC middleware chain and the lane manager. A plain
//! HTTP `GET` inherits **none** of that, and it must not: a browser `<img src>`
//! cannot run a handshake. So each `/ws` guard is restated here explicitly
//! rather than assumed:
//!
//! | Guard | `/ws` | here |
//! |---|---|---|
//! | Plaintext-remote refusal | [`refuse_insecure_remote`] | same function, 426 |
//! | Cross-origin / DNS-rebinding | [`OriginPolicy`] | same policy, 403 |
//! | Rate limiting | shared `RateLimiter` | own limiter, own bucket, 429 |
//! | Authorization | `connect` + device tier | capability in the path |
//! | Scope | session per connection | session *resolved from* the capability |
//!
//! # Why the path, never the query
//!
//! The capability is a bearer secret. The Panel/control-plane fallback is
//! wrapped in `RedactQueryLayer` so tickets never reach a log, but this route is
//! registered at the top level and gets no such layer — a `?cap=` would survive
//! into any access log or error trace. It is a path segment, and nothing in this
//! module ever logs it.
//!
//! # Why the client cannot name a session
//!
//! The URL carries no session key. [`ArtifactCapabilities::session_for`]
//! resolves one server-side, and the store is asked only within it, so "session
//! A's capability reads session B's bytes" is not a check that could be
//! forgotten — there is no code path that could express it. After the read, the
//! capability is re-validated against the record's *own* declared session, so a
//! record that somehow sat in the wrong directory is still refused.
//!
//! # Why HTML gets a stricter policy
//!
//! An exported transcript is HTML this process generated from session content,
//! and an SVG is a live document too. Both are served under
//! [`ARTIFACT_DOCUMENT_CSP`] — `default-src 'none'` plus `sandbox`, which forces
//! an opaque origin and disables scripting — so even a hole in the exporter's
//! escaping cannot reach the gateway origin. `SecurityHeadersLayer` defers to a
//! response that already carries a policy; see `crate::security::headers`.

use std::net::{IpAddr, SocketAddr};

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use tracing::{debug, warn};

use crate::artifacts::{ArtifactError, ArtifactRecord, ArtifactStore};
use crate::gateway::origin_policy::OriginPolicy;
use crate::gateway::rate_limiter::{
    RateLimitConfig, RateLimitKey, RateLimitScope, RateLimiter, WindowConfig,
};
use crate::gateway::security::ArtifactCapabilities;
use crate::gateway::trusted_proxy::resolve_client;
use crate::sync_primitives::Arc;

use super::handler::refuse_insecure_remote;

/// Response policy for artifact bytes that a browser would treat as a document.
///
/// `default-src 'none'` starves it of every fetch, `img-src data:` keeps a
/// self-contained export's inline images working, `style-src 'unsafe-inline'`
/// keeps its `<style>` block working, and `sandbox` (empty allow-list) forces an
/// opaque origin with scripting disabled. Nothing here can reach the gateway
/// origin, read its storage, or call its RPCs.
pub const ARTIFACT_DOCUMENT_CSP: &str =
    "default-src 'none'; img-src data:; style-src 'unsafe-inline'; sandbox";

/// Per-minute byte reads allowed per remote IP.
///
/// One Panel opening a gallery issues a burst of `<img>` requests, so this sits
/// far above the RPC buckets; it exists to bound bulk scraping by someone who
/// obtained a capability, not to pace a UI. Loopback is exempt (see
/// [`RateLimitConfig::exempt_loopback`]), so the desktop App is unaffected.
const ARTIFACT_READS_PER_MINUTE: u32 = 240;

/// Percent-encode set for the RFC 5987 `filename*` parameter.
const ATTR_CHAR_ESCAPE_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

/// Everything the byte route needs — and nothing else.
///
/// Deliberately not `GatewaySharedState`: this route has no business reaching
/// the connection table, the handler registry or the event bus, and a narrow
/// state is what makes the guards testable without booting a gateway.
pub struct ArtifactRouteState {
    store: Arc<ArtifactStore>,
    /// Private limiter. Sharing the gateway's would let a page full of images
    /// exhaust the same bucket `chat.send` draws from and stall the
    /// conversation; a dedicated instance keeps byte reads and RPCs independent.
    /// `RateLimitConfig::max_entries` bounds it without a pruning task.
    rate_limiter: RateLimiter,
    origin_policy: Arc<OriginPolicy>,
    trusted_proxy_enabled: bool,
    trusted_proxy_ips: Vec<IpAddr>,
    allow_insecure_remote: bool,
    tls_enabled: bool,
}

impl ArtifactRouteState {
    /// Build the route state from the gateway's already-resolved policy.
    #[must_use]
    pub fn new(
        store: Arc<ArtifactStore>,
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
                    max_requests: ARTIFACT_READS_PER_MINUTE,
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
pub fn artifact_routes(state: Arc<ArtifactRouteState>) -> Router {
    Router::new()
        .route("/artifact/{cap}/{id}/{filename}", get(serve_artifact))
        .with_state(state)
}

/// Every refusal that must not distinguish "wrong capability" from "no such
/// artifact" — an attacker learns nothing about what exists.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "artifact not found").into_response()
}

/// Reject a path segment that could escape its directory or forge a header.
///
/// axum percent-decodes path parameters, so a `%2F` arrives here as a real
/// separator; this is the check that keeps it from becoming one.
fn is_safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.contains('/')
        && !segment.contains('\\')
        && !segment.contains("..")
        && !segment.chars().any(char::is_control)
}

/// The bare type/subtype of a MIME string, lowercased: `Text/HTML; charset=x`
/// becomes `text/html`. MIME types are case-insensitive and may carry
/// parameters, so every decision below is made on this normal form rather than
/// on the raw recorded string.
fn base_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Whether a MIME type names something a browser would parse as a document.
fn is_active_document(mime: &str) -> bool {
    matches!(
        base_mime(mime).as_str(),
        "text/html" | "application/xhtml+xml" | "image/svg+xml"
    )
}

/// Whether Aleph's own renderer produced these bytes.
///
/// HTML is only ever served `inline` when the answer is yes: those documents
/// come from [`crate::export`], which emits no `<script>` at all, so serving
/// them same-origin as the Panel is safe. HTML that merely *passed through*
/// Aleph — a file the user uploaded, bytes a tool returned — downloads instead,
/// whatever it claims to be. The `match` is exhaustive on purpose: a fifth
/// origin has to answer this question rather than inherit a default.
const fn is_aleph_rendered(origin: crate::artifacts::ArtifactOrigin) -> bool {
    use crate::artifacts::ArtifactOrigin as O;
    match origin {
        O::Export | O::Deliverable => true,
        O::Inbound | O::Outbound => false,
    }
}

/// `Content-Disposition` for a record.
///
/// Images render in place, and a document Aleph rendered itself — an exported
/// transcript or a published deliverable — exists to be opened, so all three
/// are `inline`; everything else downloads. The name is emitted twice — an
/// ASCII-quoted `filename` every client understands, plus the RFC 5987
/// `filename*` that carries the real UTF-8 name. Both are escaped: a quote or a
/// newline reaching a header value verbatim is a breakout.
fn content_disposition(record: &ArtifactRecord) -> String {
    let base = base_mime(&record.mime_type);
    let inline =
        base.starts_with("image/") || (is_aleph_rendered(record.origin) && base == "text/html");
    let kind = if inline { "inline" } else { "attachment" };

    let ascii: String = record
        .filename
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded = utf8_percent_encode(&record.filename, ATTR_CHAR_ESCAPE_SET);

    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// `GET /artifact/{cap}/{id}/{filename}`.
async fn serve_artifact(
    State(state): State<Arc<ArtifactRouteState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((cap, id, filename)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    // Resolve the effective client first: behind a trusted proxy the transport
    // peer is the proxy, and every guard below keys on the real client.
    let resolved = resolve_client(
        peer.ip(),
        &headers,
        state.trusted_proxy_enabled,
        &state.trusted_proxy_ips,
    );
    let client_ip = resolved.ip;
    let secure = state.tls_enabled || resolved.secure;

    // 1. Plaintext-remote refusal, identical to the `/ws` upgrade. A capability
    //    handed over an unencrypted leg is a capability handed to whoever is on
    //    the path.
    if refuse_insecure_remote(client_ip, secure, state.allow_insecure_remote) {
        warn!(
            client = %client_ip,
            "refused artifact read: insecure transport to a remote client"
        );
        return (
            StatusCode::UPGRADE_REQUIRED,
            "TLS required for remote artifact reads",
        )
            .into_response();
    }

    // 2. Cross-origin / DNS-rebinding guard. A `<img>` load sends no `Origin`
    //    and passes, exactly like a native client on `/ws`; a cross-origin
    //    `fetch` does send one and is held to the configured policy.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !state.origin_policy.is_allowed(origin, host) {
        warn!(client = %client_ip, "refused artifact read: disallowed origin");
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }

    // 3. Own rate limit, before any filesystem work.
    let key = RateLimitKey::new(&client_ip.to_string(), RateLimitScope::RpcHeavy);
    if let Err(e) = state.rate_limiter.check_and_record(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, e.retry_after_secs().to_string())],
            "artifact read rate limit exceeded",
        )
            .into_response();
    }

    // 4. Capability → session. Unknown or expired is indistinguishable from a
    //    missing artifact on the wire.
    let Some(session_key) = ArtifactCapabilities::session_for(&cap) else {
        return not_found();
    };

    // 5. Shape-check what the client supplied. `id` is additionally required to
    //    be a bare UUID by the store itself.
    if !is_safe_segment(&id) || !is_safe_segment(&filename) {
        return not_found();
    }

    // 6. Server-side resolution: the store maps (session, id) to a path. No
    //    client-supplied path ever reaches the filesystem.
    let (record, bytes) = match state.store.read(&session_key, &id).await {
        Ok(found) => found,
        Err(e @ (ArtifactError::NotFound(_) | ArtifactError::InvalidId(_))) => {
            // A miss is the designed answer to a probe, not an operator event.
            // At `warn` it would let anyone holding one capability flood the log
            // by walking ids.
            debug!(client = %client_ip, error = %e, "artifact read missed");
            return not_found();
        }
        Err(e) => {
            // Real trouble — an unreadable blob, a broken sidecar — still
            // surfaces, because that is a disk problem an operator can act on.
            warn!(client = %client_ip, error = %e, "artifact read failed");
            return not_found();
        }
    };

    // 7. Defence in depth: re-assert the capability against the session the
    //    *record* declares, and require the URL to name this record.
    if !ArtifactCapabilities::validate(&cap, &record.session_key) || record.filename != filename {
        return not_found();
    }

    // 8. Explicit, correct Content-Type from the record (`nosniff` is global).
    let content_type = HeaderValue::from_str(&record.mime_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let disposition = HeaderValue::from_str(&content_disposition(&record))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_DISPOSITION, disposition);

    if is_active_document(&record.mime_type) {
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
    use crate::artifacts::ArtifactOrigin;
    use axum::http::Request;
    use tempfile::TempDir;
    use tower::ServiceExt;

    const REMOTE: [u8; 4] = [203, 0, 113, 7];

    struct Fixture {
        _tmp: TempDir,
        app: Router,
        store: Arc<ArtifactStore>,
    }

    /// A route wired the way a loopback desktop App reaches it.
    fn fixture() -> Fixture {
        fixture_with(true, false)
    }

    fn fixture_with(allow_insecure_remote: bool, tls_enabled: bool) -> Fixture {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(ArtifactStore::new(tmp.path().to_path_buf()));
        let state = Arc::new(ArtifactRouteState::new(
            store.clone(),
            Arc::new(OriginPolicy::loopback_only()),
            false,
            Vec::new(),
            allow_insecure_remote,
            tls_enabled,
        ));
        Fixture {
            _tmp: tmp,
            app: artifact_routes(state),
            store,
        }
    }

    /// Session keys are process-global in the capability table, so every test
    /// uses its own.
    fn unique_session(tag: &str) -> String {
        format!("agent:route:{tag}-{}", uuid::Uuid::new_v4())
    }

    fn request(uri: &str, ip: [u8; 4]) -> Request<Body> {
        let mut req = Request::builder()
            .uri(uri)
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
    async fn a_valid_capability_serves_the_bytes() {
        let fx = fixture();
        let session = unique_session("ok");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG-BYTES",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let response = fx
            .app
            .clone()
            .oneshot(request(
                &format!("/artifact/{cap}/{}/chart.png", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE).as_deref(),
            Some("image/png")
        );
        assert_eq!(body_bytes(response).await, b"PNG-BYTES");
    }

    #[tokio::test]
    async fn an_unknown_capability_is_not_found() {
        let fx = fixture();
        let session = unique_session("unknown");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let _live = ArtifactCapabilities::mint(&session);

        let response = fx
            .app
            .oneshot(request(
                &format!("/artifact/not-a-capability/{}/chart.png", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_expired_capability_is_rejected() {
        let fx = fixture();
        let session = unique_session("expired");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);
        ArtifactCapabilities::expire_for_test(&session);

        let response = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/chart.png", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_capability_for_one_session_cannot_read_another() {
        let fx = fixture();
        let session_a = unique_session("a");
        let session_b = unique_session("b");
        let secret = fx
            .store
            .put(
                &session_b,
                None,
                ArtifactOrigin::Outbound,
                "secret.png",
                "image/png",
                b"SECRET",
            )
            .await
            .expect("put");
        fx.store
            .put(
                &session_a,
                None,
                ArtifactOrigin::Outbound,
                "mine.png",
                "image/png",
                b"MINE",
            )
            .await
            .expect("put");
        let cap_a = ArtifactCapabilities::mint(&session_a);

        // Session A's capability naming session B's artifact id: the route only
        // ever looks inside the session the capability resolves to, so this can
        // only miss.
        let response = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap_a}/{}/secret.png", secret.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_traversal_id_is_rejected() {
        let fx = fixture();
        let session = unique_session("traversal");
        let cap = ArtifactCapabilities::mint(&session);

        for id in [
            "..",
            "..%2F..%2Fetc%2Fpasswd",
            "%2E%2E%2Fsecret",
            "..%5Cwindows",
        ] {
            let response = fx
                .app
                .clone()
                .oneshot(request(
                    &format!("/artifact/{cap}/{id}/x.png"),
                    [127, 0, 0, 1],
                ))
                .await
                .expect("response");
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "id {id:?} must be refused"
            );
        }
    }

    #[tokio::test]
    async fn the_url_filename_must_name_the_record() {
        let fx = fixture();
        let session = unique_session("filename");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let response = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/other.png", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn images_are_inline_and_other_types_download() {
        let fx = fixture();
        let session = unique_session("disposition");
        let png = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let pdf = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Inbound,
                "report.pdf",
                "application/pdf",
                b"%PDF",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let image = fx
            .app
            .clone()
            .oneshot(request(
                &format!("/artifact/{cap}/{}/chart.png", png.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");
        assert_eq!(
            header_of(&image, header::CONTENT_TYPE).as_deref(),
            Some("image/png")
        );
        assert!(header_of(&image, header::CONTENT_DISPOSITION)
            .expect("disposition")
            .starts_with("inline;"));

        let doc = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/report.pdf", pdf.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");
        assert_eq!(
            header_of(&doc, header::CONTENT_TYPE).as_deref(),
            Some("application/pdf")
        );
        assert!(header_of(&doc, header::CONTENT_DISPOSITION)
            .expect("disposition")
            .starts_with("attachment;"));
    }

    #[tokio::test]
    async fn an_exported_document_is_inline_under_the_strict_policy() {
        let fx = fixture();
        let session = unique_session("export");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Export,
                "transcript.html",
                "text/html",
                b"<p>hi</p>",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let response = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/transcript.html", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(header_of(&response, header::CONTENT_DISPOSITION)
            .expect("disposition")
            .starts_with("inline;"));
        assert_eq!(
            header_of(&response, header::CONTENT_SECURITY_POLICY).as_deref(),
            Some(ARTIFACT_DOCUMENT_CSP),
            "session-generated HTML must be sandboxed"
        );
    }

    /// The whole point of a deliverable is that it opens. Serving it
    /// `attachment` — which is what a whitelist naming only `Export` did —
    /// downloads the report instead of showing it, and a CJK title made that
    /// silent: the ASCII fallback name degrades to underscores, so the only
    /// visible symptom was a file appearing in Downloads.
    #[tokio::test]
    async fn a_published_deliverable_opens_instead_of_downloading() {
        let fx = fixture();
        let session = unique_session("deliverable");
        let published = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Deliverable,
                "红色方块生成实验报告.html",
                "text/html",
                b"<p>report</p>",
            )
            .await
            .expect("put");
        let uploaded = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Inbound,
                "sent-to-me.html",
                "text/html",
                b"<p>not ours</p>",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let doc = fx
            .app
            .clone()
            .oneshot(request(
                &format!(
                    "/artifact/{cap}/{}/{}",
                    published.id,
                    utf8_percent_encode(&published.filename, ATTR_CHAR_ESCAPE_SET)
                ),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");
        let disposition = header_of(&doc, header::CONTENT_DISPOSITION).expect("disposition");
        assert!(
            disposition.starts_with("inline;"),
            "a deliverable must render, got {disposition}"
        );
        assert!(
            disposition.contains("filename*=UTF-8''"),
            "the real name still travels: {disposition}"
        );
        assert_eq!(
            header_of(&doc, header::CONTENT_SECURITY_POLICY).as_deref(),
            Some(ARTIFACT_DOCUMENT_CSP),
            "rendering inline is only safe under the sandbox policy"
        );

        let foreign = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/sent-to-me.html", uploaded.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");
        assert!(
            header_of(&foreign, header::CONTENT_DISPOSITION)
                .expect("disposition")
                .starts_with("attachment;"),
            "HTML Aleph did not render must keep downloading"
        );
    }

    #[tokio::test]
    async fn an_svg_is_served_under_the_strict_policy_too() {
        let fx = fixture();
        let session = unique_session("svg");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "diagram.svg",
                "image/svg+xml",
                b"<svg/>",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let response = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/diagram.svg", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        // An SVG navigated to directly is a scriptable document, so `inline`
        // alone would be a same-origin script vector.
        assert_eq!(
            header_of(&response, header::CONTENT_SECURITY_POLICY).as_deref(),
            Some(ARTIFACT_DOCUMENT_CSP)
        );
    }

    #[tokio::test]
    async fn a_png_carries_no_document_policy() {
        let fx = fixture();
        let session = unique_session("nopolicy");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let response = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/chart.png", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");

        assert_eq!(
            header_of(&response, header::CONTENT_SECURITY_POLICY),
            None,
            "the global layer supplies the Panel policy for non-documents"
        );
    }

    #[tokio::test]
    async fn a_plaintext_remote_read_is_refused() {
        let fx = fixture_with(false, false);
        let session = unique_session("insecure");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let response = fx
            .app
            .clone()
            .oneshot(request(
                &format!("/artifact/{cap}/{}/chart.png", record.id),
                REMOTE,
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);

        // The same read over loopback is fine — the refusal is about the leg,
        // not the capability.
        let local = fx
            .app
            .oneshot(request(
                &format!("/artifact/{cap}/{}/chart.png", record.id),
                [127, 0, 0, 1],
            ))
            .await
            .expect("response");
        assert_eq!(local.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_cross_origin_read_is_refused() {
        let fx = fixture();
        let session = unique_session("origin");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);

        let mut req = request(
            &format!("/artifact/{cap}/{}/chart.png", record.id),
            [127, 0, 0, 1],
        );
        req.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        req.headers_mut()
            .insert(header::HOST, HeaderValue::from_static("127.0.0.1:18790"));

        let response = fx.app.oneshot(req).await.expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn remote_reads_are_rate_limited() {
        let fx = fixture_with(true, false);
        let session = unique_session("ratelimit");
        let record = fx
            .store
            .put(
                &session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"PNG",
            )
            .await
            .expect("put");
        let cap = ArtifactCapabilities::mint(&session);
        let uri = format!("/artifact/{cap}/{}/chart.png", record.id);

        let mut limited = None;
        for _ in 0..=ARTIFACT_READS_PER_MINUTE {
            let response = fx
                .app
                .clone()
                .oneshot(request(&uri, REMOTE))
                .await
                .expect("response");
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                limited = Some(response);
                break;
            }
        }

        let response = limited.expect("the bucket must close");
        assert!(header_of(&response, header::RETRY_AFTER).is_some());
    }

    #[test]
    fn a_quote_in_a_filename_cannot_break_out_of_the_header() {
        let record = ArtifactRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: "agent:main:main".to_string(),
            run_id: None,
            origin: ArtifactOrigin::Inbound,
            filename: "evil\"; x=\"a\r\n.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size: 4,
            created_at: 0,
        };

        let value = content_disposition(&record);
        let quoted = value
            .split_once("filename=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(inner, _)| inner)
            .expect("quoted filename");

        assert!(!quoted.contains('"'), "{value}");
        assert!(!quoted.contains('\r') && !quoted.contains('\n'), "{value}");
        assert!(
            HeaderValue::from_str(&value).is_ok(),
            "must be a legal header value: {value}"
        );
    }

    #[test]
    fn a_non_ascii_filename_survives_as_rfc_5987() {
        let record = ArtifactRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: "agent:main:main".to_string(),
            run_id: None,
            origin: ArtifactOrigin::Inbound,
            filename: "报告.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size: 4,
            created_at: 0,
        };

        let value = content_disposition(&record);
        assert!(
            value.contains("filename*=UTF-8''%E6%8A%A5%E5%91%8A.pdf"),
            "{value}"
        );
        assert!(HeaderValue::from_str(&value).is_ok(), "{value}");
    }

    #[test]
    fn unsafe_segments_are_rejected() {
        assert!(is_safe_segment("a5f3c0d2-0000-4000-8000-000000000000"));
        assert!(is_safe_segment("report.pdf"));
        assert!(!is_safe_segment(""));
        assert!(!is_safe_segment(".."));
        assert!(!is_safe_segment("../etc/passwd"));
        assert!(!is_safe_segment("a/b"));
        assert!(!is_safe_segment("a\\b"));
        assert!(!is_safe_segment("a\rb"));
    }

    #[test]
    fn document_types_are_recognised_with_parameters() {
        assert!(is_active_document("text/html"));
        assert!(is_active_document("text/html; charset=utf-8"));
        assert!(is_active_document("Image/SVG+XML"));
        assert!(!is_active_document("image/png"));
        assert!(!is_active_document("text/plain"));
    }

    #[test]
    fn disposition_reads_the_mime_case_insensitively() {
        let record = |mime: &str, origin| ArtifactRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_key: "agent:main:main".to_string(),
            run_id: None,
            origin,
            filename: "x".to_string(),
            mime_type: mime.to_string(),
            size: 1,
            created_at: 0,
        };

        // An exporter or a tool that spells the type differently must not flip
        // an image into a download or a transcript into an attachment.
        assert!(
            content_disposition(&record("Image/PNG", ArtifactOrigin::Outbound))
                .starts_with("inline;")
        );
        assert!(
            content_disposition(&record("Text/HTML; charset=utf-8", ArtifactOrigin::Export))
                .starts_with("inline;")
        );
        // Only an *exported* document is inline; the same type from a tool
        // downloads instead of rendering on the gateway origin.
        assert!(
            content_disposition(&record("text/html", ArtifactOrigin::Outbound))
                .starts_with("attachment;")
        );
    }
}
