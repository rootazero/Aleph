# Compression Service Wiring Design

> Wire the existing `CompressionService` into server startup so Layer 2 facts are actually produced from Layer 1 memories.

**Date**: 2026-03-07
**Status**: Approved

---

## Problem

`CompressionService` is fully implemented (LLM fact extraction, conflict detection, storage) but never instantiated or called. The `handle_compress` RPC handler is a stub returning zeros. No background task runs. Result: Layer 2 facts table is always empty.

## Approach

**Independent initialization function** (`init_compression_service`) at the `run_server()` level, parallel to existing service initialization patterns. Avoids bloating `register_agent_handlers()`.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| AI/Embedding provider | Reuse default providers, independent config later | YAGNI |
| Turn counting | `ExecutionEngine` holds `Arc<CompressionService>` directly | Matches existing `Option<MemoryBackend>` pattern; EventBus is over-engineering for 1:1 relationship |
| Trigger modes | All three: periodic (1h), turn threshold (20), manual RPC | User requested full wiring |
| Degradation | `Option<Arc<CompressionService>>` — None when providers unavailable | Graceful, no panic |

## Data Flow

```
Server Startup
    |
    +- init embedding provider (lifted to outer scope)
    |       -> Option<Arc<dyn EmbeddingProvider>>
    |
    +- init_compression_service(memory_db, default_provider, embedder, config)
    |       -> Option<Arc<CompressionService>>
    |       -> spawns background task (hourly check loop)
    |
    +- register_memory_handlers(server, memory_db, compression_service)
    |       -> memory.compress wired to real CompressionService
    |
    +- ExecutionEngine::new(...).with_compression_service(cs)
            -> conversation complete -> record_turn_and_check()

Runtime Triggers:
    1. Turn threshold (20 turns) -> immediate compress()
    2. Background loop (1h) -> check idle_timeout + pending_turns -> compress()
    3. Manual RPC (memory.compress) -> compress()
```

## Code Changes

### File 1: `core/src/bin/aleph/commands/start/mod.rs`

**Lift embedding provider initialization** from inside `register_agent_handlers()` to `run_server()` scope. Create a standalone `init_embedding_provider()` async function. Pass `embedder` as parameter to `register_agent_handlers()`.

**Create CompressionService** after provider_registry is available:

```rust
let compression_service: Option<Arc<CompressionService>> = match (&embedder, default_provider) {
    (Some(emb), Some(prov)) => Some(init_compression_service(...)),
    _ => { warn!("Compression disabled: missing provider"); None }
};
```

**Inject into ExecutionEngine**:

```rust
if let Some(ref cs) = compression_service {
    engine = engine.with_compression_service(cs.clone());
}
```

### File 2: `core/src/bin/aleph/commands/start/builder/handlers.rs`

**New function**: `init_compression_service()` — creates `CompressionService`, spawns background task, returns `Arc`.

**Modify**: `register_memory_handlers()` — accept `Option<Arc<CompressionService>>`, wire `memory.compress` to real handler or error stub.

### File 3: `core/src/gateway/handlers/memory.rs`

**Replace stub**: `handle_compress` takes `Arc<CompressionService>`, calls `service.compress().await`.

### File 4: `core/src/gateway/execution_engine/engine.rs`

**New field**: `compression_service: Option<Arc<CompressionService>>`

**New builder method**: `with_compression_service()`

**Turn counting**: After `write_conversation_memory()`, call `compression_service.record_turn_and_check()` in a spawned task.

## Degradation

| Scenario | Behavior |
|----------|----------|
| No embedding/AI provider configured | `compression_service = None`, no background task, RPC returns error |
| LLM call failure (network/rate limit) | `compress()` returns Err, background loop logs warning, continues next cycle |
| Empty database (no Layer 1) | `compress()` returns Ok, 0 facts extracted |
| Concurrent compression (manual + auto) | Guard with internal mutex to prevent duplicate processing |

## Not In Scope

- `DreamDaemon` changes (runs independently, no conflict)
- `HybridTrigger` integration (future chat-aware compression)
- Config hot-reload (restart to apply changes)
- `MemoryCommandHandler` event sourcing (optional, not wired)
- Independent compression provider config (future enhancement)

## Files Affected

| File | Change |
|------|--------|
| `core/src/bin/aleph/commands/start/mod.rs` | Lift embedder, create service, inject engine |
| `core/src/bin/aleph/commands/start/builder/handlers.rs` | `init_compression_service()`, modify `register_memory_handlers` |
| `core/src/gateway/handlers/memory.rs` | Replace `handle_compress` stub |
| `core/src/gateway/execution_engine/engine.rs` | Add field + builder + turn counting |
