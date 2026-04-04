//! Handler service adapter for the gateway middleware pipeline.
//!
//! Wraps `HandlerRegistry` as a Tower `Service` so it can serve as the
//! terminal service in the middleware chain.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};

use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

#[derive(Clone)]
pub struct HandlerLayer;

impl HandlerLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HandlerLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for HandlerLayer {
    type Service = HandlerService<S>;

    fn layer(&self, _inner: S) -> Self::Service {
        panic!("HandlerLayer must be used with HandlerService::new(Arc<HandlerRegistry>)")
    }
}

#[derive(Clone)]
pub struct HandlerService<S> {
    _marker: S,
    handlers: Arc<HandlerRegistry>,
}

impl HandlerService<()> {
    pub fn new(handlers: Arc<HandlerRegistry>) -> Self {
        Self { _marker: (), handlers }
    }
}

impl<S> HandlerService<S> {
    pub fn with_inner(inner: S, handlers: Arc<HandlerRegistry>) -> Self {
        Self { _marker: inner, handlers }
    }
}

impl Service<JsonRpcRequest> for HandlerService<()> {
    type Response = JsonRpcResponse;
    type Error = std::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: JsonRpcRequest) -> Self::Future {
        let handlers = self.handlers.clone();
        Box::pin(async move {
            Ok(handlers.handle(&req).await)
        })
    }
}
