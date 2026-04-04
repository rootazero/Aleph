//! Middleware chain builder for the gateway JSON-RPC handler pipeline.

use std::sync::Arc;

use tower::{Layer, Service};

use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::middleware::{
    AuthLayer, HandlerService, MetricsLayer, RateLimitLayer, TraceLayer, ValidateLayer,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::rate_limiter::RateLimiter;

#[derive(Clone)]
pub struct MiddlewareChain {
    handlers: Arc<HandlerRegistry>,
    rate_limiter: Arc<RateLimiter>,
}

impl MiddlewareChain {
    pub fn new(handlers: Arc<HandlerRegistry>, rate_limiter: Arc<RateLimiter>) -> Self {
        Self { handlers, rate_limiter }
    }

    pub async fn serve(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let handlers = self.handlers.clone();
        let rate_limiter = self.rate_limiter.clone();

        let trace = TraceLayer::new();
        let metrics = MetricsLayer::new();
        let auth = AuthLayer::new();
        let validate = ValidateLayer::new();
        let rate_limit = RateLimitLayer::new(rate_limiter);

        let terminal: HandlerService<()> = HandlerService::new(handlers);
        let traced = trace.layer(terminal);
        let metered = metrics.layer(traced);
        let authed = auth.layer(metered);
        let rate_limited = rate_limit.layer(authed);
        let validated = validate.layer(rate_limited);

        let mut svc = validated;
        match svc.call(request).await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(error = ?e, "middleware chain error");
                JsonRpcResponse::error(
                    None,
                    crate::gateway::protocol::INTERNAL_ERROR,
                    "Middleware chain error",
                )
            }
        }
    }
}
