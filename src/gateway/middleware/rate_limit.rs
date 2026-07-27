//! Rate limit middleware for the gateway JSON-RPC handler pipeline.

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Layer, Service};
use tracing::warn;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::rate_limiter::{scope_for_method, RateLimitKey, RateLimitScope, RateLimiter};

#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Arc<RateLimiter>,
}

impl RateLimitLayer {
    #[must_use]
    pub const fn new(limiter: Arc<RateLimiter>) -> Self {
        Self { limiter }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.limiter.clone(),
        }
    }
}

#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: Arc<RateLimiter>,
}

impl<S> RateLimitService<S> {
    pub const fn new(inner: S, limiter: Arc<RateLimiter>) -> Self {
        Self { inner, limiter }
    }
}

impl<S> Service<JsonRpcRequest> for RateLimitService<S>
where
    S: Service<JsonRpcRequest, Response = JsonRpcResponse> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = JsonRpcResponse;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: JsonRpcRequest) -> Self::Future {
        let limiter = self.limiter.clone();
        let inner = self.inner.clone();
        let mut inner_mut = std::mem::replace(&mut self.inner, inner);
        let method = req.method.clone();

        let scope = scope_for_method(&method);
        // `connect` maps to the strict Auth window (with lockout), which the
        // WS dispatch loop enforces per client IP upstream. This layer pools
        // every remote caller into the one shared "rpc" identity, so letting
        // the Auth lockout trip here would let a single token-guessing client
        // lock every remote Panel out of the handshake (cross-client DoS).
        // Pool handshakes into the lockout-free default bucket instead — it
        // still bounds aggregate dispatch load.
        let scope = if matches!(scope, RateLimitScope::Auth) {
            RateLimitScope::RpcDefault
        } else {
            scope
        };
        // The dispatch loop scopes the caller's network position into a
        // task-local before calling the chain. A loopback caller (the desktop
        // App's local Panel) must flow through the limiter's own loopback
        // exemption — the old fixed identity "rpc" pooled EVERY caller,
        // local and remote, into one shared bucket and rate-limited the
        // local Panel's voice loop off the air.
        let identity = if crate::gateway::caller_identity::current_caller_is_loopback() {
            "127.0.0.1"
        } else {
            "rpc"
        };
        let key = RateLimitKey::new(identity, scope.clone());

        if let Err(e) = limiter.check_and_record(&key) {
            warn!(method = %method, scope = %scope, error = %e, "rate limited");
            // Real backoff hint (the old hardcoded 0 told clients to hammer).
            let retry_after_ms = e.retry_after_secs() * 1000;
            let response = JsonRpcResponse::error_with_data(
                req.id.clone(),
                crate::gateway::protocol::RATE_LIMITED,
                e.to_string(),
                serde_json::json!({"retry_after_ms": retry_after_ms}),
            );
            return Box::pin(async move { Ok(response) });
        }

        Box::pin(async move { inner_mut.call(req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_IS_LOOPBACK;
    use crate::gateway::rate_limiter::RateLimitConfig;

    /// Terminal service that always succeeds.
    #[derive(Clone)]
    struct Ok200;
    impl Service<JsonRpcRequest> for Ok200 {
        type Response = JsonRpcResponse;
        type Error = std::convert::Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: JsonRpcRequest) -> Self::Future {
            let resp = JsonRpcResponse::success(req.id, serde_json::json!({}));
            Box::pin(async move { Ok(resp) })
        }
    }

    fn tight_limiter() -> Arc<RateLimiter> {
        // rpc_heavy: 2 per second — exhausted on the 3rd chat.send.
        let mut config = RateLimitConfig::default();
        config.rpc_heavy.max_requests = 2;
        config.rpc_heavy.window_secs = 1;
        Arc::new(RateLimiter::new(config))
    }

    fn send_req() -> JsonRpcRequest {
        JsonRpcRequest::with_id("chat.send", None, serde_json::json!(1))
    }

    #[tokio::test]
    async fn loopback_caller_is_exempt_from_the_shared_bucket() {
        let mut svc = RateLimitService::new(Ok200, tight_limiter());
        // Scoped exactly like the WS dispatch loop scopes a local Panel.
        CALLER_IS_LOOPBACK
            .scope(true, async {
                for i in 0..20 {
                    let resp = svc.call(send_req()).await.unwrap();
                    assert!(
                        resp.error.is_none(),
                        "loopback chat.send {i} must not be rate limited"
                    );
                }
            })
            .await;
    }

    #[tokio::test]
    async fn remote_caller_still_hits_the_limit_with_a_real_retry_hint() {
        let mut svc = RateLimitService::new(Ok200, tight_limiter());
        CALLER_IS_LOOPBACK
            .scope(false, async {
                for _ in 0..2 {
                    let resp = svc.call(send_req()).await.unwrap();
                    assert!(resp.error.is_none());
                }
                let resp = svc.call(send_req()).await.unwrap();
                let err = resp.error.expect("3rd chat.send in 1s should be limited");
                let retry = err
                    .data
                    .as_ref()
                    .and_then(|d| d.get("retry_after_ms"))
                    .and_then(serde_json::Value::as_u64)
                    .expect("retry_after_ms present");
                assert!(
                    retry > 0,
                    "retry hint must be real, not the old hardcoded 0"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn connect_uses_the_default_bucket_here_not_the_pooled_auth_lockout() {
        // A hair-trigger auth window: 1 attempt/min with a 5-min lockout. If
        // this layer applied the Auth scope to its shared "rpc" identity, the
        // 2nd remote connect would lock every remote caller out.
        let mut config = RateLimitConfig::default();
        config.auth.max_requests = 1;
        config.auth.window_secs = 60;
        config.auth.lockout_secs = Some(300);
        let mut svc = RateLimitService::new(Ok200, Arc::new(RateLimiter::new(config)));
        CALLER_IS_LOOPBACK
            .scope(false, async {
                for i in 0..5 {
                    let req = JsonRpcRequest::with_id("connect", None, serde_json::json!(i));
                    let resp = svc.call(req).await.unwrap();
                    assert!(
                        resp.error.is_none(),
                        "connect {i} must use the lockout-free default bucket here"
                    );
                }
            })
            .await;
    }

    #[tokio::test]
    async fn outside_any_scope_behaves_like_remote() {
        // Non-gateway callers (no task-local scope) keep the shared bucket.
        let mut svc = RateLimitService::new(Ok200, tight_limiter());
        for _ in 0..2 {
            let resp = svc.call(send_req()).await.unwrap();
            assert!(resp.error.is_none());
        }
        let resp = svc.call(send_req()).await.unwrap();
        assert!(resp.error.is_some(), "unscoped callers are not exempt");
    }
}
