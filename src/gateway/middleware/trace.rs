//! Trace middleware for the gateway JSON-RPC handler pipeline.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Layer, Service};
use tracing::{debug, error};
use uuid::Uuid;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

#[derive(Clone)]
pub struct TraceLayer;

impl TraceLayer {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for TraceLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for TraceLayer {
    type Service = TraceService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TraceService { inner }
    }
}

#[derive(Clone)]
pub struct TraceService<S> {
    inner: S,
}

impl<S> TraceService<S> {
    pub const fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Service<JsonRpcRequest> for TraceService<S>
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

        let request_id = Uuid::new_v4();
        let method = req.method.clone();

        debug!(
            request_id = %request_id,
            method = %method,
            "→ incoming request"
        );

        let start = std::time::Instant::now();

        Box::pin(async move {
            let result = inner_mut.call(req).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;

            match &result {
                Ok(resp) => {
                    if resp.is_error() {
                        debug!(
                            request_id = %request_id,
                            method = %method,
                            elapsed_ms = %elapsed_ms,
                            "← request completed with error"
                        );
                    } else {
                        debug!(
                            request_id = %request_id,
                            method = %method,
                            elapsed_ms = %elapsed_ms,
                            "← request completed"
                        );
                    }
                }
                Err(_) => {
                    error!(
                        request_id = %request_id,
                        method = %method,
                        elapsed_ms = %elapsed_ms,
                        "← request failed"
                    );
                }
            }

            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcResponse;
    use crate::sync_primitives::{AtomicU64, Ordering};

    #[derive(Clone)]
    struct MockService {
        counter: std::sync::Arc<AtomicU64>,
    }

    impl Service<JsonRpcRequest> for MockService {
        type Response = JsonRpcResponse;
        type Error = std::convert::Infallible;
        type Future =
            Pin<Box<dyn Future<Output = Result<JsonRpcResponse, std::convert::Infallible>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: JsonRpcRequest) -> Self::Future {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(JsonRpcResponse::success(None, serde_json::Value::Null)) })
        }
    }

    #[tokio::test]
    async fn test_trace_layer_passes_through() {
        let counter = std::sync::Arc::new(AtomicU64::new(0));
        let mock = MockService {
            counter: counter.clone(),
        };
        let traced = TraceLayer::new().layer(mock);

        let req = JsonRpcRequest::notification("test.method", None);
        let mut svc = traced;
        let waker = futures_util::task::noop_waker_ref();
        let _ = svc.poll_ready(&mut Context::from_waker(waker));
        let result = svc.call(req).await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}