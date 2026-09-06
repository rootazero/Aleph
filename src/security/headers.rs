//! Security response headers middleware.
//!
//! Implements a tower Layer that injects security headers on all HTTP responses.
//! Static asset paths are exempt from Cache-Control: no-store.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{self, HeaderName, HeaderValue, Request, Response};
use tower::{Layer, Service};

// --- Header values ---------------------------------------------------------

// `blob:` in script-src: the Panel's voice capture registers its AudioWorklet
// processor module from a same-origin-created blob URL (worklet module loads
// are governed by script-src). Only scripts already running on the page can
// mint blob URLs, and script-src already carries 'unsafe-inline', so this
// grants no capability an injected script wouldn't have.
const CSP_VALUE: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self'; connect-src 'self' ws: wss:; frame-ancestors 'none'; object-src 'none'; base-uri 'none'";
const HSTS_VALUE: &str = "max-age=31536000; includeSubDomains";
const X_CONTENT_TYPE_OPTIONS_VALUE: &str = "nosniff";
const X_FRAME_OPTIONS_VALUE: &str = "DENY";
const X_XSS_PROTECTION_VALUE: &str = "0";
const REFERRER_POLICY_VALUE: &str = "strict-origin-when-cross-origin";
// `microphone=(self)`: the Panel's immersive voice mode runs getUserMedia on
// this same origin — an empty allowlist would block it and dead-end voice chat.
// Camera/geolocation stay fully denied; cross-origin embedding is already
// impossible (frame-ancestors 'none' + X-Frame-Options DENY), so `self` here
// grants nothing to third parties.
const PERMISSIONS_POLICY_VALUE: &str = "camera=(), microphone=(self), geolocation=()";
const CACHE_CONTROL_NO_STORE_VALUE: &str = "no-store";

// --- Static asset detection ------------------------------------------------

/// Returns true if the path corresponds to a static asset that should be
/// served with normal caching (i.e., exempt from Cache-Control: no-store).
fn is_static_asset(path: &str) -> bool {
    if path.starts_with("/assets/") {
        return true;
    }
    let extensions = [".js", ".css", ".wasm", ".png", ".svg", ".ico", ".woff2"];
    extensions.iter().any(|ext| path.ends_with(ext))
}

/// Returns true if the CSP string contains a standalone `*` source on any
/// directive (the canonical `default-src *;` or `script-src *` form).
///
/// A CSP like `default-src *;frame-ancestors 'none'` was missed by the
/// previous substring detector that only matched `"* "`, `" *;"`, or a
/// trailing `" *"` — the asterisk directly followed by `;` is the most
/// natural spelling a careless refactor produces. Tokenize on whitespace
/// and treat the source list as the segment between a directive name and
/// the next `;`.
fn csp_contains_wildcard_source(csp: &str) -> bool {
    let mut source_iter = csp.split(';');
    source_iter.any(|directive| {
        // Drop the directive name and inspect only the source list, so a
        // legitimate `frame-ancestors *` (already a permissive policy) is
        // flagged via this same helper. Quoted `'none'` is not a wildcard.
        let mut tokens = directive.split_whitespace();
        match tokens.next() {
            Some(_) => tokens.any(|tok| tok == "*"),
            None => false,
        }
    })
}

// --- Layer -----------------------------------------------------------------

/// Tower layer that adds HTTP security headers to every response.
#[derive(Clone, Default)]
pub struct SecurityHeadersLayer;

impl SecurityHeadersLayer {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for SecurityHeadersLayer {
    type Service = SecurityHeadersService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SecurityHeadersService { inner }
    }
}

// --- Service ---------------------------------------------------------------

/// Tower service that wraps an inner service and injects security headers.
#[derive(Clone)]
pub struct SecurityHeadersService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for SecurityHeadersService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        // Capture the path before consuming the request
        let path = req.uri().path().to_owned();
        let static_asset = is_static_asset(&path);

        let future = self.inner.call(req);

        Box::pin(async move {
            let mut response: Response<Body> = future.await?;
            inject_security_headers(response.headers_mut(), static_asset);
            Ok(response)
        })
    }
}

// --- Header injection ------------------------------------------------------

/// Injects all security headers into the provided header map.
fn inject_security_headers(headers: &mut http::HeaderMap, is_static: bool) {
    // Content-Security-Policy is the one header a handler may set for itself,
    // and its own value wins. `HeaderMap::insert` *replaces*, so applying
    // [`CSP_VALUE`] unconditionally here — after the handler has already run —
    // would silently downgrade a response that deliberately chose a stricter
    // policy. The artifact byte route serves session-generated HTML and SVG
    // under `default-src 'none'; ...; sandbox`; handing those documents the
    // Panel policy instead (`script-src 'self' 'unsafe-inline'`, same origin)
    // would give them script capability on the gateway's own origin.
    //
    // Only tightening is possible: nothing that reaches this layer without its
    // own policy escapes [`CSP_VALUE`], and the strict policies that do set one
    // are strict supersets of these restrictions.
    if !headers.contains_key(http::header::CONTENT_SECURITY_POLICY) {
        if let Ok(value) = HeaderValue::from_str(CSP_VALUE) {
            headers.insert(http::header::CONTENT_SECURITY_POLICY, value);
        }
    } else if let Some(existing) = headers.get(http::header::CONTENT_SECURITY_POLICY) {
        // Defense-in-depth: a handler that emits a CSP less restrictive than
        // the default would land on the wire untouched. The layer cannot
        // *tighten* an already-set policy without breaking legitimate use
        // cases (the Panel artifact route sets a strict superset that drops
        // the default's `script-src 'unsafe-inline'`, for example), but it
        // can surface a permissive policy in the audit trail so a future
        // refactor that loosens a handler's CSP shows up at runtime rather
        // than silently. The two tells: a `default-src *` (or `*` anywhere)
        // and missing `frame-ancestors`.
        if let Ok(s) = existing.to_str() {
            // `csp_contains_wildcard_source` covers the common careless
            // spellings of a permissive policy: a bare `*` token (e.g.
            // `default-src *;frame-ancestors 'none'`) was missed by the
            // previous substring detector that only matched `"* "`, `" *;"`,
            // or trailing `" *"`. Tokenize on whitespace and look for any
            // directive whose source list contains the `*` source.
            let suspicious_wildcard = csp_contains_wildcard_source(s);
            let missing_frame_ancestors = !s.contains("frame-ancestors");
            if suspicious_wildcard || missing_frame_ancestors {
                tracing::warn!(
                    csp = %s,
                    suspicious_wildcard = suspicious_wildcard,
                    missing_frame_ancestors = missing_frame_ancestors,
                    "handler-supplied CSP observed; verify it is at least as restrictive as the layer default"
                );
            }
        }
    }

    let entries: &[(&str, &str)] = &[
        ("strict-transport-security", HSTS_VALUE),
        ("x-content-type-options", X_CONTENT_TYPE_OPTIONS_VALUE),
        ("x-frame-options", X_FRAME_OPTIONS_VALUE),
        ("x-xss-protection", X_XSS_PROTECTION_VALUE),
        ("referrer-policy", REFERRER_POLICY_VALUE),
        ("permissions-policy", PERMISSIONS_POLICY_VALUE),
    ];

    for (name, value) in entries {
        if let (Ok(header_name), Ok(header_value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(header_name, header_value);
        }
    }

    if !is_static {
        // SAFETY: "no-store" is a valid static header value
        let v = HeaderValue::from_static(CACHE_CONTROL_NO_STORE_VALUE);
        headers.insert(http::header::CACHE_CONTROL, v);
    }
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::{routing::get, Router};
    use tower::ServiceExt;

    /// The strict, per-response policy the artifact byte route serves
    /// session-generated HTML under. Duplicated here (rather than imported)
    /// so this test keeps guarding the *behaviour* even if that route moves.
    const STRICT_CSP: &str =
        "default-src 'none'; img-src data:; style-src 'unsafe-inline'; sandbox";

    fn test_router() -> Router {
        Router::new()
            .route("/api/test", get(|| async { "ok" }))
            .route("/assets/app.js", get(|| async { "js" }))
            .route(
                "/artifact/doc.html",
                get(|| async { ([("content-security-policy", STRICT_CSP)], "<p>hi</p>") }),
            )
            .layer(SecurityHeadersLayer::new())
    }

    #[tokio::test]
    async fn test_security_headers_present() {
        let app = test_router();

        let req = Request::builder()
            .uri("/api/test")
            .body(Body::empty())
            .unwrap();

        let response: Response<Body> = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let headers = response.headers();

        assert_eq!(
            headers
                .get("x-content-type-options")
                .and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("nosniff"),
        );
        assert_eq!(
            headers
                .get("x-frame-options")
                .and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("DENY"),
        );
        assert!(
            headers.contains_key("content-security-policy"),
            "CSP header should be present"
        );
        assert!(
            headers.contains_key("strict-transport-security"),
            "HSTS header should be present"
        );
        // The Panel's immersive voice mode captures the mic via getUserMedia on
        // the same origin the server serves it from. `microphone=()` (empty
        // allowlist) blocks even same-origin capture and kills voice chat
        // entirely; the policy must allow self while still denying third-party
        // frames (which are already impossible under frame-ancestors 'none').
        assert_eq!(
            headers
                .get("permissions-policy")
                .and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("camera=(), microphone=(self), geolocation=()"),
            "Permissions-Policy must allow same-origin microphone for voice mode"
        );
        assert_eq!(
            headers
                .get("cache-control")
                .and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("no-store"),
            "API paths should get Cache-Control: no-store"
        );
    }

    #[tokio::test]
    async fn handler_supplied_csp_is_not_downgraded() {
        let app = test_router();

        let req = Request::builder()
            .uri("/artifact/doc.html")
            .body(Body::empty())
            .unwrap();

        let response: Response<Body> = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // The artifact byte route serves HTML that a session produced — it must
        // land under `default-src 'none'; ...; sandbox`, not the Panel policy
        // that allows same-origin and inline script. This layer runs *after*
        // the handler, so an unconditional `insert` here would silently hand
        // untrusted documents script capability.
        assert_eq!(
            response
                .headers()
                .get("content-security-policy")
                .and_then(|v: &HeaderValue| v.to_str().ok()),
            Some(STRICT_CSP),
            "a handler's own CSP must survive the global layer"
        );
    }

    #[tokio::test]
    async fn test_static_assets_skip_no_store() {
        let app = test_router();

        let req = Request::builder()
            .uri("/assets/app.js")
            .body(Body::empty())
            .unwrap();

        let response: Response<Body> = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let headers = response.headers();

        // Static assets must NOT get Cache-Control: no-store
        let cache_control_no_store = headers
            .get("cache-control")
            .and_then(|v: &HeaderValue| v.to_str().ok())
            .map(|v| v.contains("no-store"))
            .unwrap_or(false);
        assert!(
            !cache_control_no_store,
            "Static assets should not get Cache-Control: no-store"
        );

        // But they must still get other security headers
        assert_eq!(
            headers
                .get("x-content-type-options")
                .and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("nosniff"),
        );
        assert!(
            headers.contains_key("content-security-policy"),
            "CSP header should be present on static assets"
        );
    }

    /// Regression for `severed-wire-2026-09-05-modules2 security I-1`: the
    /// wildcard detector used to miss the `default-src *;frame-ancestors
    /// 'none'` shape (asterisk directly followed by `;`). Tokenizing on
    /// whitespace + `;` covers every natural spelling.
    #[test]
    fn csp_contains_wildcard_source_catches_careless_permissive_spellings() {
        // The exact case the previous detector missed.
        assert!(csp_contains_wildcard_source(
            "default-src *;frame-ancestors 'none'"
        ));
        // The spellings the previous detector caught still trip.
        assert!(csp_contains_wildcard_source("default-src *"));
        assert!(csp_contains_wildcard_source(
            "default-src 'self' *;script-src 'self'"
        ));
        assert!(csp_contains_wildcard_source(
            "default-src 'self' *"
        ));
        // Strict policies must NOT trip.
        assert!(!csp_contains_wildcard_source(
            "default-src 'self';script-src 'self';frame-ancestors 'none'"
        ));
        // `'unsafe-inline'` / `'none'` quoted sources must not trip the
        // wildcard rule.
        assert!(!csp_contains_wildcard_source(
            "default-src 'none'; script-src 'unsafe-inline'"
        ));
    }
}
