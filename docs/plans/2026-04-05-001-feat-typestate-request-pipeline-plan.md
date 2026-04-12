---
title: Typestate Request Pipeline
type: feat
status: active
date: 2026-04-05
origin: docs/brainstorms/2026-04-05-typestate-request-pipeline-requirements.md
---

# Typestate Request Pipeline

## Overview

Implement request lifecycle state tracking for the JSON-RPC middleware chain with state distribution metrics and backpressure signals.

## Problem Frame

The current middleware chain handles JSON-RPC requests but lacks explicit lifecycle state tracking. When a request enters the system, we increment `requests_in_flight`, but we don't track which stage it's in (validating, processing, awaiting response). This makes it impossible to:
- Detect when the system is saturated at a specific stage
- Provide meaningful state feedback to operators
- Apply backpressure based on processing depth

## Requirements Trace

- **R1**: Request lifecycle as typestates with compile-time transitions
- **R2**: `RequestState` enum with states: `Pending`, `Validating`, `Processing`, `AwaitingResponse`, `Completed`, `Failed`, `Cancelled`
- **R3**: Extend `MetricsLayer` to track state distribution
- **R4**: Backpressure signals (429 on threshold exceeded)
- **R5**: `request.state` RPC introspection endpoint
- **R6**: Per-request timing tracking

## Scope Boundaries

- The typestate is for **request lifecycle**, not media pipeline (MessagePipeline is separate)
- Does not change how handlers are dispatched — only adds tracking
- Does not replace existing error handling — supplements it with state tracking

## Context & Research

### Relevant Code and Patterns

| File | Purpose |
|------|---------|
| `src/gateway/middleware/chain.rs` | Middleware chain builder - Trace → Metrics → Auth → RateLimit → Validate → Handler |
| `src/gateway/middleware/metrics.rs` | MetricsLayer - tracks `requests_total`, `requests_in_flight` with atomic counters |
| `src/gateway/middleware/context.rs` | `GatewayRequestContext` - request-scoped context propagated through chain |
| `src/gateway/middleware/handler_service.rs` | Terminal service wrapping `HandlerRegistry` |
| `src/gateway/handlers/mod.rs` | `HandlerRegistry` - HashMap of method → HandlerFn |
| `src/gateway/protocol.rs` | `JsonRpcRequest`, `JsonRpcResponse` types |

### Middleware Chain Architecture

```
MiddlewareChain::serve(request)
  → TraceLayer     (logs request_id, method, duration)
  → MetricsLayer   (increments counters, records latency) ← EXTEND HERE
  → AuthLayer      (validates token, populates user_id)
  → RateLimitLayer (token bucket, returns 429 on exceed)
  → ValidateLayer  (schema validation)
  → HandlerService (terminal, dispatches to HandlerRegistry)
```

### Existing Patterns

- **Atomic counters**: `Arc<AtomicU64>` for thread-safe metrics
- **Tower Layer/Service**: `Layer<S>` wraps `Service` to add behavior
- **Request context**: `GatewayRequestContext` carries request-scoped data through the chain
- **State machines**: Rust patterns docs show enum state machines with exhaustive matching

### Key Technical Decision

**Use a hybrid typestate-inspired approach:**

True compile-time typestates are incompatible with Tower's `Layer<Service>` composition (layers erase type state). Instead:

1. **Runtime-validated state machine** with `RequestState` enum and explicit transitions
2. **Compile-time state advancement** via the `StateMachine` wrapper that only exposes valid transition methods per state
3. **Per-request state instance** stored in `Arc<AtomicU64>` for lock-free updates
4. **State registry** tracks all in-flight requests for introspection

This achieves the spirit of typestate (invalid transitions are prevented) while remaining practical for async middleware.

## Key Technical Decisions

- **Decision**: Track state per-request via shared `Arc`, not embedded in types
  - Rationale: Middleware layers are composed via `Layer<S>` which erases type parameters. Each request needs its own state instance.
  - Tradeoff: Runtime state validation instead of compile-time, but backed by a StateMachine wrapper that enforces valid transitions

- **Decision**: States track middleware processing stages, not business logic
  - Rationale: Aligns with existing MiddlewareChain architecture
  - Middleware stages: parse → validate → auth → rate_limit → handle → await_response → complete

- **Decision**: State registry uses DashMap for concurrent access
  - Rationale: Multiple requests updating state concurrently; DashMap provides better ergonomics than Arc<Mutex<HashMap>>

## Open Questions

### Resolved During Planning

- **Q**: How to integrate with Tower Service pattern?
  - **A**: Each middleware layer receives a shared state tracker. The layer advances state at entry and exit of each middleware.

- **Q**: How to handle concurrent requests sharing state?
  - **A**: Use `DashMap<Uuid, RequestStateData>` for lock-free concurrent access. Each request has unique Uuid from GatewayRequestContext.

- **Q**: Should AwaitingResponse represent awaiting LLM response?
  - **A**: No — it represents any point where the request is waiting for external resources (LLM, file I/O, etc.). Handlers can advance to AwaitingResponse explicitly.

## High-Level Technical Design

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Request Lifecycle States                         │
│                                                                     │
│  ┌──────────┐    ┌────────────┐    ┌────────────┐    ┌───────────┐│
│  │ Pending  │───▶│ Validating │───▶│ Processing │───▶│ Awaiting  ││
│  └──────────┘    └────────────┘    └────────────┘    │ Response  ││
│       │                                     │         └─────┬─────┘│
│       │                                     ▼               │      │
│       │                              ┌────────────┐          │      │
│       │                              │  Completed │◀────────┴──────│
│       │                              └────────────┘                │
│       ▼                              ┌────────────┐                │
│  ┌──────────┐                   ┌──▶│  Failed    │◀───────────────┘
│  │Cancelled │                   │   └────────────┘                │
│  └──────────┘                   │                                 │
│       ▲                        │   ┌──────────┐                   │
│       └────────────────────────┴───┤ Cancelled│◀─────────────────┘
│                                    └──────────┘
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│                    MetricsLayer Extension                             │
│                                                                     │
│  requests_total: AtomicU64                                          │
│  requests_in_flight: AtomicU64                                       │
│                                                                     │
│  NEW: state_counts: Arc<[AtomicU64; 7]>  ← per-state counters      │
│       pending_count, validating_count, processing_count,             │
│       awaiting_count, completed_count, failed_count, cancelled_count │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│                    RequestStateRegistry                              │
│                                                                     │
│  DashMap<Uuid, RequestStateData>                                    │
│                                                                     │
│  RequestStateData {                                                 │
│    state: AtomicU8 (packed RequestState),                           │
│    stage_entered_at: AtomicU64 (unix timestamp ms),                  │
│    total_duration: AtomicU64 (ms, set on terminal state),           │
│  }                                                                  │
│                                                                     │
│  Provides: snapshot() -> StateDistribution                          │
└─────────────────────────────────────────────────────────────────────┘
```

## Implementation Units

- [ ] **Unit 1: Define RequestState enum and state machine**

**Goal:** Define the state types and transition logic

**Requirements:** R1, R2

**Dependencies:** None

**Files:**
- Create: `src/gateway/middleware/request_state.rs`

**Approach:**
- Define `RequestState` enum with 7 states (Pending, Validating, Processing, AwaitingResponse, Completed, Failed, Cancelled)
- Implement `StateMachine` wrapper that only exposes valid transition methods per current state
- Use `TransitionError` for invalid transition attempts
- Pack state into `u8` for atomic storage

**Patterns to follow:**
- Rust enum state machine pattern from `rust-patterns` skill
- Exhaustive matching without wildcard branches

**Test scenarios:**
- Happy path: Valid transitions succeed (Pending→Validating, Processing→AwaitingResponse, etc.)
- Error path: Invalid transitions return TransitionError (Pending→Completed, etc.)
- Edge case: Concurrent transition attempts are handled safely via atomic operations

**Verification:**
- `cargo check -p alephcore` passes
- `cargo clippy -p alephcore -- -D warnings` passes
- All unit tests pass

---

- [ ] **Unit 2: Implement RequestStateRegistry**

**Goal:** Thread-safe registry for tracking all in-flight request states

**Requirements:** R3, R5, R6

**Dependencies:** Unit 1

**Files:**
- Create: `src/gateway/middleware/request_state.rs` (append registry code)

**Approach:**
- Use `DashMap<Uuid, RequestStateData>` for concurrent access
- `RequestStateData` holds atomic state, stage_entered_at, total_duration
- `RequestStateRegistry::insert()` - register new request in Pending state
- `RequestStateRegistry::transition()` - atomically advance state with validation
- `RequestStateRegistry::snapshot()` - return StateDistribution for introspection
- `RequestStateRegistry::complete()` - set terminal state and record total_duration

**Patterns to follow:**
- DashMap for concurrent HashMap access (already used elsewhere in Aleph)
- AtomicU64 for timestamp storage

**Test scenarios:**
- Happy path: Multiple concurrent requests can be registered and transitioned
- Edge case: Registry snapshot returns accurate state distribution
- Integration: State transitions are recorded correctly with timing

**Verification:**
- `cargo check -p alephcore` passes
- Unit tests for registry operations pass

---

- [ ] **Unit 3: Extend MetricsLayer with state distribution tracking**

**Goal:** Track per-state counts alongside existing metrics

**Requirements:** R3

**Dependencies:** Unit 1, Unit 2

**Files:**
- Modify: `src/gateway/middleware/metrics.rs`

**Approach:**
- Add `state_counts: Arc<[AtomicU64; 7]>` to MetricsLayer
- MetricsLayer now holds `Arc<RequestStateRegistry>` to update counts on transitions
- Each layer in the chain calls `registry.transition()` at entry/exit points
- Add `state_distribution()` method returning snapshot

**Patterns to follow:**
- Existing MetricsLayer pattern with Arc<AtomicU64> counters
- Clone pattern for sharing across layers

**Test scenarios:**
- Happy path: State counts increment/decrement correctly as requests move through stages
- Edge case: Multiple concurrent requests don't cause race conditions in counters
- Integration: MetricsLayer correctly wraps the full middleware chain

**Verification:**
- `cargo check -p alephcore` passes
- Existing metrics tests still pass
- New state tracking tests pass

---

- [ ] **Unit 4: Add backpressure thresholds and 429 responses**

**Goal:** Return 429 Too Many Requests when processing stage is saturated

**Requirements:** R4

**Dependencies:** Unit 3

**Files:**
- Modify: `src/gateway/middleware/rate_limit.rs` (or create backpressure layer)

**Approach:**
- Add `ProcessingThreshold` config (default: 100 concurrent processing requests)
- In MetricsLayer or dedicated BackpressureLayer, check `processing_count` before allowing request to enter Processing
- If threshold exceeded, return `JsonRpcResponse::error(429, "Processing capacity exceeded")`
- Configurable via `aleph.toml` under `[gateway.backpressure]`

**Patterns to follow:**
- RateLimitLayer pattern for returning 429 responses
- Configuration pattern from existing config types

**Test scenarios:**
- Happy path: Under threshold, requests proceed normally
- Edge case: At threshold, new requests receive 429 response
- Error path: Threshold reset when processing count drops

**Verification:**
- `cargo check -p alephcore` passes
- Backpressure triggers correctly at threshold
- Backpressure releases when capacity frees up

---

- [ ] **Unit 5: Add `request.state` RPC introspection endpoint**

**Goal:** Allow operators to inspect current state distribution

**Requirements:** R5

**Dependencies:** Unit 2

**Files:**
- Create: `src/gateway/handlers/request_state.rs`
- Modify: `src/gateway/handlers/mod.rs` (register handler)
- Modify: `src/gateway/middleware/mod.rs` (export registry)

**Approach:**
- Create `RequestStateHandler` implementing `HandlerSchema`
- `handle_with_params()` returns `StateSnapshot` via `RequestStateRegistry::snapshot()`
- Register as `request.state` method in HandlerRegistry
- Response shape:
  ```json
  {
    "pending": 5,
    "validating": 2,
    "processing": 10,
    "awaiting": 3,
    "completed": 142,
    "failed": 1,
    "cancelled": 0,
    "total_in_flight": 20,
    "timestamp": "2026-04-05T12:00:00Z"
  }
  ```

**Patterns to follow:**
- `health.rs` HandlerSchema pattern
- `health_summary()` pattern from previous work

**Test scenarios:**
- Happy path: Returns accurate snapshot of current state distribution
- Edge case: Empty registry returns all zeros
- Integration: Handler is correctly registered and accessible via JSON-RPC

**Verification:**
- `cargo check -p alephcore` passes
- Handler test passes with mock registry
- Endpoint responds correctly via curl/websocket

---

- [ ] **Unit 6: Integrate state tracking into MiddlewareChain**

**Goal:** Wire the state tracker into the middleware chain

**Requirements:** R1, R3

**Dependencies:** Units 1-5

**Files:**
- Modify: `src/gateway/middleware/chain.rs`

**Approach:**
- MiddlewareChain now holds `Arc<RequestStateRegistry>`
- Each layer (Trace, Metrics, Auth, RateLimit, Validate) calls `registry.transition()` at:
  - Entry: current_state → next_state
  - Exit: propagate state unchanged (or advance if handler completed)
- On request entry: `registry.insert()` with Pending state
- On request exit: `registry.complete()` or `registry.fail()` or `registry.cancel()`
- The MetricsLayer is the natural integration point since it's already first in chain

**Patterns to follow:**
- MiddlewareChain::new() pattern for constructing layered services
- Arc.clone() pattern for sharing state across layers

**Test scenarios:**
- Happy path: Full request lifecycle traces through all states correctly
- Integration: State distribution metrics update correctly through full chain
- Edge case: Error during middleware processing correctly transitions to Failed

**Verification:**
- `cargo check -p alephcore` passes
- Full integration test with simulated request flow
- Metrics reflect accurate state transitions

---

- [ ] **Unit 7: Add per-request timing tracking to state data**

**Goal:** Track how long requests spend in each stage

**Requirements:** R6

**Dependencies:** Unit 2

**Files:**
- Modify: `src/gateway/middleware/request_state.rs`

**Approach:**
- `RequestStateData` includes `stage_entered_at: AtomicU64` (unix timestamp ms)
- `total_duration: AtomicU64` (set when entering terminal state)
- Each transition updates `stage_entered_at` to current time
- On terminal transition, compute `total_duration = now - created_at`
- Export timing via `StateSnapshot` response

**Patterns to follow:**
- Existing timing tracking in MetricsLayer (elapsed_ms calculation)
- Atomic timestamp storage pattern

**Test scenarios:**
- Happy path: Timing is recorded correctly for each stage
- Edge case: Very fast transitions (sub-ms) are recorded accurately
- Integration: total_duration matches sum of stage durations

**Verification:**
- `cargo check -p alephcore` passes
- Timing data is accurate in snapshot

---

- [ ] **Unit 8: Add configuration and cleanup**

**Goal:** Make thresholds configurable and add registry cleanup for completed requests

**Requirements:** R4 (configuration)

**Dependencies:** Unit 4

**Files:**
- Modify: `src/config.rs` or relevant config file

**Approach:**
- Add `gateway.backpressure.processing_threshold` config (default: 100)
- Add `gateway.backpressure.awaiting_threshold` config (default: 50)
- Consider: Should completed/failed requests be removed from registry after N minutes?
- Add `RequestStateRegistry::cleanup()` for periodic cleanup of old terminal states

**Patterns to follow:**
- Configuration pattern from existing config types
- Background cleanup pattern from other Aleph components

**Test scenarios:**
- Happy path: Config values are read correctly
- Edge case: Invalid config values use defaults
- Integration: Cleanup removes old entries without affecting in-flight requests

**Verification:**
- `cargo check -p alephcore` passes
- Config changes apply at runtime
- Cleanup doesn't interfere with active requests

## System-Wide Impact

- **Interaction graph**: MiddlewareChain now holds `Arc<RequestStateRegistry>`. Each layer accesses registry to update state. Handlers don't directly interact but may call `registry.transition(AwaitingResponse)` for long-running operations.
- **Error propagation**: If state transition fails, request should still complete with error response. State tracking is informational, not gating (except for backpressure thresholds).
- **State lifecycle risks**: Terminal states (Completed/Failed/Cancelled) should eventually be cleaned up to prevent memory growth. Design includes `cleanup()` method.
- **API surface parity**: `request.state` is a new endpoint. No existing endpoints change behavior.
- **Integration coverage**: Full chain integration test with simulated requests through all stages will prove correctness.
- **Unchanged invariants**: HandlerRegistry dispatch unchanged. All existing handlers work without modification.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Performance overhead of state tracking | Use lock-free atomics for state updates; registry snapshot is O(1) for counter reads |
| Memory growth from registry | Add cleanup for terminal states; limit max in-flight tracked requests |
| Integration complexity | Start with MetricsLayer integration; add handlers last |
| Backpressure interaction with RateLimitLayer | Ensure backpressure runs before RateLimitLayer in chain |

## Documentation / Operational Notes

- Update `docs/reference/ARCHITECTURE.md` with request lifecycle states diagram
- Add `request.state` endpoint to OpenAPI schema (schema.openapi handler already exists)
- Operational runbook: If `processing_count` is consistently at threshold, scale processing capacity or adjust threshold

## Sources & References

- **Origin document:** `docs/brainstorms/2026-04-05-typestate-request-pipeline-requirements.md`
- Middleware architecture: `src/gateway/middleware/mod.rs`
- MetricsLayer: `src/gateway/middleware/metrics.rs`
- HandlerRegistry: `src/gateway/handlers/mod.rs`
- Rust state machine pattern: `rust-patterns` skill
