//! Metrics middleware for the gateway JSON-RPC handler pipeline.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};
use uuid::Uuid;

use crate::gateway::middleware::request_state::{RequestState, RequestStateRegistry};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

#[derive(Clone)]
pub struct MetricsLayer {
    requests_total: Arc<AtomicU64>,
    requests_in_flight: Arc<AtomicU64>,
    state_registry: Arc<RequestStateRegistry>,
    processing_threshold: u64,
}

impl MetricsLayer {
    pub fn new() -> Self {
        Self {
            requests_total: Arc::new(AtomicU64::new(0)),
            requests_in_flight: Arc::new(AtomicU64::new(0)),
            state_registry: Arc::new(RequestStateRegistry::new()),
            processing_threshold: 100,
        }
    }

    pub fn new_with_registry(state_registry: Arc<RequestStateRegistry>) -> Self {
        Self {
            requests_total: Arc::new(AtomicU64::new(0)),
            requests_in_flight: Arc::new(AtomicU64::new(0)),
            state_registry,
            processing_threshold: 100,
        }
    }

    pub fn with_threshold(mut self, threshold: u64) -> Self {
        self.processing_threshold = threshold;
        self
    }

    pub fn requests_total(&self) -> u64 {
        self.requests_total.load(Ordering::SeqCst)
    }

    pub fn requests_in_flight(&self) -> u64 {
        self.requests_in_flight.load(Ordering::SeqCst)
    }

    pub fn state_registry(&self) -> Arc<RequestStateRegistry> {
        self.state_registry.clone()
    }

    pub fn processing_count(&self) -> u64 {
        self.state_registry.count(RequestState::Processing)
    }

    pub fn pending_count(&self) -> u64 {
        self.state_registry.count(RequestState::Pending)
    }

    pub fn validating_count(&self) -> u64 {
        self.state_registry.count(RequestState::Validating)
    }

    pub fn awaiting_count(&self) -> u64 {
        self.state_registry.count(RequestState::AwaitingResponse)
    }
}

impl Default for MetricsLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetricsService {
            inner,
            requests_total: self.requests_total.clone(),
            requests_in_flight: self.requests_in_flight.clone(),
            state_registry: self.state_registry.clone(),
            processing_threshold: self.processing_threshold,
        }
    }
}

#[derive(Clone)]
pub struct MetricsService<S> {
    inner: S,
    requests_total: Arc<AtomicU64>,
    requests_in_flight: Arc<AtomicU64>,
    state_registry: Arc<RequestStateRegistry>,
    processing_threshold: u64,
}

impl<S> MetricsService<S> {
    pub fn new(
        inner: S,
        requests_total: Arc<AtomicU64>,
        requests_in_flight: Arc<AtomicU64>,
        state_registry: Arc<RequestStateRegistry>,
        processing_threshold: u64,
    ) -> Self {
        Self {
            inner,
            requests_total,
            requests_in_flight,
            state_registry,
            processing_threshold,
        }
    }
}

impl<S> Service<JsonRpcRequest> for MetricsService<S>
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
        let inner = self.inner.clone();
        let mut inner_mut = std::mem::replace(&mut self.inner, inner);
        let requests_total = self.requests_total.clone();
        let requests_in_flight = self.requests_in_flight.clone();
        let state_registry = self.state_registry.clone();
        let processing_threshold = self.processing_threshold;

        let request_id = Uuid::new_v4();
        let method = req.method.clone();
        let start = std::time::Instant::now();

        // Check backpressure - if processing count exceeds threshold, return 429
        if state_registry.count(RequestState::Processing) >= processing_threshold {
            requests_total.fetch_add(1, Ordering::SeqCst);
            tracing::warn!(
                method = %method,
                processing_count = state_registry.count(RequestState::Processing),
                threshold = processing_threshold,
                "backpressure triggered"
            );
            return Box::pin(async move {
                Ok(JsonRpcResponse::error(
                    None,
                    crate::gateway::protocol::RATE_LIMITED,
                    "Processing capacity exceeded, try again later",
                ))
            });
        }

        // Register request and transition through states
        // These transitions should always succeed since we just inserted the request,
        // but we check and warn in case of programming errors.
        state_registry.insert(request_id);
        if let Some(Err(e)) = state_registry.transition(request_id, RequestState::Validating) {
            tracing::error!(request_id = %request_id, error = ?e, "failed to transition to Validating");
        }

        // Transition to Processing before calling handler
        if let Some(Err(e)) = state_registry.transition(request_id, RequestState::Processing) {
            tracing::error!(request_id = %request_id, error = ?e, "failed to transition to Processing");
        }

        requests_in_flight.fetch_add(1, Ordering::SeqCst);

        Box::pin(async move {
            let result = inner_mut.call(req).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            requests_in_flight.fetch_sub(1, Ordering::SeqCst);

            match &result {
                Ok(resp) => {
                    if resp.is_error() {
                        state_registry.fail(request_id);
                        tracing::debug!(
                            method = %method,
                            elapsed_ms = %elapsed_ms,
                            status = "error",
                            "rpc request"
                        );
                    } else {
                        state_registry.complete(request_id);
                        tracing::debug!(
                            method = %method,
                            elapsed_ms = %elapsed_ms,
                            status = "ok",
                            "rpc request"
                        );
                    }
                }
                Err(_) => {
                    state_registry.fail(request_id);
                    tracing::debug!(
                        method = %method,
                        elapsed_ms = %elapsed_ms,
                        status = "error",
                        "rpc request"
                    );
                }
            }

            requests_total.fetch_add(1, Ordering::SeqCst);
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcResponse;

    #[derive(Clone)]
    struct MockService;

    impl Service<JsonRpcRequest> for MockService {
        type Response = JsonRpcResponse;
        type Error = std::convert::Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<JsonRpcResponse, std::convert::Infallible>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: JsonRpcRequest) -> Self::Future {
            Box::pin(async move {
                Ok(JsonRpcResponse::success(None, serde_json::Value::Null))
            })
        }
    }

    #[tokio::test]
    async fn test_metrics_increments_counter() {
        let layer = MetricsLayer::new();
        assert_eq!(layer.requests_total(), 0);

        let traced = layer.layer(MockService);
        let mut svc = traced;

        let waker = futures_util::task::noop_waker_ref();
        let _ = svc.poll_ready(&mut Context::from_waker(waker));
        let _ = svc.call(JsonRpcRequest::notification("test", None)).await;

        assert_eq!(layer.requests_total(), 1);
        assert_eq!(layer.requests_in_flight(), 0);
    }

    #[tokio::test]
    async fn test_state_registry_tracks_requests() {
        let layer = MetricsLayer::new();
        let registry = layer.state_registry();

        let traced = layer.layer(MockService);
        let mut svc = traced;

        let waker = futures_util::task::noop_waker_ref();
        let _ = svc.poll_ready(&mut Context::from_waker(waker));
        let _ = svc.call(JsonRpcRequest::notification("test", None)).await;

        assert_eq!(registry.completed(), 1);
        assert_eq!(registry.in_flight(), 0);
    }
}
