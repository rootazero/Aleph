//! HTTP-level middleware that strips the query string from incoming requests.
//!
//! Why: browsers load the Panel with credentials in the URL (`?bt=...`, legacy
//! `?token=...`). If any downstream service, access log, or future tracing layer
//! records the request URI, we must ensure those credentials are not present.
//! This layer removes the query component before the request reaches static-file
//! serving, the control-plane SPA fallback, or any later middleware.
//!
//! Note: the gateway's WebSocket endpoint (`/ws`) does **not** authenticate from
//! the URL; credentials are presented inside the JSON-RPC `connect` frame, so
//! stripping the query string there is safe.

use std::str::FromStr;
use std::task::{Context, Poll};

use axum::http::{Request, Uri};
use tower::{Layer, Service};

/// Tower layer that installs [`RedactQueryService`].
#[derive(Clone, Debug, Default)]
pub struct RedactQueryLayer;

impl RedactQueryLayer {
    /// Create a new layer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for RedactQueryLayer {
    type Service = RedactQueryService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RedactQueryService { inner }
    }
}

/// Service that redacts the query portion of the request URI before forwarding.
#[derive(Clone, Debug)]
pub struct RedactQueryService<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for RedactQueryService<S>
where
    S: Service<Request<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        if let Some(redacted) = redact_uri(req.uri()) {
            *req.uri_mut() = redacted;
        }
        self.inner.call(req)
    }
}

/// Return a URI with the query string removed.
///
/// Preserves scheme/authority if present and only touches `path_and_query`.
fn redact_uri(uri: &Uri) -> Option<Uri> {
    uri.query()?;

    let path = uri.path();
    let pq = axum::http::uri::PathAndQuery::from_str(path).ok()?;
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(pq);
    Uri::from_parts(parts).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::convert::Infallible;
    use tower::{Service, ServiceExt};

    #[tokio::test]
    async fn strips_bootstrap_ticket_from_query() {
        let mut svc =
            RedactQueryLayer::new().layer(tower::service_fn(|req: Request<Body>| async move {
                Ok::<_, Infallible>(req.uri().clone())
            }));

        let req = Request::builder()
            .uri("/?bt=super-secret-ticket")
            .body(Body::empty())
            .unwrap();
        let uri = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(uri.path(), "/");
        assert!(uri.query().is_none());
    }

    #[tokio::test]
    async fn strips_legacy_token_and_device_token() {
        let mut svc =
            RedactQueryLayer::new().layer(tower::service_fn(|req: Request<Body>| async move {
                Ok::<_, Infallible>(req.uri().clone())
            }));

        let req = Request::builder()
            .uri("/panel?token=legacy&device_token=per-device")
            .body(Body::empty())
            .unwrap();
        let uri = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(uri.path(), "/panel");
        assert!(uri.query().is_none());
    }

    #[tokio::test]
    async fn leaves_queryless_uri_unchanged() {
        let mut svc =
            RedactQueryLayer::new().layer(tower::service_fn(|req: Request<Body>| async move {
                Ok::<_, Infallible>(req.uri().clone())
            }));

        let req = Request::builder()
            .uri("/assets/main.js")
            .body(Body::empty())
            .unwrap();
        let uri = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(uri.path(), "/assets/main.js");
        assert!(uri.query().is_none());
    }
}
