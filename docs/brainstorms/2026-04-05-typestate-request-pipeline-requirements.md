---
date: 2026-04-05
topic: typestate-request-pipeline
---

# Typestate Request Pipeline

## Problem Frame

The current middleware chain handles JSON-RPC requests but lacks explicit lifecycle state tracking. When a request enters the system, we increment `requests_in_flight`, but we don't track which stage it's in (validating, processing, awaiting response). This makes it impossible to:
- Detect when the system is saturated at a specific stage
- Provide meaningful state feedback to operators
- Apply backpressure based on processing depth

OpenClaw's RunStateMachine tracks `busy`/`activeRuns` but uses simple booleans. Aleph can do better with Rust's typestate pattern — making illegal state transitions impossible at compile time.

## Requirements

**R1.** Implement request lifecycle as typestates with compile-time transitions:
- `Pending` → `Validating` → `Processing` → `AwaitingResponse` → `Completed`
- Error path: any state → `Failed`
- Cancellation: any state → `Cancelled`

**R2.** Add `RequestState` enum with states: `Pending`, `Validating`, `Processing`, `AwaitingResponse`, `Completed`, `Failed`, `Cancelled`

**R3.** Extend `MetricsLayer` to track state distribution:
- `pending_count`: requests not yet being processed
- `validating_count`: requests in validation
- `processing_count`: requests being handled
- `awaiting_count`: requests waiting for external resources (LLM calls, etc.)

**R4.** Add backpressure signals:
- When `processing_count` exceeds threshold, return `429 Too Many Requests`
- Configurable thresholds per state

**R5.** Provide state introspection RPC endpoint:
- `request.state` - returns current state distribution across all in-flight requests

**R6.** Track per-request timing:
- `stage_entered_at`: when request entered current state
- `total_duration`: from Pending to terminal state

## Success Criteria

- [ ] Compile-time enforcement: Cannot call `complete()` on a `Pending` request
- [ ] Metrics reflect true state distribution
- [ ] Backpressure triggers when thresholds exceeded
- [ ] State introspection returns accurate snapshot
- [ ] Existing middleware chain continues to work unchanged

## Scope Boundaries

- The typestate is for **request lifecycle**, not media pipeline (MessagePipeline is separate)
- Does not change how handlers are dispatched — only adds tracking
- Does not replace existing error handling — supplements it with state tracking

## Key Decisions

**Decision**: Use typestate pattern via Rust type system rather than simple enum state
- Rationale: Compile-time invalid transition prevention is stronger guarantee than runtime checks
- Tradeoff: More complex types, but catches bugs at compile time

**Decision**: States track middleware processing stages, not business logic
- Rationale: Aligns with existing MiddlewareChain architecture
- Middleware stages: parse → validate → auth → rate_limit → handle

## Dependencies / Assumptions

- Assumes `MiddlewareChain` architecture remains stable
- Uses existing `MetricsLayer` infrastructure, extends rather than replaces

## Outstanding Questions

### Deferred to Planning
- **[Technical]** How to integrate typestate with the Tower `Service` pattern? Each middleware layer would need to be aware of state transitions.
- **[Technical]** Should AwaitingResponse represent awaiting LLM response, or is that abstracted away?
- **[Technical]** How to handle concurrent requests sharing state? Need thread-safe state registry.

### Next Steps
→ `/ce:plan` for structured implementation planning
