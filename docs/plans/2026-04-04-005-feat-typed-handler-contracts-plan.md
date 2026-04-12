---
title: "feat: Typed handler request/response contracts"
type: feat
status: completed
date: 2026-04-04
origin: docs/brainstorms/2026-04-04-protocol-versioning-schema-requirements.md
---

# feat: Typed Handler Request/Response Contracts

## Overview

Migrate gateway handlers from untyped `Option<serde_json::Value>` params to concrete Rust structs with compile-time schema derivation. This is Phase 1 of the Protocol Versioning plan — it delivers immediate validation value independently of versioning or schema export.

## Problem Frame

The gateway's 122 registered handlers accept `JsonRpcRequest` with `params: Option<Value>`. Only ~39 files use the existing `parse_params<T>` helper for typed deserialization. The remaining handlers manually destructure raw JSON or ignore params entirely. This pushes type errors deep into handler logic, produces unhelpful error messages, and provides no compile-time contract enforcement. (see origin: docs/brainstorms/2026-04-04-protocol-versioning-schema-requirements.md)

## Requirements Trace

- R1. Concrete Params/Result structs deriving `Serialize`, `Deserialize` (`JsonSchema` deferred to Phase 3 — adding the derive later is mechanical and additive)
- R2. Dispatch-layer deserialization with sanitized `INVALID_PARAMS` errors
- R3. Typed handler signatures; dispatch handles ser/de uniformly
- R4. No-params methods use default-deserializable struct; optional fields use `Option<T>` with `serde(default)`
- R5. Serde deserialization IS structural validation; semantic validation in handler body
- R6. Typed and untyped handlers coexist during migration
- R7. Permissive parsing by default; `deny_unknown_fields` opt-in per-handler
- R8. Incremental migration: stateless → auth → stateful

## Scope Boundaries

- **In scope:** TypedHandler trait, sanitized error formatting, incremental handler migration
- **Not in scope:** Handler auto-registration via inventory/linkme (ideation #9)
- **Not in scope:** Protocol version handshake (Phase 2)
- **Not in scope:** Schema export CLI command (Phase 3)
- **Not in scope:** Refactoring HandlerRegistry's HashMap dispatch

## Context & Research

### Relevant Code and Patterns

- `src/gateway/handlers/mod.rs:149-172` — `parse_params<T>` helper, the existing typed deserialization pattern
- `src/gateway/handlers/mod.rs:175-177` — `HandlerFn` type alias: `Arc<dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send>> + Send + Sync>`
- `src/gateway/handlers/mod.rs:661-670` — `HandlerRegistry::register<F, Fut>(method, handler)` accepting closures
- `src/gateway/handlers/logs.rs` — Good migration example: `SetLevelParams` struct + parse_params
- `src/gateway/handlers/mcp.rs` — Good example: multiple typed param structs
- `src/gateway/handlers/echo.rs` — Migration target: raw `request.params` usage
- `src/gateway/handlers/health.rs` — Migration target: no params, ignores request
- `src/gateway/protocol.rs` — Gateway-local `JsonRpcRequest` (canonical for handlers)
- `shared/protocol/src/jsonrpc.rs` ��� Shared protocol types (wire-level, stays untyped)

## Key Technical Decisions

- **Closure-based `register_typed` function** over trait-based dispatch: A generic `register_typed<P, R, F, Fut>(method, closure)` wraps typed closures into the existing `HandlerFn` signature. The wrapper performs deserialization, calls the closure, and serializes the result. This matches the existing codebase pattern where handlers are closures capturing `Arc` state — no need to restructure handlers into trait-impl structs. Preserves full backward compatibility. (see origin: R3, R6)

- **Wire-level JsonRpcRequest stays untyped** over dual-type unification: The gateway-local `JsonRpcRequest` (with `params: Option<Value>`) remains the wire deserialization target. Typed deserialization happens inside the `TypedHandler` wrapper, not at the transport layer. This avoids touching the WebSocket handler or any non-gateway consumer of the shared protocol crate. (see origin: Outstanding Questions)

- **Sanitized error wrapper** over raw serde messages: Replace `format!("Invalid params: {}", e)` in `parse_params` with a sanitizer that extracts field name and expected type from serde errors but strips Rust internal type paths (e.g., `alephcore::gateway::handlers::logs::SetLevelParams`). Full error logged at `warn` level server-side. (see origin: R2)

- **EmptyParams default struct** over special-casing `()`: Methods with no params use `#[derive(Default, Deserialize, Serialize, JsonSchema)] struct EmptyParams {}` which deserializes from `null`, `{}`, or missing params. This is simpler than special-casing unit type in the dispatch layer. (see origin: R4)

## Open Questions

### Resolved During Planning

- **TypedHandler vs macro pattern:** TypedHandler trait chosen — preserves backward compat, no build-time magic, allows incremental adoption.
- **How to handle no-params methods:** EmptyParams struct with `#[derive(Default)]` that accepts null/missing.
- **Where typed deserialization lives:** In the TypedHandler wrapper, not in parse_params or the transport layer.

### Deferred to Implementation

- Exact list of which handlers need migration vs already use typed params — discoverable by grepping for `parse_params` usage patterns.
- Whether some handlers need custom error formatting beyond the default sanitizer.

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```
                      JsonRpcRequest (untyped, wire-level)
                              │
                     HandlerRegistry.handle()
                              │
                   ┌──────────┴──────────┐
                   │                     │
            TypedHandler<P,R>      Legacy HandlerFn
            (new pattern)          (unchanged)
                   │                     │
         ┌────────┴────────┐             │
         │ deserialize P   │    request.params: Option<Value>
         │ from params     │             │
         │     │           │             │
         │ sanitize error  │             │
         │ on failure      │             │
         │     │           │             │
         │ call handler(P) │             │
         │     │           │             │
         │ serialize R     │             │
         │ into response   │             │
         └───���────┬────────┘             │
                  │                      │
                  └──────────┬───────────┘
                             │
                      JsonRpcResponse
```

**Closure-based register_typed (directional):**
```
// Closure-based approach -- matches existing codebase pattern where
// handlers are closures capturing Arc state. NOT a trait-based approach.

HandlerRegistry::register_typed<P, R, F, Fut>(method, handler_fn)
where
    P: DeserializeOwned + Serialize,  // JsonSchema deferred to Phase 3
    R: Serialize,
    F: Fn(P) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<R, HandlerError>> + Send + 'static

// Stateful handler example (closure captures Arc state):
let agent_mgr = Arc::clone(&agent_manager);
registry.register_typed::<RunParams, RunResult, _, _>("agent.run", move |params| {
    let mgr = Arc::clone(&agent_mgr);
    async move { handle_agent_run(params, mgr).await }
});

// HandlerError carries JSON-RPC error code for semantic validation errors:
struct HandlerError { code: i32, message: String }
```

```
// The wrapper (inside register_typed) performs:
// 1. Convert params: None → Value::Object(empty) for null/missing tolerance
// 2. serde_json::from_value::<P>(params) with sanitized error on failure
// 3. Call handler closure with typed P
// 4. Serialize Result<R, HandlerError> into JsonRpcResponse
```

**Sanitized error (directional):**
```
Input serde error:  "invalid type: string "abc", expected u32 at field `count` 
                     for alephcore::gateway::handlers::logs::SetLevelParams"
Output to client:   {"field": "count", "expected": "u32", "actual": "string"}
Server log (warn):  full serde error with Rust type path
```

## Implementation Units

- [ ] **Unit 1: TypedHandler trait and wrapper infrastructure**

**Goal:** Define the TypedHandler trait and its blanket impl that wraps typed handlers into the existing HandlerFn signature.

**Requirements:** R3, R6

**Dependencies:** None

**Files:**
- Create: `src/gateway/handlers/typed.rs`
- Modify: `src/gateway/handlers/mod.rs` (add module, add `register_typed` to HandlerRegistry)
- Test: `src/gateway/handlers/typed_tests.rs`

**Approach:**
- Define `TypedHandler` trait with associated types `Params` and `Result`
- Implement an `into_handler_fn()` method or `From<T> for HandlerFn` that: (1) deserializes params, (2) calls typed handler, (3) serializes result
- Add `HandlerRegistry::register_typed<H: TypedHandler>(method, handler)` that wraps and delegates to existing `register()`
- EmptyParams struct for no-param methods: `#[derive(Default, Deserialize, Serialize)] pub struct EmptyParams {}`
- `HandlerError` struct with `code: i32` and `message: String` so handlers can return specific JSON-RPC error codes for semantic validation
- `register_typed` must support stateful closures that capture `Arc<T>` state — this is the dominant handler pattern (~250 of ~350 handlers)
- Handle `params: None` gracefully — wrapper converts `None` and `Some(Value::Null)` to `Value::Object(Default::default())` before deserialization, so EmptyParams and optional-only structs accept null/missing params

**Patterns to follow:**
- `src/gateway/handlers/mod.rs:661-670` — existing register() pattern
- `src/gateway/handlers/logs.rs:35-45` — existing typed params pattern

**Test scenarios:**
- Happy path: TypedHandler with valid params deserializes and returns typed result
- Happy path: TypedHandler with EmptyParams accepts null, missing, and empty object params
- Edge case: Optional fields with `serde(default)` accept missing fields
- Error path: Invalid param type returns INVALID_PARAMS (-32602) with sanitized error
- Error path: Missing required field returns INVALID_PARAMS with field name in error
- Error path: Extra unknown fields are silently ignored (no deny_unknown_fields by default)
- Integration: register_typed() produces a handler that dispatch can route like any HandlerFn
- Integration: Stateful closure capturing Arc<T> state works with register_typed — handler receives typed params AND has access to captured state

**Verification:**
- TypedHandler compiles alongside existing HandlerFn handlers
- Both typed and untyped handlers coexist in the same HandlerRegistry
- Deserialization errors produce sanitized client messages without Rust type paths

---

- [ ] **Unit 2: Error message sanitizer**

**Goal:** Replace raw `format!("Invalid params: {}", e)` with a sanitizer that extracts structured error info.

**Requirements:** R2

**Dependencies:** Unit 1

**Files:**
- Create: `src/gateway/handlers/error_sanitizer.rs`
- Modify: `src/gateway/handlers/mod.rs` (update parse_params to use sanitizer)
- Test: `src/gateway/handlers/error_sanitizer_tests.rs`

**Approach:**
- Use `serde_path_to_error` crate (wraps serde deserializer to capture exact field path) instead of regex-parsing error strings. This provides stable, structured access to the failing field path regardless of serde_json version.
- Build structured JSON error data: `{"field": "...", "expected": "...", "actual": "..."}`
- Fallback: if field/type extraction fails, return generic `{"error": "invalid params"}` with no field detail. Strip any substring containing `::` (Rust module paths) as a safety floor.
- Log full original serde error at `warn!` level with method name context
- Update existing `parse_params<T>` to use the sanitizer (backward compat for legacy callers)

**Patterns to follow:**
- `src/gateway/handlers/mod.rs:149-172` — existing parse_params error formatting

**Test scenarios:**
- Happy path: Type mismatch error → structured JSON with field, expected, actual
- Happy path: Missing field error → structured JSON with field name
- Edge case: Nested field path (e.g., `config.timeout`) correctly extracted
- Edge case: Unparseable serde error → generic fallback message
- Error path: Error message contains no Rust module paths (regression test with known type path)
- Integration: Full round-trip — send bad params via JsonRpcRequest, verify sanitized response

**Verification:**
- No Rust type paths (containing `::`) appear in any client-facing error message
- Server logs contain full serde error for debugging

---

- [ ] **Unit 3: Migrate stateless simple handlers (Wave 1)**

**Goal:** Convert the simplest, stateless handlers to use TypedHandler pattern as proof of migration.

**Requirements:** R1, R4, R7, R8

**Dependencies:** Unit 1, Unit 2

**Files:**
- Modify: `src/gateway/handlers/echo.rs`
- Modify: `src/gateway/handlers/health.rs`
- Modify: `src/gateway/handlers/version.rs`
- Modify: `src/gateway/handlers/logs.rs` (already typed — add JsonSchema derive)
- Modify: `src/gateway/handlers/mod.rs` (switch registrations to register_typed)
- Test: existing test files for each handler

**Approach:**
- For each handler: (1) define Params/Result structs with `Serialize, Deserialize, JsonSchema` derives, (2) rewrite handler to accept typed Params, (3) switch registration from `register()` to `register_typed()`
- echo: `EchoParams { #[serde(default)] data: Option<Value> }` → `EchoResult { echo: Option<Value> }` (preserves existing behavior where missing params returns `{"echo": null}`)
- health: `EmptyParams` → `HealthResult { status, timestamp, ... }`
- version: `EmptyParams` → `VersionResult { version, ... }`
- logs (SetLevelParams): add `JsonSchema` derive, switch to register_typed

**Patterns to follow:**
- `src/gateway/handlers/logs.rs` — existing typed handler pattern (add JsonSchema derive)
- `src/gateway/handlers/mcp.rs` — multiple params structs pattern

**Test scenarios:**
- Happy path: echo with `{"data": "hello"}` returns `{"echo": "hello"}`
- Happy path: health with no params returns status JSON
- Happy path: version with no params returns version string
- Edge case: echo with missing params returns `{"echo": null}` (preserves existing test_echo_without_params behavior)
- Edge case: health called with unexpected params (extra fields) still succeeds (permissive)
- Integration: Full WebSocket round-trip for each migrated handler

**Verification:**
- All 4 handlers compile and pass existing tests
- New handler registrations use `register_typed`
- Handlers that previously accepted raw params now validate types at dispatch

---

- [ ] **Unit 4: Migrate high-traffic handlers (Wave 2)**

**Goal:** Convert the most commonly used handlers — config, session, agent, chat.

**Requirements:** R1, R5, R8

**Dependencies:** Unit 3 (proves the pattern works)

**Execution note:** Execution target: external-delegate — this is mechanical migration work following the proven pattern from Unit 3.

**Files:**
- Modify: `src/gateway/handlers/config.rs` (get, set, list handlers)
- Modify: `src/gateway/handlers/session_usage.rs` (session-related handlers)
- Modify: `src/gateway/handlers/agent.rs` (run, cancel, status handlers)
- Modify: `src/gateway/handlers/chat.rs` (send, history handlers)
- Modify: `src/gateway/handlers/mod.rs` (switch registrations)
- Test: existing test files for each handler module

**Approach:**
- Follow the same pattern from Unit 3 for each handler module
- Where handlers already use `parse_params<T>` with typed structs, only add `JsonSchema` derive and switch to `register_typed`
- Where handlers use raw params, define new Params/Result structs
- Preserve all existing semantic validation in handler bodies (R5)
- Do NOT add `deny_unknown_fields` — permissive by default during migration (R7)

**Patterns to follow:**
- Unit 3's migrated handlers as the canonical pattern
- Each handler module's existing Params struct (if any) as the starting point

**Test scenarios:**
- Happy path: Each migrated handler works with valid params
- Error path: Each migrated handler rejects wrong param types with sanitized error
- Edge case: Handlers with optional params accept both present and missing fields
- Edge case: Handlers that perform semantic validation (e.g., agent.run checks agent_id exists) still validate in handler body, not at dispatch
- Integration: Existing integration tests continue passing without modification

**Verification:**
- All migrated handlers compile with typed signatures
- `cargo test` passes with no regressions
- At least 20 high-traffic handlers migrated (per success criteria)

---

- [ ] **Unit 5: Enforcement — prevent new untyped handlers**

**Goal:** Add a compile-time or CI check that prevents adding new handlers using the untyped `register()` path.

**Requirements:** R6

**Dependencies:** Unit 3

**Files:**
- Modify: `src/gateway/handlers/mod.rs` (deprecate `register()` or add lint)

**Approach:**
- Add `#[deprecated(note = "Use register_typed() for new handlers")]` to `HandlerRegistry::register()`
- This produces compiler warnings for any new usage while keeping existing untyped handlers working
- Alternatively, rename `register()` to `register_legacy()` to make the intent explicit
- Document in code comment: "All new handlers must use register_typed(). Legacy handlers will be migrated incrementally."

**Test expectation:** none — pure enforcement annotation, no behavioral change.

**Verification:**
- Calling `register()` for a new handler produces a deprecation warning
- Existing `register()` calls continue to compile (warnings, not errors)

## System-Wide Impact

- **Interaction graph:** The TypedHandler wrapper sits between HandlerRegistry dispatch and individual handlers. No change to WebSocket transport, event emitter, or channel layer. `parse_params<T>` callers in legacy handlers continue working unchanged.
- **Error propagation:** Deserialization errors are now caught uniformly at dispatch (before handler invocation) rather than scattered across handlers. This changes error timing for handlers that previously accepted bad params and failed later.
- **State lifecycle risks:** None — this is a request/response path change, no persistent state affected.
- **API surface parity:** All interfaces (webchat, CLI, Telegram, unix socket, stdio) send JSON-RPC through the same gateway. Typed validation applies uniformly.
- **Integration coverage:** The TypedHandler wrapper must be tested with actual JsonRpcRequest objects, not just unit-tested in isolation, to prove the dispatch integration works.
- **Unchanged invariants:** `JsonRpcRequest` wire format is unchanged. `JsonRpcResponse` format is unchanged. WebSocket handler is unchanged. `HandlerRegistry::handle()` dispatch path is unchanged.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Existing tests rely on sending malformed params that now fail at dispatch | Run full test suite after each wave; fix tests that send intentionally bad params |
| Handler closures that capture runtime state (e.g., `McpManagerHandle`) need adaptation for TypedHandler | TypedHandler trait design must accommodate closures with captured state, not just free functions |
| Sanitizer regex for serde error parsing may miss edge cases | Fallback to generic message; full error in server logs ensures debugging path |
| Migration wave 2 (Unit 4) is large — risk of partial completion | Each handler migrates independently; partial completion is acceptable |

## Sources & References

- **Origin document:** [docs/brainstorms/2026-04-04-protocol-versioning-schema-requirements.md](docs/brainstorms/2026-04-04-protocol-versioning-schema-requirements.md)
- Related code: `src/gateway/handlers/mod.rs` (HandlerRegistry, parse_params, HandlerFn)
- Related code: `src/gateway/protocol.rs` (gateway-local JsonRpcRequest)
- Related ideation: `docs/ideation/2026-04-04-gateway-openclaw-comparison-ideation.md` (#9 RPC Schema Registry — future work)
