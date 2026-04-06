//! Rate limit middleware for the gateway JSON-RPC handler pipeline.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};
use tracing::warn;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::rate_limiter::{RateLimiter, RateLimitKey, scope_for_method};

#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Arc<RateLimiter>,
}

impl RateLimitLayer {
    pub fn new(limiter: Arc<RateLimiter>) -> Self {
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
    pub fn new(inner: S, limiter: Arc<RateLimiter>) -> Self {
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
        let identity = "rpc".to_string();
        let key = RateLimitKey::new(&identity, scope.clone());

        if let Err(e) = limiter.check_and_record(&key) {
            warn!(method = %method, scope = %scope, error = %e, "rate limited");
            let response = JsonRpcResponse::error_with_data(
                req.id.clone(),
                crate::gateway::protocol::RATE_LIMITED,
                e.to_string(),
                serde_json::json!({"retry_after_ms": 0}),
            );
            return Box::pin(async move { Ok(response) });
        }

        Box::pin(async move {
            inner_mut.call(req).await
        })
    }
}
