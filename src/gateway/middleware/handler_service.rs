//! Handler service adapter for the gateway middleware pipeline.
//!
//! Wraps `HandlerRegistry` as a Tower `Service` so it can serve as the
//! terminal service in the middleware chain.

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Layer, Service};

use crate::gateway::handlers::{voice_turn::VoiceRequestContext, HandlerRegistry};
use crate::gateway::panel_project_context::PanelRequestContext;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

#[derive(Clone)]
pub struct HandlerLayer {
    handlers: Arc<HandlerRegistry>,
}

impl HandlerLayer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(HandlerRegistry::default()),
        }
    }
}

impl Default for HandlerLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for HandlerLayer {
    type Service = HandlerService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HandlerService::with_inner(inner, self.handlers.clone())
    }
}

#[derive(Clone)]
pub struct HandlerService<S> {
    _marker: S,
    handlers: Arc<HandlerRegistry>,
    panel_context: Option<PanelRequestContext>,
    voice_context: Option<VoiceRequestContext>,
}

impl HandlerService<()> {
    #[must_use]
    pub const fn new(handlers: Arc<HandlerRegistry>) -> Self {
        Self {
            _marker: (),
            handlers,
            panel_context: None,
            voice_context: None,
        }
    }

    pub(crate) fn with_context(
        mut self,
        panel_context: Option<PanelRequestContext>,
        voice_context: Option<VoiceRequestContext>,
    ) -> Self {
        self.panel_context = panel_context;
        self.voice_context = voice_context;
        self
    }
}

impl<S> HandlerService<S> {
    pub const fn with_inner(inner: S, handlers: Arc<HandlerRegistry>) -> Self {
        Self {
            _marker: inner,
            handlers,
            panel_context: None,
            voice_context: None,
        }
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
        let panel_context = self.panel_context.clone();
        let voice_context = self.voice_context.clone();
        Box::pin(async move {
            Ok(handlers
                .handle_with_context(&req, panel_context.as_ref(), voice_context)
                .await)
        })
    }
}
