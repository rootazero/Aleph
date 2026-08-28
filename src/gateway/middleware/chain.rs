//! Middleware chain builder for the gateway JSON-RPC handler pipeline.

use crate::sync_primitives::Arc;

use tower::{Layer, Service};

use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::middleware::{
    latency::global_latency_or_init,
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
    trace: TraceLayer,
    metrics: MetricsLayer,
    auth: AuthLayer,
    validate: ValidateLayer,
}

impl MiddlewareChain {
    #[must_use]
    pub fn new(handlers: Arc<HandlerRegistry>, rate_limiter: Arc<RateLimiter>) -> Self {
        let state_registry = Arc::new(RequestStateRegistry::new());
        set_global_registry(state_registry.clone());
        let metrics = MetricsLayer::new_with_registry(state_registry.clone())
            .with_latency(global_latency_or_init());
        Self {
            handlers,
            rate_limiter,
            state_registry,
            trace: TraceLayer::new(),
            metrics,
            auth: AuthLayer::new(),
            validate: ValidateLayer::new(),
        }
    }

    #[must_use]
    pub fn state_registry(&self) -> Arc<RequestStateRegistry> {
        self.state_registry.clone()
    }

    pub async fn serve(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let rate_limit = RateLimitLayer::new(self.rate_limiter.clone());
        let terminal: HandlerService<()> = HandlerService::new(self.handlers.clone());

        let traced = self.trace.layer(terminal);
        let metered = self.metrics.layer(traced);
        let authed = self.auth.layer(metered);
        let rate_limited = rate_limit.layer(authed);
        let validated = self.validate.layer(rate_limited);

        let mut svc = validated;
        // Capture the request id BEFORE moving `request` into the service
        // call: JSON-RPC correlation is by `id`, and answering a chain-level
        // error with `id: null` leaves the client's pending promise for that
        // request hanging until a client-side timeout (the Panel has several
        // call sites with no timeout).
        let request_id = request.id.clone();
        match svc.call(request).await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(error = ?e, "middleware chain error");
                JsonRpcResponse::error(
                    request_id,
                    crate::gateway::protocol::INTERNAL_ERROR,
                    "Middleware chain error",
                )
            }
        }
    }
}
