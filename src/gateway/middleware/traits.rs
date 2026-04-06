//! Middleware abstractions for the gateway middleware chain.


/// Marker trait for middleware that can be composed in the gateway middleware chain.
/// Each middleware implements `Layer<S>` following Tower's pattern:
/// `impl<S> Layer<S> for TraceLayer { type Service = TraceService<S>; }`
pub trait GatewayMiddleware: Clone + Send + Sync + 'static {
    type Service: Clone + Send + Sync + 'static;
}
