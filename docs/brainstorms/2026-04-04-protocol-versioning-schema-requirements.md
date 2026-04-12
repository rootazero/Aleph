---
date: 2026-04-04
topic: protocol-versioning-schema
---

# Protocol Versioning + Compile-Time Schema Generation

## Problem Frame

Aleph's WebSocket gateway uses untyped JSON-RPC (`params: Option<serde_json::Value>`) with no protocol version negotiation. This creates three problems with different urgencies:

1. **Zero validation (HIGH urgency)** — Server accepts any JSON as params, pushing type errors deep into handler logic instead of catching them at the boundary. Client receives untyped `Value` responses with no contract guarantee. Note: a `parse_params<T>` helper already exists (used in ~39 of 120+ handler files) that deserializes into typed structs — the infrastructure exists, but not all handlers use it and there is no compile-time enforcement.
2. **No evolution path (MEDIUM urgency)** — Any wire format change silently breaks clients. No mechanism to detect version mismatch between server and client (webchat, CLI, Telegram bridge). Note: `GatewayConfig` already has a `protocol_version: u32` field (defaulting to 1) and `HelloParams` sends a string `version: "1"`, but neither is used for handshake negotiation.
3. **No API documentation (LOW urgency)** — 120+ RPC handlers exist with no machine-readable schema. New client development requires reading Rust handler source code.

Aleph has a unique Rust advantage: `schemars` (already in Cargo.toml) can derive JSON Schema from the actual Rust types at compile time — the schema and the code are structurally identical. OpenClaw uses runtime AJV validation that can drift from actual TypeScript types; Aleph's approach makes drift impossible.

## Phased Delivery

Each phase delivers independent value. Phases can be planned and shipped separately.

```
Phase 1: Typed Handlers          Phase 2: Version Handshake       Phase 3: Schema Export
(R1-R8, immediate value)         (R9-R12, when Phase 1 done)      (R13-R15, when consumers exist)
                                                                   
  Option<Value> → typed structs    connect adds version field       CLI export + codegen pipeline
  parse_params<T> enforced         mismatch → clear error           TypeScript types from schema
  incremental migration            additive-only evolution          API documentation
```

## Requirements

### Phase 1: Typed Request/Response Contracts

- R1. Each RPC method defines a concrete Rust struct for its Params and Result types (replacing `Option<Value>`). These structs derive `Serialize`, `Deserialize`, and `schemars::JsonSchema`.
- R2. The handler dispatch deserializes incoming `params` into the method's concrete Params type. Deserialization failure returns `INVALID_PARAMS` (-32602) before the handler is invoked. Error messages returned to clients MUST be sanitized — include field name and expected type but strip internal Rust type paths. Full serde error is logged server-side only.
- R3. Handler functions accept their concrete Params type and return their concrete Result type. The dispatch layer handles serialization/deserialization uniformly.
- R4. Methods with no params use a unit struct or `()` that deserializes from null/missing params. Methods with optional fields use `Option<T>` with `#[serde(default)]`.
- R5. Incoming request params are validated structurally by serde deserialization into the typed struct — this IS the structural validation (correct types, required fields). No separate schema matching at runtime. Semantic validation (value ranges, cross-field invariants, business rules) remains in the handler body.
- R6. Typed and untyped handlers MUST coexist during migration. The dispatch layer supports both — typed handlers use the new `TypedHandler<P,R>` pattern, legacy handlers continue using `HandlerFn` with `Option<Value>`. No new untyped handlers may be added after Phase 1 infrastructure is in place.
- R7. Unknown fields in params are silently ignored by default during migration (`#[serde(default)]`). Individual handlers MAY opt in to `#[serde(deny_unknown_fields)]` when their client surface is fully migrated.
- R8. Migration proceeds incrementally: stateless/simple handlers first, then auth handlers, then stateful handlers. Each handler can be typed independently without blocking others.

### Phase 2: Protocol Version Handshake

- R9. The `connect` message MUST include a `protocol_version: u32` field in params. Version validation occurs within the connect handler (after message acceptance, before full authentication completes). On mismatch, the server rejects the connection with a typed error and closes the WebSocket. Version mismatch counts toward the existing `auth_attempts` counter.
- R10. Protocol version is a single monotonic integer (not semver). The version bumps only for breaking wire format changes. Additive changes (new optional fields with `serde(default)`) do NOT require a version bump — they are backward compatible by design.
- R11. Version mismatch produces a clear JSON-RPC error (`PROTOCOL_VERSION_MISMATCH`, code -32020) with `server_version` and `min_supported_version` in the error data, so clients can display actionable upgrade messages.
- R12. The server's protocol version is derived from the version constant in `shared/protocol`, not hardcoded per handler.

### Phase 3: Schema Generation & Export

- R13. A CLI command (`aleph-server schema export`) generates the complete JSON Schema for all typed RPC methods as a single JSON document, organized by method name → { params_schema, result_schema }. Methods still on untyped params are listed with a `"typed": false` marker.
- R14. The schema export includes protocol version, method names, and human-readable descriptions (via `schemars` `#[schemars(description = "...")]`).
- R15. (Conditional) If a TypeScript consumer exists, the exported schema is used to auto-generate TypeScript type definitions. The generation pipeline (schema → .d.ts) runs as a build step, not manually. This requirement activates only when a confirmed consumer is identified.

## Success Criteria

### Phase 1
- Dispatch layer supports both typed and untyped handlers simultaneously
- All new handlers use typed Params/Result structs — zero new `Option<Value>` additions
- A client sending wrong param types gets `INVALID_PARAMS` with a sanitized, useful error message
- At least the top 20 highest-traffic handlers are migrated to typed structs

### Phase 2
- Protocol version mismatch between server and client produces a clear, actionable error
- Adding a new optional field to a handler does NOT require a version bump
- Existing `GatewayConfig.protocol_version` is the authoritative source

### Phase 3
- `aleph-server schema export` produces valid JSON Schema
- Schema accurately reflects all typed handlers

## Scope Boundaries

- **Not in scope:** Multiple protocol version support — clean break on version bump, all clients upgrade together
- **Not in scope:** Binary protocol or non-JSON wire format
- **Not in scope:** Refactoring handler dispatch to auto-registration (that's ideation #9, a separate effort)
- **Not in scope:** WebSocket transport changes (compression, subprotocols)
- **Deferred:** Browsable HTML API documentation from schema
- **Deferred:** TypeScript codegen — activates when a consumer is confirmed (webchat is Rust/WASM, CLI is Rust)

## Key Decisions

- **Phased delivery** over monolithic migration: Typed handlers deliver immediate validation value; versioning and schema export can follow independently.
- **Additive-only evolution** as default: New optional fields with `serde(default)` are backward compatible, no version bump required. Version bumps reserved for breaking changes only.
- **Permissive parsing during migration** over `deny_unknown_fields`: Strict field rejection is opt-in per-handler to avoid breaking existing clients during incremental migration.
- **Single monotonic version** over semver: Simpler, no ambiguity about breaking vs non-breaking. Single user project doesn't need semver complexity.
- **Serde deserialization as structural validation** over separate schema validation: Rust's type system + serde IS the structural validator. Semantic validation stays in handler bodies.
- **Error message sanitization**: Client-facing errors include field/type info but strip Rust internal paths. Full errors logged server-side.

## Outstanding Questions

### Deferred to Planning

- [Affects R1][Needs research] Of the 120+ registered handlers, how many already use `parse_params<T>` with typed structs vs raw `Option<Value>`? What's the migration order?
- [Affects R3][Technical] Should the dispatch layer use a `TypedHandler<P, R>` trait with blanket impl wrapping into `HandlerFn`, or a `#[rpc_handler("method.name")]` macro? The trait approach preserves backward compat; the macro is more ergonomic.
- [Affects R13][Technical] Should schema export be a compile-time artifact (build.rs) or a runtime CLI subcommand? Build.rs ensures schema is always fresh; CLI is simpler to implement.
- [Affects R1][Technical] How to reconcile the dual `JsonRpcRequest` types (shared/protocol vs gateway-local)? Both have `params: Option<Value>`. Typed params should live at the dispatch layer while wire-level `JsonRpcRequest` stays untyped.
- [Affects R13][Technical] Schema registry — extend `HandlerRegistry` to store optional `(RootSchema, RootSchema)` per method via a `register_typed` method that generates schemas at registration time.

## Next Steps

→ `/ce:plan` for Phase 1 (Typed Request/Response Contracts) implementation planning
