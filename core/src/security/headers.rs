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

const CSP_VALUE: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self'; connect-src 'self' ws: wss:; frame-ancestors 'none'; object-src 'none'; base-uri 'none'";
const HSTS_VALUE: &str = "max-age=31536000; includeSubDomains";
const X_CONTENT_TYPE_OPTIONS_VALUE: &str = "nosniff";
const X_FRAME_OPTIONS_VALUE: &str = "DENY";
const X_XSS_PROTECTION_VALUE: &str = "0";
const REFERRER_POLICY_VALUE: &str = "strict-origin-when-cross-origin";
const PERMISSIONS_POLICY_VALUE: &str = "camera=(), microphone=(), geolocation=()";
const CACHE_CONTROL_NO_STORE_VALUE: &str = "no-store";

// --- Static asset detection ------------------------------------------------

/// Returns true if the path corresponds to a static asset that should be
/// served with normal caching (i.e., exempt from Cache-Control: no-store).
fn is_static_asset(path: &str) -> bool {
    if path.contains("/assets/") {
        return true;
    }
    let extensions = [".js", ".css", ".wasm", ".png", ".svg", ".ico", ".woff2"];
    extensions.iter().any(|ext| path.ends_with(ext))
}

// --- Layer -----------------------------------------------------------------

/// Tower layer that adds HTTP security headers to every response.
#[derive(Clone, Default)]
pub struct SecurityHeadersLayer;

impl SecurityHeadersLayer {
    pub fn new() -> Self {
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
    let entries: &[(&str, &str)] = &[
        ("content-security-policy", CSP_VALUE),
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
    use axum::{Router, routing::get};
    use axum::http::StatusCode;
    use tower::ServiceExt;

    fn test_router() -> Router {
        Router::new()
            .route("/api/test", get(|| async { "ok" }))
            .route("/assets/app.js", get(|| async { "js" }))
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
            headers.get("x-content-type-options").and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("nosniff"),
        );
        assert_eq!(
            headers.get("x-frame-options").and_then(|v: &HeaderValue| v.to_str().ok()),
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
        assert_eq!(
            headers.get("cache-control").and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("no-store"),
            "API paths should get Cache-Control: no-store"
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
            headers.get("x-content-type-options").and_then(|v: &HeaderValue| v.to_str().ok()),
            Some("nosniff"),
        );
        assert!(
            headers.contains_key("content-security-policy"),
            "CSP header should be present on static assets"
        );
    }
}
