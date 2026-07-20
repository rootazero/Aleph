---
date: 2026-04-05
topic: tower-style-handler-middleware
---

# Tower-Style Handler Middleware

## Problem Frame

Aleph's gateway layer uses a flat `HandlerRegistry` (simple `HashMap<String, HandlerFn>`) with **no composable middleware chain**. Every handler is a raw function with no shared cross-cutting concerns — authentication, rate-limiting, tracing, and metrics are either hardcoded inside handlers or absent entirely.

OpenClaw (TypeScript) solves this with `withPluginRuntimeGatewayRequestScope()` — an AsyncLocalStorage per-request scope wrapper. It's not composable middleware, but it establishes the pattern: **per-request context propagation that plugins can access**.

Aleph has `tower = "0.5"` and `tower-http = "0.6"` in `Cargo.toml` but only uses Tower for HTTP CORS headers (`SecurityHeadersLayer`). The Tower `Layer` abstraction is the correct Rust pattern for composable middleware — we should use it.

**Goal**: Implement Tower-style handler middleware that:
- Leverages Aleph's Rust/Tower advantages (not a copy of OpenClaw's AsyncLocalStorage)
- Adds global middleware chain: Trace → Metrics → Auth → RateLimit → Validate → Handler
- Supports plugin middleware injection (OpenClaw's `withPluginRuntimeGatewayRequestScope` equivalent)
- Maintains backward compatibility with existing `HandlerRegistry::call` patterns

**Affected Users**: All gateway request paths (WebSocket RPC, HTTP endpoints, agent execution pipeline).

---

## Design Decisions (Confirmed)

| Decision | Choice |
|----------|--------|
| Backward compatibility | Keep existing `HandlerRegistry` + `HandlerFn` — wrap in compatibility layer |
| Middleware granularity | Global chain — single middleware chain for all requests |
| Plugin integration | Yes — plugins can inject custom middleware via hook |
| Middleware order | Trace → Metrics → Auth → RateLimit → Validate → Handler |

---

## Requirements

### Middleware Architecture

- **R1**: Create `src/gateway/middleware/` module with Tower `Layer` implementations
- **R2**: Each middleware implements `tower::Layer<Service>` where `Service` is the downstream handler
- **R3**: Middleware services are `Clone` and share no mutable state (stateless per request)
- **R4**: Middleware chain order is fixed: `TraceLayer → MetricsLayer → AuthLayer → RateLimitLayer → ValidateLayer → HandlerService`

### Middleware Traits

- **R5**: Define a `GatewayMiddleware` trait that all middleware must implement:
  ```rust
  pub trait GatewayMiddleware: Clone {
      type Service: Service<GatewayRequest, Response = GatewayResponse, Error = GatewayError> + Send + 'static;
      fn layer(&self) -> Arc<dyn Layer<Self::Service>>;
  }
  ```
- **R6**: Each middleware wraps the downstream `Service` and returns a `Service` implementing the same `GatewayRequest → GatewayResponse` transform

### Trace Layer

- **R7**: `TraceLayer` logs every inbound request with method, params shape, and a generated `request_id` (UUID v4)
- **R8**: `TraceLayer` logs every outbound response with status, duration_ms, and request_id
- **R9**: Uses `tracing` crate (already in Aleph's dependency tree) — `tracing::info!` for requests, `tracing::debug!` for bodies
- **R10**: Request ID is propagated via `request_id` field in `GatewayRequest` context (not AsyncLocalStorage — pure Tower context)

### Metrics Layer

- **R11**: `MetricsLayer` tracks per-method request counts, error counts, and latency histograms
- **R12**: Metrics are exposed via Aleph's existing metrics system (prometheus-compatible)
- **R13**: Metrics labels: `method`, `status` (ok/error), `handler`
- **R14**: Use `metrics::increment_counter!` and `metrics::histogram!` macros (already in Aleph)

### Auth Layer

- **R15**: `AuthLayer` validates the session token / API key from `GatewayRequest`
- **R16**: Rejects unauthenticated requests with `GatewayError::Unauthorized` before passing to downstream
- **R17**: Extracts `user_id` from token and injects into `GatewayRequest` context for downstream handlers
- **R18**: Uses Aleph's existing `security/` module for token validation (do not reimplement)

### RateLimit Layer

- **R19**: `RateLimitLayer` enforces per-client rate limits using a token bucket algorithm
- **R20**: Default: 100 requests/minute per client_id; configurable per handler namespace
- **R21**: Returns `GatewayError::RateLimited` with `Retry-After` header when exceeded
- **R22**: In-memory rate limit state (no external cache needed for v1); shared state via `Arc<dashmap::DashMap<ClientId, RateLimitState>>`

### Validate Layer

- **R23**: `ValidateLayer` validates inbound `GatewayRequest` against the handler's JSON Schema
- **R24**: Uses `schemars` (already in Aleph's dependency tree) for schema generation
- **R25**: Validation errors return `GatewayError::ValidationError` with field-level error details

### Handler Service (Terminal)

- **R26**: `HandlerService` is the terminal service — it dispatches to the existing `HandlerRegistry`
- **R27**: `HandlerService` wraps `HandlerRegistry::call` and adapts it to the `Service` trait
- **R28**: Handler lookup is by `request.method` — same as current `HandlerRegistry`

### Backward Compatibility

- **R29**: Existing `HandlerRegistry::call(req: &GatewayRequest) -> Result<GatewayResponse>` remains unchanged
- **R30**: `HandlerFn` type alias remains the same — handlers are unaware of the middleware chain
- **R31**: The middleware chain wraps `HandlerRegistry` at the gateway routing layer, not inside handlers
- **R32**: Existing handler tests continue to pass without modification

### Plugin Middleware Injection

- **R33**: Plugins register middleware via a new `gateway.middleware` hook point
- **R34**: Plugin middleware is inserted into the chain at a fixed position: after Auth, before RateLimit
- **R35**: Plugin middleware must implement `GatewayMiddleware` trait
- **R36**: Plugin middleware registration happens at server startup (compile-time known plugins) and at runtime for dynamic plugins

### Request Context Propagation

- **R37**: `GatewayRequest` gains a `context: GatewayRequestContext` field:
  ```rust
  pub struct GatewayRequestContext {
      pub request_id: Uuid,
      pub user_id: Option<UserId>,
      pub client_id: ClientId,
      pub trace_flags: TraceFlags,
      pub plugin_state: Arc<PluginState>, // for plugin middleware access
  }
  ```
- **R38**: Context is immutable per request — created at the edge, propagated via Tower's `Service` pattern
- **R39**: No AsyncLocalStorage — all context is explicit and passed through the middleware chain

### Module Structure

```
src/gateway/
├── middleware/
│   ├── mod.rs                    # Module exports + MiddlewareChain builder
│   ├── traits.rs                 # GatewayMiddleware trait definition
│   ├── trace.rs                  # TraceLayer implementation
│   ├── metrics.rs                # MetricsLayer implementation
│   ├── auth.rs                   # AuthLayer implementation
│   ├── rate_limit.rs             # RateLimitLayer implementation
│   ├── validate.rs               # ValidateLayer implementation
│   ├── plugin.rs                 # PluginMiddlewareRegistry
│   └── context.rs                # GatewayRequestContext
├── handlers/
│   └── mod.rs                    # HandlerRegistry + HandlerFn (unchanged)
├── inbound_router/
│   └── mod.rs                    # InboundMessageRouter (updated to use MiddlewareChain)
```

---

## Success Criteria

- **SC1**: `cargo check -p alephcore` passes with zero new errors
- **SC2**: `cargo test -p alephcore --lib` passes — existing handler tests still work
- **SC3**: Middleware chain is configurable at startup (enable/disable per layer)
- **SC4**: `tracing` shows request_id from edge to handler (log correlation)
- **SC5**: Metrics are exposed at `/metrics` endpoint for all gateway methods
- **SC6**: Unauthenticated requests are rejected at the `AuthLayer` before reaching handlers
- **SC7**: Rate-limited requests return `429` with `Retry-After` header
- **SC8**: Plugin middleware appears in the chain after AuthLayer
- **SC9**: Backward compatibility: existing `HandlerRegistry` and `HandlerFn` are unchanged

---

## Scope Boundaries

**In Scope:**
- Gateway middleware chain in `src/gateway/middleware/`
- Middleware `Layer` implementations: Trace, Metrics, Auth, RateLimit, Validate
- `GatewayRequestContext` addition to `GatewayRequest`
- `HandlerService` adapter wrapping existing `HandlerRegistry`
- Plugin middleware registry and hook integration
- Backward compatibility with existing `HandlerRegistry`

**Out of Scope:**
- Middleware for non-gateway paths (CLI, desktop bridge)
- Distributed rate limiting ( Redis) — in-memory only for v1
- Middleware per handler (global chain only)
- Modifying existing handler implementations
- Transport-level middleware (TLS, CORS — already handled by tower-http)

---

## Open Questions (Resolved)

| Question | Resolution |
|----------|------------|
| Backward compat for HandlerRegistry? | Yes — wrap in HandlerService adapter, keep HandlerFn unchanged |
| Global or per-handler middleware? | Global chain only |
| Plugin middleware injection? | Yes — after Auth, before RateLimit, via `gateway.middleware` hook |
| Middleware order? | Trace → Metrics → Auth → RateLimit → Validate → Handler |
| AsyncLocalStorage vs explicit context? | Explicit context via `GatewayRequestContext` — no AsyncLocalStorage |
| OpenClaw's `withPluginRuntimeGatewayRequestScope` equivalent? | `GatewayRequestContext.plugin_state: Arc<PluginState>` carries plugin context through the middleware chain |

---

## Comparison with OpenClaw

| Aspect | OpenClaw | Aleph (Target) |
|--------|----------|----------------|
| Middleware model | AsyncLocalStorage per-request scope | Tower `Layer` + explicit `GatewayRequestContext` |
| Handler map | Flat `coreGatewayHandlers` HashMap | Same `HandlerRegistry`, wrapped in `HandlerService` |
| Auth | `authorizeGatewayMethod()` in handler dispatch | `AuthLayer` as first-class Tower layer |
| Rate limiting | Per-method budget in dispatch | `RateLimitLayer` with token bucket |
| Tracing | None (implicit via logs) | `TraceLayer` with request_id correlation |
| Plugin scope | `withPluginRuntimeGatewayRequestScope()` wraps handler | `PluginState` in `GatewayRequestContext`, injected via `PluginMiddlewareRegistry` |
| Type safety | TypeScript (dynamic) | Rust (compile-time `Service` trait) |

**Advantage over OpenClaw**: Aleph's middleware is composable via Tower's `Layer::layer()` combinators, typed via `Service` trait, and verified at compile time. OpenClaw's AsyncLocalStorage is dynamic and implicit.
