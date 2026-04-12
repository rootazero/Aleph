---
title: "feat: Tower-Style Handler Middleware"
type: feat
status: active
date: 2026-04-05
origin: docs/brainstorms/2026-04-05-tower-style-handler-middleware-requirements.md
---

# Tower-Style Handler Middleware

## Overview

Add a composable Tower `Layer` middleware chain to Aleph's JSON-RPC gateway. The chain (Trace → Metrics → Auth → RateLimit → Validate → HandlerService) wraps the existing `HandlerRegistry` dispatch without modifying `HandlerFn` signatures. Plugin middleware can be injected at a fixed position (after Auth, before RateLimit).

## Problem Frame

Aleph's gateway has no shared middleware. Cross-cutting concerns (auth, rate-limiting, tracing, metrics) are either hardcoded inside handlers or absent. OpenClaw uses AsyncLocalStorage per-request scope; Aleph should use Tower's `Layer<S>` composable middleware — a more type-safe, Rust-idiomatic approach that Aleph's existing `tower = "0.5"` dependency enables.

## Requirements Trace

- **R1**: `src/gateway/middleware/` module with Tower `Layer` implementations
- **R5**: `GatewayMiddleware` trait that all middleware implement
- **R7–R10**: `TraceLayer` — logs request/response with `request_id` correlation
- **R11–R14**: `MetricsLayer` — per-method counters, histograms via existing `metrics` crate
- **R15–R18**: `AuthLayer` — validates tokens, rejects unauthenticated, injects `user_id` into context
- **R19–R22**: `RateLimitLayer` — token bucket, in-memory `DashMap`, `429` + `Retry-After`
- **R23–R25**: `ValidateLayer` — JSON Schema validation via `schemars`
- **R26–R28**: `HandlerService` — adapts `HandlerRegistry::handle` to Tower `Service`
- **R29–R32**: Backward compatibility — `HandlerRegistry`, `HandlerFn` unchanged
- **R33–R36**: Plugin middleware via `PluginMiddlewareRegistry`
- **R37–R39**: `GatewayRequestContext` added to `GatewayRequest`

## Scope Boundaries

**In Scope:**
- Gateway middleware chain in `src/gateway/middleware/`
- Middleware `Layer` implementations: Trace, Metrics, Auth, RateLimit, Validate
- `GatewayRequestContext` added to request path
- `HandlerService` adapter wrapping existing `HandlerRegistry`
- Plugin middleware registry and hook integration
- Backward compatibility with existing `HandlerRegistry`

**Out of Scope:**
- Middleware for non-gateway paths (CLI, desktop bridge)
- Distributed rate limiting (Redis) — in-memory only for v1
- Per-handler middleware (global chain only)
- Modifying existing handler implementations
- Transport-level middleware (TLS, CORS — already handled)

## Key Technical Decisions

- **Service shape**: Middleware operates on `JsonRpcRequest → JsonRpcResponse` (not HTTP types). This differs from `SecurityHeadersLayer` which wraps HTTP services — the middleware chain sits between JSON-RPC parsing and handler dispatch in `process_request`.
- **No AsyncLocalStorage**: `GatewayRequestContext` is explicit, passed through the middleware chain via Tower's `Service` pattern. No `AsyncLocalStorage` equivalent.
- **Stateless per-request**: All middleware services are `Clone` and maintain no mutable shared state. Rate limit state uses existing `RateLimiter` (shared `Arc<DashMap<ClientId, RateLimitState>>`).
- **HandlerFn unchanged**: `HandlerFn` remains `Fn(JsonRpcRequest)`. The `HandlerService` is a Tower `Service` that calls `HandlerRegistry::handle` internally — handlers remain unaware of the middleware chain.
- **Plugin injection point**: Plugin middleware inserts after `AuthLayer`, before `RateLimitLayer`. This ensures plugins see authenticated `user_id` but are subject to rate limiting.
- **REUSE existing implementations**: Do NOT build Auth or RateLimit from scratch. `AuthLayer` adapts existing `security/` auth patterns; `RateLimitLayer` wraps existing `rate_limiter.rs` with its sliding-window algorithm.

## Open Questions

### Resolved During Planning

- **Request type for middleware**: Use `JsonRpcRequest` directly rather than wrapping in a new middleware-specific type. `process_request` already parses/validates before calling `handlers.handle`, so middleware sees pre-validated requests.
- **Rate limit key**: Per `client_id` (extracted from auth context), fallback to IP address. No per-method rate limits in v1 (too complex for initial implementation).
- **Trace context propagation**: Use `tracing::info!` with `request_id` field. No distributed context propagation needed (single Aleph instance).

### Deferred to Implementation

- Whether to validate schema per-method or per-request body shape
- Plugin middleware trait exact signature (whether plugins return a `Layer` or a pre-built `Service`)

## Module Structure (Target)

```
src/gateway/
├── middleware/
│   ├── mod.rs                      # MiddlewareChain builder + exports
│   ├── traits.rs                   # GatewayMiddleware trait
│   ├── context.rs                  # GatewayRequestContext
│   ├── trace.rs                    # TraceLayer
│   ├── metrics.rs                  # MetricsLayer
│   ├── auth.rs                     # AuthLayer
│   ├── rate_limit.rs               # RateLimitLayer
│   ├── validate.rs                 # ValidateLayer
│   ├── handler_service.rs          # HandlerService (terminal)
│   └── plugin.rs                   # PluginMiddlewareRegistry
├── handlers/
│   └── mod.rs                      # HandlerRegistry + HandlerFn (UNCHANGED)
├── server/
│   └── handler.rs                  # process_request() — wires MiddlewareChain
```

## Implementation Units

- [ ] **Unit 1: `GatewayRequestContext` and `GatewayMiddleware` trait**

**Goal:** Establish the types that all middleware share.

**Requirements:** R5, R37, R38, R39

**Dependencies:** None

**Files:**
- Create: `src/gateway/middleware/context.rs`
- Create: `src/gateway/middleware/traits.rs`

**Approach:**
- `GatewayRequestContext` is a `Clone + Send + Sync` struct carried through the middleware chain. It holds: `request_id: Uuid`, `user_id: Option<UserId>`, `client_id: ClientId`, `trace_flags: TraceFlags`, `plugin_state: Arc<PluginState>`.
- `GatewayMiddleware` trait: `trait GatewayMiddleware: Clone { type Service; fn layer(&self) -> Arc<dyn Layer<Self::Service>>; }`
- All middleware implement this trait. The trait is not a Tower requirement but aligns with the requirements doc pattern and makes plugin injection uniform.

**Patterns to follow:**
- `SecurityHeadersLayer` (`src/security/headers.rs`) for `Layer<S>` implementation shape
- Standard Rust `trait` + `impl` pattern with `Clone` bound

**Test scenarios:**
- Happy path: `GatewayRequestContext::new()` creates a context with a fresh UUID and no user_id
- Edge case: context.clone() preserves all fields correctly
- `GatewayMiddleware` can be cloned and produces an `Arc<dyn Layer>`

**Verification:**
- `cargo check -p alephcore` passes
- Unit tests in `context.rs` and `traits.rs` pass

---

- [ ] **Unit 2: `TraceLayer` and `MetricsLayer`**

**Goal:** Add observability — request logging and metrics collection.

**Requirements:** R7, R8, R9, R10, R11, R12, R13, R14

**Dependencies:** Unit 1

**Files:**
- Create: `src/gateway/middleware/trace.rs`
- Create: `src/gateway/middleware/metrics.rs`

**Approach:**
- `TraceLayer`: wraps the downstream service, logs `tracing::info!` on call with `request_id`, `method`, duration_ms on completion. Request ID is generated at the edge (first middleware) and propagated via `GatewayRequestContext`.
- `MetricsLayer`: uses `metrics::increment_counter!` and `metrics::histogram!` macros (already in Aleph). Labels: `method`, `status`, `handler`.
- Both follow the `SecurityHeadersService<S>` pattern: `struct TraceService<S> { inner: S, context: GatewayRequestContext }`

**Patterns to follow:**
- `SecurityHeadersLayer` / `SecurityHeadersService` in `src/security/headers.rs`
- Aleph's existing `tracing` usage (grep for `tracing::info!` in gateway)

**Test scenarios:**
- Happy path: TraceLayer logs request with method name and request_id
- Happy path: MetricsLayer increments counter on both success and error
- Edge case: metrics labels are correctly extracted from response status

**Verification:**
- `cargo check -p alephcore` passes
- Unit tests for both layers pass

---

- [ ] **Unit 3: `AuthLayer`** *(Reuse existing — do not build from scratch)*

**Goal:** Authenticate requests before they reach handlers, injecting `user_id` into `GatewayRequestContext`.

**Requirements:** R15, R16, R17, R18

**Dependencies:** Unit 1

**Files:**
- Create: `src/gateway/middleware/auth.rs` — thin Tower `Layer` adapter around existing auth middleware

**Approach:**
- Aleph already has `bearer_auth_middleware` and `session_auth_middleware` in `src/gateway/auth_middleware.rs` (axum `Next` pattern).
- `AuthLayer` adapts these into a Tower `Layer<Service>` that:
  1. Extracts token from `JsonRpcRequest` params or headers (note: JSON-RPC doesn't have HTTP headers — token must be in params, e.g., `{"token": "..."}` or `{"method": "auth.login", "params": {...}}`).
  2. Validates using existing `security/` module (do NOT reimplement token validation).
  3. Rejects with `JsonRpcResponse::error(AUTH_REQUIRED, ...)` on failure.
  4. On success, populates `GatewayRequestContext.user_id`.
- **Key insight**: The existing axum middleware is HTTP-specific. The new `AuthLayer` must work with `JsonRpcRequest → JsonRpcResponse` service shape.

**Patterns to follow:**
- Existing `src/gateway/auth_middleware.rs` — bearer and session auth patterns
- Existing `security/` module for token validation (do NOT reimplement)
- `SecurityHeadersLayer` for Layer implementation shape

**Test scenarios:**
- Happy path: valid token populates `user_id` in context
- Error path: missing token returns AUTH_REQUIRED error response (not panic)
- Error path: invalid token returns AUTH_FAILED error response
- Edge case: token valid but user disabled

**Verification:**
- `cargo check -p alephcore` passes
- Unit tests pass

---

- [ ] **Unit 4: `RateLimitLayer`** *(Wrap existing — do not build from scratch)*

**Goal:** Enforce per-client rate limits using the existing `RateLimiter`.

**Requirements:** R19, R20, R21, R22

**Dependencies:** Unit 1

**Files:**
- Create: `src/gateway/middleware/rate_limit.rs` — thin Tower `Layer` adapter around existing `RateLimiter`

**Approach:**
- Aleph already has a production-ready rate limiter at `src/gateway/rate_limiter.rs`:
  - **Sliding window** algorithm with `DashMap` for lock-free concurrent access
  - `RateLimitScope::RpcDefault`, `RpcWrite`, `RpcHeavy`, `Auth`, `WebhookAuth` — scoped per method
  - `check_and_record(key, scope) -> Result<(), RateLimitError>` — already async-compatible
  - `RateLimitError` carries `retry_after_ms`
- `RateLimitLayer` wraps this existing `RateLimiter` as a Tower `Layer`:
  1. Extracts `client_id` from `GatewayRequestContext` (set by AuthLayer upstream)
  2. Calls `rate_limiter.check_and_record(client_id, scope_for_method(method))`
  3. Returns `JsonRpcResponse::error(RATE_LIMITED, ...)` with `Retry-After` on failure
- Key integration: The existing `rate_limiter` is already injected into `server/handler.rs` at startup — wire it into the middleware chain via `Arc`.

**Patterns to follow:**
- Existing `src/gateway/rate_limiter.rs` — sliding window, `DashMap`, scope classification
- `dashmap::DashMap` for concurrent shared state (already in Aleph's dependency tree)
- Existing `Lane` pattern in `src/gateway/lane.rs` for scope classification (similar concept)

**Test scenarios:**
- Happy path: first request succeeds, rate limit state recorded
- Edge case: rate limit exceeded returns 429 with `Retry-After` header
- Edge case: concurrent requests from same client handled correctly (DashMap is lock-free)
- Integration: works with existing `rate_limiter` configuration and loopback exemption

**Verification:**
- `cargo check -p alephcore` passes
- Rate limit behavior verified via integration test with concurrent requests

---

- [ ] **Unit 5: `ValidateLayer` and `HandlerService`**

**Goal:** Validate requests and dispatch to `HandlerRegistry`.

**Requirements:** R23, R24, R25, R26, R27, R28

**Dependencies:** Units 1–4 (all prior middleware)

**Files:**
- Create: `src/gateway/middleware/validate.rs`
- Create: `src/gateway/middleware/handler_service.rs`
- Modify: `src/gateway/server/handler.rs` (wire MiddlewareChain into `process_request`)

**Approach:**
- `ValidateLayer`: validates inbound `JsonRpcRequest` against JSON Schema (using `schemars` — need to check if schemas are already generated for handlers or if we need a lightweight per-method validation). For v1, validate basic JSON-RPC structure only (method field present, params is object/null).
- `HandlerService`: This is the terminal service — it wraps `HandlerRegistry` and implements `Service<JsonRpcRequest, Response = JsonRpcResponse>`. It calls `HandlerRegistry::handle(&request)` synchronously (already async-compatible).
- The existing `process_request` in `handler.rs` currently parses JSON, validates, then calls `handlers.handle(&request)`. This becomes: parse JSON → build `GatewayRequestContext` → call `MiddlewareChain::new().serve(request)`.

**Patterns to follow:**
- `SecurityHeadersService` for service implementation shape
- Existing `HandlerRegistry::handle` call pattern

**Test scenarios:**
- Happy path: valid request passes through ValidateLayer to HandlerService
- Error path: malformed JSON-RPC returns PARSE_ERROR before middleware
- Error path: invalid params structure returns INVALID_PARAMS after ValidateLayer

**Verification:**
- `cargo check -p alephcore` passes
- Existing handler tests (`cargo test -p alephcore --lib`) pass — confirms backward compatibility

---

- [ ] **Unit 6: `MiddlewareChain` builder and wiring**

**Goal:** Compose all layers into a single `MiddlewareChain` and wire into `process_request`.

**Requirements:** R4, R29, R30, R31, R32

**Dependencies:** Units 1–5

**Files:**
- Create: `src/gateway/middleware/mod.rs` (the builder and exports)
- Modify: `src/gateway/server/handler.rs`

**Approach:**
- `MiddlewareChain` is a builder that assembles layers in order: `TraceLayer → MetricsLayer → AuthLayer → PluginMiddleware → RateLimitLayer → ValidateLayer → HandlerService`.
- `MiddlewareChain::new()` composes using repeated `.layer()` calls: `TraceLayer.layer().layer(MetricsLayer.layer())...`
- In `process_request`, after parsing JSON, build context and call `middleware_chain.serve(request).await`.
- The existing `HandlerRegistry::handle` is unchanged — `HandlerService` wraps it.

**Patterns to follow:**
- Tower's `ServiceBuilder` pattern for layer composition
- Aleph's existing builder patterns (grep for `impl Foo { pub fn new() -> Self`)

**Test scenarios:**
- Happy path: full chain processes a request end-to-end
- Integration: middleware layers execute in correct order (trace → metrics → auth → plugin → rate_limit → validate → handler)
- Backward compat: existing handler tests pass without modification

**Verification:**
- `cargo test -p alephcore --lib` passes
- Manual verification: send JSON-RPC request and observe trace logs, metrics increments

---

- [ ] **Unit 7: Plugin middleware injection**

**Goal:** Allow plugins to register custom middleware into the chain.

**Requirements:** R33, R34, R35, R36

**Dependencies:** Units 1–6

**Files:**
- Create: `src/gateway/middleware/plugin.rs`
- Modify: `src/gateway/middleware/mod.rs` (register plugin middleware in chain)

**Approach:**
- `PluginMiddlewareRegistry`: maintains a `Vec<Box<dyn GatewayMiddleware>>`. Plugins call `registry.register(middleware)` at startup or runtime.
- The `MiddlewareChain` builder accepts the registry and inserts plugin middleware after AuthLayer.
- Plugin middleware must implement `GatewayMiddleware` trait — provides `Layer<Self::Service>`.

**Patterns to follow:**
- Aleph's existing plugin hook system (grep for `plugin_hook`, `gateway_hook` in codebase)
- Tower's `Layer` composition pattern

**Test scenarios:**
- Plugin registers middleware → middleware appears in chain after Auth
- Plugin middleware can access `GatewayRequestContext.user_id` set by AuthLayer
- Multiple plugins can each register their own middleware

**Verification:**
- `cargo check -p alephcore` passes
- Plugin middleware fires in correct position in chain

---

## System-Wide Impact

- **Interaction graph:** `process_request` in `handler.rs` now calls `MiddlewareChain::new().serve(request)` instead of `handlers.handle(&request)`. All existing callers of `HandlerRegistry` are unaffected.
- **Error propagation:** Middleware errors return `JsonRpcResponse::error(...)` — same type as handlers, so WebSocket/HTTP transports are unaffected.
- **State lifecycle risks:** Rate limit state grows unbounded — add a periodic cleanup task (deferred to future work).
- **API surface parity:** `HandlerFn` type alias unchanged — existing code using `Arc<HandlerFn>` continues to work.
- **Integration coverage:** Full chain test requires a real `JsonRpcRequest` with valid auth token.
- **Unchanged invariants:** `HandlerRegistry::handle` behavior is unchanged — same method lookup, same error responses.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Middleware chain adds per-request latency | Benchmark before/after; ensure p99 < 10ms overhead |
| Rate limit state grows unbounded (existing `rate_limiter.rs` has same limitation) | Accept as known limitation for v1; add cleanup in future |
| Auth layer adaptation from HTTP-specific middleware to JSON-RPC | `AuthLayer` must handle `JsonRpcRequest` params (no HTTP headers in JSON-RPC context) |
| Plugin middleware trait is too restrictive | Keep trait minimal; allow plugins to provide pre-built `Service` if `Layer` is too complex |
| Breaking change to `process_request` interface | Wrap in compatibility shim if needed; existing callers unchanged |

## Verification Commands

```bash
cargo check -p alephcore                          # Fast compile
cargo clippy -p alephcore -- --deny warnings     # Lint
cargo test -p alephcore --lib                     # Unit tests
cargo test -p alephcore --lib test_name          # Single test
```

## Sources & References

- **Origin document:** [docs/brainstorms/2026-04-05-tower-style-handler-middleware-requirements.md](docs/brainstorms/2026-04-05-tower-style-handler-middleware-requirements.md)
- **Existing Tower Layer example:** `src/security/headers.rs` — `SecurityHeadersLayer`, `SecurityHeadersService`
- **HandlerRegistry:** `src/gateway/handlers/mod.rs` — `HandlerFn`, `HandlerRegistry::handle`
- **Gateway server:** `src/gateway/server/handler.rs` — `process_request`, the injection point
- **Protocol types:** `src/gateway/protocol.rs` — `JsonRpcRequest`, `JsonRpcResponse`
- **Existing RateLimiter (REUSE):** `src/gateway/rate_limiter.rs` — sliding window, DashMap, scope classification
- **Existing Auth Middleware (REUSE):** `src/gateway/auth_middleware.rs` — bearer and session auth patterns
- **Existing Metrics/Timing:** `src/metrics/mod.rs` — `StageTimer`, `time_stage!` macro
- **Existing Lane pattern:** `src/gateway/lane.rs` — scope classification pattern for concurrency
- **Schema validation:** `src/config/patcher.rs` — `jsonschema` crate usage for runtime validation
- **Extension/Hook system:** `src/extension/mod.rs` — plugin hook architecture to align plugin middleware with
