//! Validation middleware for the gateway JSON-RPC handler pipeline.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Layer, Service};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_REQUEST};

#[derive(Clone)]
pub struct ValidateLayer;

impl ValidateLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ValidateLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for ValidateLayer {
    type Service = ValidateService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ValidateService { inner }
    }
}

#[derive(Clone)]
pub struct ValidateService<S> {
    inner: S,
}

impl<S> ValidateService<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Service<JsonRpcRequest> for ValidateService<S>
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
        if let Err(e) = req.validate() {
            let response = JsonRpcResponse::error(req.id.clone(), INVALID_REQUEST, e.message);
            return Box::pin(async move { Ok(response) });
        }

        let inner = self.inner.clone();
        let mut inner_mut = std::mem::replace(&mut self.inner, inner);
        Box::pin(async move {
            inner_mut.call(req).await
        })
    }
}
