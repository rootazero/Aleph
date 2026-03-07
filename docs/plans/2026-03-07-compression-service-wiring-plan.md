# Compression Service Wiring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the existing `CompressionService` into server startup so Layer 2 facts are produced from Layer 1 memories via three trigger paths (periodic, turn-threshold, manual RPC).

**Architecture:** Independent `init_compression_service()` function at the `run_server()` scope. Embedding provider initialization is lifted from `register_agent_handlers()` to outer scope for reuse. `ExecutionEngine` gains `Option<Arc<CompressionService>>` for turn counting.

**Tech Stack:** Rust, Tokio (async), LanceDB, LLM providers (OpenAI-compatible)

**Design doc:** `docs/plans/2026-03-07-compression-service-wiring-design.md`

---

### Task 1: Add `CompressionConfig::from_policy` constructor

**Files:**
- Modify: `core/src/memory/compression/service.rs:28-49`

**Step 1: Add the method**

In `service.rs`, add `from_policy` to `CompressionConfig` (after the `Default` impl, ~line 49):

```rust
impl CompressionConfig {
    /// Create from config policy
    pub fn from_policy(policy: &crate::config::CompressionPolicy) -> Self {
        Self {
            batch_size: 50,
            scheduler: SchedulerConfig::from_policy(policy),
            conflict: ConflictConfig::default(),
            background_interval_seconds: policy.background_interval_seconds,
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add core/src/memory/compression/service.rs
git commit -m "compression: add CompressionConfig::from_policy constructor"
```

---

### Task 2: Add `compression_service` field to `ExecutionEngine`

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs:33-95`

**Step 1: Add field and builder method**

In `ExecutionEngine` struct (after `task_router` field, line 52):

```rust
    /// Compression service for turn-based fact extraction
    compression_service: Option<Arc<crate::memory::compression::CompressionService>>,
```

In `ExecutionEngine::new()` (line 65, add to Self initializer):

```rust
            compression_service: None,
```

After `with_workspace_manager` method (~line 95), add:

```rust
    /// Set a compression service for automatic turn-based compression.
    pub fn with_compression_service(
        mut self,
        service: Arc<crate::memory::compression::CompressionService>,
    ) -> Self {
        self.compression_service = Some(service);
        self
    }
```

**Step 2: Add turn counting after memory write**

In the `execute_run` method, after the memory write block (after line 370 `}`), add:

```rust
                // Record conversation turn for compression scheduling
                if let Some(ref cs) = self.compression_service {
                    let cs = cs.clone();
                    tokio::spawn(async move {
                        cs.record_turn_and_check();
                    });
                }
```

Note: `record_turn_and_check` takes `&Arc<Self>` so the clone provides the Arc. It internally spawns compression if threshold is reached — the outer `tokio::spawn` just ensures we don't block the response path.

**Step 3: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compiles (possibly with unused warnings, that's OK)

**Step 4: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: add compression_service field and turn counting"
```

---

### Task 3: Replace `handle_compress` stub with real implementation

**Files:**
- Modify: `core/src/gateway/handlers/memory.rs:270-283`

**Step 1: Replace the stub**

Replace the entire `handle_compress` function (lines 271-283) with:

```rust
/// Trigger memory compression
pub async fn handle_compress(
    request: JsonRpcRequest,
    service: Arc<crate::memory::compression::CompressionService>,
) -> JsonRpcResponse {
    match service.compress().await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "memoriesProcessed": result.memories_processed,
                "factsExtracted": result.facts_extracted,
                "factsInvalidated": result.facts_invalidated,
                "durationMs": result.duration_ms,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Compression failed: {}", e),
        ),
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compiles (handler signature change will cause error in handlers.rs registration — fixed in Task 5)

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/memory.rs
git commit -m "memory: replace handle_compress stub with real implementation"
```

---

### Task 4: Lift embedding provider init and add `init_compression_service`

**Files:**
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs`

**Step 1: Add `init_embedding_provider` function**

Add a standalone async function (after existing `register_*` functions):

```rust
// --- init_embedding_provider ------------------------------------------------

pub(in crate::commands::start) async fn init_embedding_provider(
    app_config: &alephcore::Config,
    daemon: bool,
) -> Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>> {
    let embedding_settings = &app_config.memory.embedding;
    let manager = alephcore::memory::EmbeddingManager::new(embedding_settings.clone());
    match manager.init().await {
        Ok(()) => {
            let provider = manager.get_active_provider().await;
            if provider.is_some() && !daemon {
                println!("Embedding provider initialized");
            }
            provider
        }
        Err(e) => {
            if !daemon {
                eprintln!("Warning: Failed to initialize embedding provider: {}", e);
            }
            None
        }
    }
}
```

**Step 2: Add `init_compression_service` function**

```rust
// --- init_compression_service -----------------------------------------------

pub(in crate::commands::start) fn init_compression_service(
    memory_db: &alephcore::memory::store::MemoryBackend,
    provider: std::sync::Arc<dyn alephcore::providers::AiProvider>,
    embedder: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
    policy: &alephcore::config::CompressionPolicy,
    daemon: bool,
) -> std::sync::Arc<alephcore::memory::compression::CompressionService> {
    use alephcore::memory::compression::{CompressionConfig, CompressionService};

    let config = CompressionConfig::from_policy(policy);
    let service = std::sync::Arc::new(CompressionService::new(
        memory_db.clone(),
        provider,
        embedder,
        config,
    ));

    // Start background compression loop (hourly check)
    let _handle = service.clone().start_background_task();

    if !daemon {
        println!("Compression service started (background interval: {}s, turn threshold: {})",
            policy.background_interval_seconds, policy.turn_threshold);
    }

    service
}
```

**Step 3: Update `register_memory_handlers` signature**

Change the existing function to accept an optional compression service:

```rust
pub(in crate::commands::start) fn register_memory_handlers(
    server: &mut GatewayServer,
    memory_db: &MemoryBackend,
    compression_service: &Option<std::sync::Arc<alephcore::memory::compression::CompressionService>>,
    daemon: bool,
) {
```

Update the `memory.compress` registration inside this function. Replace:

```rust
    register_handler!(server, "memory.compress", memory_handlers::handle_compress);
```

With:

```rust
    if let Some(cs) = compression_service {
        register_handler!(server, "memory.compress", memory_handlers::handle_compress, cs);
    } else {
        server.handlers_mut().register("memory.compress", |req| async move {
            alephcore::gateway::protocol::JsonRpcResponse::error(
                req.id,
                alephcore::gateway::protocol::INTERNAL_ERROR,
                "Compression not available: missing AI or embedding provider".to_string(),
            )
        });
    }
```

**Step 4: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: may have errors in `start/mod.rs` due to changed signatures — fixed in Task 5

**Step 5: Commit**

```bash
git add core/src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "startup: add init_embedding_provider and init_compression_service"
```

---

### Task 5: Wire everything in `run_server()`

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Step 1: Add embedder to `AgentHandlersResult`**

In `AgentHandlersResult` struct (~line 282), add:

```rust
    embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
```

**Step 2: Lift embedder init in `register_agent_handlers`**

Replace the embedder initialization block (lines 322-340) with a call to the new function, and pass `embedder` as a parameter to `register_agent_handlers`. Two approaches — simplest is to accept it as a parameter:

Add `embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>` as a parameter to `register_agent_handlers`.

Remove the internal embedder init block (lines 322-340).

In `AgentHandlersResult` return value (line 685-691), add `embedder: embedder_param.clone()` (where `embedder_param` is the passed-in clone).

Actually — simpler approach: keep embedder init inside `register_agent_handlers` but also return it in `AgentHandlersResult`. This minimizes diff:

In the `AgentHandlersResult` return (line 685), add the embedder field. In the else branch (no provider_registry), return `embedder: None`.

**Step 3: Create compression service after `register_agent_handlers`**

After the `register_agent_handlers` call (~line 1347), and after accessing `agent_result.default_provider`, add:

```rust
    // Initialize compression service (Layer 1 -> Layer 2 fact extraction)
    let compression_service: Option<Arc<alephcore::memory::compression::CompressionService>> = {
        match (&agent_result.embedder, &agent_result.default_provider) {
            (Some(emb), Some(prov)) => {
                let policy = &app_config.read().await.policies.memory.compression;
                Some(builder::handlers::init_compression_service(
                    &memory_db,
                    prov.clone(),
                    emb.clone(),
                    policy,
                    args.daemon,
                ))
            }
            _ => {
                if !args.daemon {
                    println!("Compression service disabled: missing AI or embedding provider");
                }
                None
            }
        }
    };
```

**Step 4: Update `register_memory_handlers` call**

Change (line 1381):

```rust
    register_memory_handlers(&mut server, &memory_db, args.daemon);
```

To:

```rust
    register_memory_handlers(&mut server, &memory_db, &compression_service, args.daemon);
```

**Step 5: Inject into ExecutionEngine**

The engine is created inside `register_agent_handlers` (line 504-518). Since `compression_service` is created after this call, we need to wire it differently.

Option: Add a method to the execution adapter or engine to set compression service after creation. But `ExecutionEngine` is wrapped in `Arc` by line 518.

Better approach: return the `Arc<ExecutionEngine>` from `register_agent_handlers` as part of `AgentHandlersResult`, then use `Arc::get_mut` before any clones, or use interior mutability.

Simplest: Add compression_service as `Option<Arc<CompressionService>>` parameter to `register_agent_handlers`, so it's set during engine creation. This means we need to init compression service BEFORE `register_agent_handlers`.

Revised order in `run_server()`:

```
1. init memory_db                    (existing, ~line 1277)
2. init embedding provider           (NEW - call init_embedding_provider)
3. create provider_registry preview  (NEW - lightweight check for default_provider)
4. init compression_service          (NEW - needs memory_db + provider + embedder)
5. register_agent_handlers(... compression_service)  (existing, modified)
6. register_memory_handlers(... compression_service) (existing, modified)
```

But this has a problem: the full `provider_registry` is created inside `register_agent_handlers`. We need the default AI provider before that.

Revised simplest approach: create the compression service inside `register_agent_handlers` (where provider_registry is available), return it in `AgentHandlersResult`, and pass it to `register_memory_handlers`.

**Final approach for Step 5:**

Inside `register_agent_handlers`, after engine creation (line 518), create compression service:

```rust
        // Create compression service for Layer 1 -> Layer 2 fact extraction
        let compression_svc: Option<Arc<alephcore::memory::compression::CompressionService>> =
            embedder.as_ref().map(|emb| {
                let policy = &app_config.policies.memory.compression;
                builder::handlers::init_compression_service(
                    memory_db, default_prov.as_ref().unwrap().clone(), emb.clone(), policy, daemon,
                )
            });

        // Inject into engine (before Arc wrapping)
        if let Some(ref cs) = compression_svc {
            engine = engine.with_compression_service(cs.clone());
        }
        let engine = Arc::new(engine);
```

Move the `let engine = Arc::new(engine);` line (518) to after this block.

Add `compression_service: compression_svc` to `AgentHandlersResult`.

**Step 6: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: clean compile

**Step 7: Commit**

```bash
git add core/src/bin/aleph/commands/start/mod.rs
git commit -m "startup: wire CompressionService into server initialization"
```

---

### Task 6: Integration test — manual RPC

**Step 1: Start the server**

Run: `cargo run --bin aleph 2>&1 | head -50`

Verify output includes:
- `Embedding provider initialized`
- `Compression service started (background interval: 3600s, turn threshold: 20)`

**Step 2: Test memory.compress RPC**

Use wscat or a test client to send:
```json
{"jsonrpc":"2.0","method":"memory.compress","params":null,"id":1}
```

Expected response (empty database):
```json
{"jsonrpc":"2.0","result":{"memoriesProcessed":0,"factsExtracted":0,"factsInvalidated":0,"durationMs":0},"id":1}
```

If no embedding provider configured, expected:
```json
{"jsonrpc":"2.0","error":{"code":-32603,"message":"Compression not available: missing AI or embedding provider"},"id":1}
```

**Step 3: Test memory.stats RPC**

```json
{"jsonrpc":"2.0","method":"memory.stats","params":null,"id":2}
```

Verify `totalFacts` field is present (may be 0 if no conversations yet).

**Step 4: Commit any fixes**

---

### Task 7: End-to-end test — conversation triggers compression

**Step 1: Send a chat message via RPC**

```json
{"jsonrpc":"2.0","method":"chat.send","params":{"message":"What is 2+2?"},"id":1}
```

**Step 2: Verify memory was written**

```json
{"jsonrpc":"2.0","method":"memory.stats","params":null,"id":2}
```

Expect `totalMemories` >= 1.

**Step 3: Send 20+ messages to trigger turn threshold**

After 20 conversations, check logs for:
```
Turn threshold reached, triggering immediate compression
```

**Step 4: Verify facts were created**

```json
{"jsonrpc":"2.0","method":"memory.stats","params":null,"id":3}
```

Expect `totalFacts` > 0.

**Step 5: Check dashboard**

Open panel, verify Knowledge Base card shows fact count > 0.
