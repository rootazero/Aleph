//! Middleware chain builder for the gateway JSON-RPC handler pipeline.

use std::sync::Arc;

use tower::{Layer, Service};

use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::middleware::{
    request_state::{set_global_registry, RequestStateRegistry},
    AuthLayer, HandlerService, MetricsLayer, RateLimitLayer, TraceLayer, ValidateLayer,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::rate_limiter::RateLimiter;

#[derive(Clone)]
pub struct MiddlewareChain {
    handlers: Arc<HandlerRegistry>,
    rate_limiter: Arc<RateLimiter>,
    state_registry: Arc<RequestStateRegistry>,
}

impl MiddlewareChain {
    pub fn new(handlers: Arc<HandlerRegistry>, rate_limiter: Arc<RateLimiter>) -> Self {
        let state_registry = Arc::new(RequestStateRegistry::new());
        set_global_registry(state_registry.clone());
        Self {
            handlers,
            rate_limiter,
            state_registry,
        }
    }

    pub fn state_registry(&self) -> Arc<RequestStateRegistry> {
        self.state_registry.clone()
    }

    pub async fn serve(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let handlers = self.handlers.clone();
        let rate_limiter = self.rate_limiter.clone();

        let trace = TraceLayer::new();
        let metrics = MetricsLayer::new_with_registry(self.state_registry.clone());
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
