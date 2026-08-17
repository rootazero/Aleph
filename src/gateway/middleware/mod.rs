//! Tower-style middleware for the gateway JSON-RPC handler pipeline.
//!
//! # Architecture
//!
//! The middleware chain runs between JSON-RPC parsing and `HandlerRegistry` dispatch:
//!
//! ```text
//! process_request (handler.rs)
//!   → parse JSON
//!   → MiddlewareChain
//!       → TraceLayer      (logs request_id, method, duration)
//!       → MetricsLayer   (increments counters, records latency, tracks state)
//!       → AuthLayer      (pass-through; connection identity is set at connect)
//!       → PluginMiddlewares  (inserted by PluginMiddlewareRegistry)
//!       → RateLimitLayer (token bucket, returns 429 on exceed)
//!       → ValidateLayer  (schema validation)
//!       → HandlerService (terminal, dispatches to HandlerRegistry)
//! ```
//!
//! All middleware operates on `JsonRpcRequest → JsonRpcResponse` via Tower's
//! `Layer<Service>` composable pattern.
//!
//! # Request Lifecycle
//!
//! Requests are tracked through typestate-inspired stages:
//! `Pending → Validating → Processing → AwaitingResponse → Completed/Failed/Cancelled`

pub mod auth;
pub mod chain;
pub mod handler_service;
pub mod latency;
pub mod metrics;
pub mod rate_limit;
pub mod redact_query;
pub mod request_state;
pub mod trace;
pub mod traits;
pub mod validate;

pub use auth::{AuthLayer, AuthService};
pub use chain::MiddlewareChain;
pub use handler_service::{HandlerLayer, HandlerService};
pub use metrics::{MetricsLayer, MetricsService};
pub use rate_limit::RateLimitLayer;
pub use redact_query::RedactQueryLayer;
pub use request_state::{RequestState, RequestStateData, RequestStateRegistry, StateSnapshot};
pub use trace::{TraceLayer, TraceService};
pub use validate::ValidateLayer;
