# Memory Event Sourcing Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect the fully-implemented event sourcing subsystem to production by adding dual-write to the handler, wiring it into server init, running migration at startup, and exposing MemoryTimeTraveler as a builtin tool.

**Architecture:** MemoryCommandHandler gains an optional MemoryBackend field. Each write method appends to the event log then projects to the facts table. Server init creates the handler, runs migration, and injects it into CompressionService and DreamDaemon. A new `memory_timeline` builtin tool wraps MemoryTimeTraveler::explain_fact().

**Tech Stack:** Rust, SQLite (rusqlite), serde, schemars, tokio

**Spec:** `docs/superpowers/specs/2026-04-10-event-sourcing-wiring-design.md`

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/memory/events/handler.rs` | Add `memory_store: Option<MemoryBackend>`, dual-write in all 8 methods |
| Create | `src/builtin_tools/memory_timeline.rs` | MemoryTimelineTool struct + call_json |
| Modify | `src/builtin_tools/mod.rs` | Export MemoryTimelineTool |
| Modify | `src/executor/builtin_registry/definitions.rs` | Add memory_timeline definition |
| Modify | `src/executor/builtin_registry/registry.rs` | Add field + execute_tool match arm |
| Modify | `src/executor/builtin_registry/builder.rs` | Create and register memory_timeline tool |
| Modify | `src/executor/builtin_registry/groups.rs` | Add to memory_knowledge group |
| Modify | `src/bin/aleph-server/commands/start/builder/handlers.rs` | Wire handler into init_compression_service |
| Modify | `src/bin/aleph-server/commands/start/builder/agent_init.rs` | Create handler, run migration, store in AgentInitResult |

---

### Task 1: Handler Dual-Write — Struct + Constructor

**Files:**
- Modify: `src/memory/events/handler.rs:1-25`

- [ ] **Step 1: Update MemoryCommandHandler struct to add memory_store field**

In `src/memory/events/handler.rs`, replace the struct and constructor:

```rust
use crate::memory::store::MemoryBackend;

pub struct MemoryCommandHandler {
    db: Arc<StateDatabase>,
    memory_store: Option<MemoryBackend>,
}

impl MemoryCommandHandler {
    pub fn new(db: Arc<StateDatabase>, memory_store: Option<MemoryBackend>) -> Self {
        Self { db, memory_store }
    }
```

- [ ] **Step 2: Fix existing test helper to pass None**

In the same file, update `make_handler()` in the `#[cfg(test)]` module:

```rust
fn make_handler() -> MemoryCommandHandler {
    let db = Arc::new(crate::resilience::database::StateDatabase::in_memory().unwrap());
    MemoryCommandHandler::new(db, None)
}
```

- [ ] **Step 3: Run existing tests to verify no breakage**

Run: `cargo test -p alephcore --lib memory::events::handler`
Expected: All 7 existing tests PASS (they use `memory_store: None`)

- [ ] **Step 4: Compile check for downstream callers**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: Compile errors in `memory/integration_tests/mod.rs` where `MemoryCommandHandler::new(db)` is called with 1 arg. Fix by adding `None` as second argument.

- [ ] **Step 5: Fix integration test constructor call**

In `src/memory/integration_tests/mod.rs`, find `MemoryCommandHandler::new(db.clone())` and change to `MemoryCommandHandler::new(db.clone(), None)`.

- [ ] **Step 6: Verify full compile**

Run: `cargo check -p alephcore`
Expected: Clean compile

- [ ] **Step 7: Commit**

```bash
git add src/memory/events/handler.rs src/memory/integration_tests/mod.rs
git commit -m "memory: add MemoryBackend field to MemoryCommandHandler"
```

---

### Task 2: Handler Dual-Write — project_and_store helper

**Files:**
- Modify: `src/memory/events/handler.rs`

- [ ] **Step 1: Add the projection helper method**

Add a private method to `MemoryCommandHandler` that projects events to facts table:

```rust
impl MemoryCommandHandler {
    /// Project the current event stream for a fact into the facts table.
    ///
    /// Called after appending an event to the event log. If `memory_store`
    /// is None, this is a no-op (test mode).
    async fn project_to_store(&self, fact_id: &str) -> Result<(), AlephError> {
        let Some(ref store) = self.memory_store else {
            return Ok(());
        };

        let events = self.db.get_memory_events_for_fact(fact_id).await?;
        let projected = super::projector::EventProjector::fold_events_to_fact(&events)?;

        match projected {
            Some(fact) => {
                // Try update first; if the fact doesn't exist yet, insert.
                if store.get_fact(fact_id).await?.is_some() {
                    if let Err(e) = store.update_fact(&fact).await {
                        tracing::error!(fact_id, error = %e, "Dual-write: failed to update fact in store");
                    }
                } else {
                    if let Err(e) = store.insert_fact(&fact).await {
                        tracing::error!(fact_id, error = %e, "Dual-write: failed to insert fact into store");
                    }
                }
            }
            None => {
                // Fact was deleted — remove from store
                if let Err(e) = store.delete_fact(fact_id).await {
                    tracing::error!(fact_id, error = %e, "Dual-write: failed to delete fact from store");
                }
            }
        }

        Ok(())
    }
}
```

- [ ] **Step 2: Verify compile**

Run: `cargo check -p alephcore`
Expected: Clean compile (method is unused but will be called in Task 3)

- [ ] **Step 3: Commit**

```bash
git add src/memory/events/handler.rs
git commit -m "memory: add project_to_store dual-write helper to handler"
```

---

### Task 3: Handler Dual-Write — Wire into all 8 methods

**Files:**
- Modify: `src/memory/events/handler.rs`

- [ ] **Step 1: Add project_to_store call to create_fact**

After `self.db.append_memory_event(&envelope).await?;`, add:

```rust
self.project_to_store(&fact_id).await?;
```

- [ ] **Step 2: Add project_to_store call to update_content**

After `self.db.append_memory_event(&envelope).await?;`, add:

```rust
self.project_to_store(&cmd.fact_id).await?;
```

Note: `cmd.fact_id` has been moved into the envelope by this point. Capture it before the move:

```rust
let fact_id = cmd.fact_id.clone();
// ... existing envelope creation using cmd.fact_id ...
self.db.append_memory_event(&envelope).await?;
self.project_to_store(&fact_id).await?;
```

Wait — check the existing code: `cmd.fact_id` is used in the `MemoryEventEnvelope::new(cmd.fact_id, ...)` call which moves it. So we need to clone before the move. Review each method and apply the same pattern where `cmd.fact_id` is moved.

Apply to all methods: `update_content`, `invalidate_fact`, `restore_fact`, `record_access`, `delete_fact`. Each needs a `let fact_id = cmd.fact_id.clone();` before the envelope construction, then `self.project_to_store(&fact_id).await?;` after append.

- [ ] **Step 3: Add project_to_store to apply_decay (bulk)**

After `self.db.append_memory_events(&envelopes).await?;`, project each affected fact:

```rust
for (fact_id, _, _) in &cmd.fact_ids_with_strength {
    self.project_to_store(fact_id).await?;
}
```

- [ ] **Step 4: Add project_to_store to consolidate_facts**

After `self.db.append_memory_event(&envelope).await?;`, add:

```rust
self.project_to_store(&fact_id).await?;
```

(`fact_id` is already a local variable in this method)

- [ ] **Step 5: Run existing tests**

Run: `cargo test -p alephcore --lib memory::events::handler`
Expected: All 7 tests PASS (memory_store is None, project_to_store is a no-op)

- [ ] **Step 6: Commit**

```bash
git add src/memory/events/handler.rs
git commit -m "memory: wire project_to_store into all 8 handler methods"
```

---

### Task 4: Dual-Write Integration Test

**Files:**
- Modify: `src/memory/events/handler.rs` (test module)

- [ ] **Step 1: Write dual-write test for create_fact**

Add to the `#[cfg(test)] mod tests` in handler.rs:

```rust
#[tokio::test]
async fn test_create_fact_dual_write() {
    use crate::memory::store::MemoryStore;

    let db = Arc::new(crate::resilience::database::StateDatabase::in_memory().unwrap());
    let store = crate::memory::store::sqlite::SqliteMemoryBackend::in_memory().unwrap();
    let store = Arc::new(store);
    let handler = MemoryCommandHandler::new(db.clone(), Some(store.clone()));

    let fact_id = handler
        .create_fact(CreateFactCommand {
            content: "Dual-write test".into(),
            fact_type: FactType::Learning,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            path: "/test/dual".into(),
            namespace: "owner".into(),
            agent: "default".into(),
            confidence: 0.9,
            source: FactSource::Extracted,
            source_memory_ids: vec![],
            actor: EventActor::Agent,
            correlation_id: None,
        })
        .await
        .unwrap();

    // Verify event log
    let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.event_type_tag(), "FactCreated");

    // Verify facts table
    let fact = store.get_fact(&fact_id).await.unwrap().expect("fact should exist in store");
    assert_eq!(fact.content, "Dual-write test");
    assert_eq!(fact.fact_type, FactType::Learning);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p alephcore --lib memory::events::handler::tests::test_create_fact_dual_write`
Expected: PASS

- [ ] **Step 3: Write dual-write test for update + invalidate + delete**

```rust
#[tokio::test]
async fn test_dual_write_lifecycle() {
    use crate::memory::store::MemoryStore;

    let db = Arc::new(crate::resilience::database::StateDatabase::in_memory().unwrap());
    let store = crate::memory::store::sqlite::SqliteMemoryBackend::in_memory().unwrap();
    let store = Arc::new(store);
    let handler = MemoryCommandHandler::new(db.clone(), Some(store.clone()));

    // Create
    let fact_id = handler
        .create_fact(CreateFactCommand {
            content: "Original".into(),
            fact_type: FactType::Learning,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            path: "/test/lifecycle".into(),
            namespace: "owner".into(),
            agent: "default".into(),
            confidence: 0.9,
            source: FactSource::Extracted,
            source_memory_ids: vec![],
            actor: EventActor::Agent,
            correlation_id: None,
        })
        .await
        .unwrap();

    // Update content
    handler
        .update_content(UpdateContentCommand {
            fact_id: fact_id.clone(),
            new_content: "Updated".into(),
            reason: "test".into(),
            actor: EventActor::User,
            correlation_id: None,
        })
        .await
        .unwrap();

    let fact = store.get_fact(&fact_id).await.unwrap().unwrap();
    assert_eq!(fact.content, "Updated");

    // Invalidate
    handler
        .invalidate_fact(InvalidateFactCommand {
            fact_id: fact_id.clone(),
            reason: "outdated".into(),
            actor: EventActor::System,
            strength_at_invalidation: Some(0.5),
            correlation_id: None,
        })
        .await
        .unwrap();

    let fact = store.get_fact(&fact_id).await.unwrap().unwrap();
    assert!(!fact.is_valid);

    // Delete
    handler
        .delete_fact(DeleteFactCommand {
            fact_id: fact_id.clone(),
            reason: "cleanup".into(),
            actor: EventActor::User,
            correlation_id: None,
        })
        .await
        .unwrap();

    let fact = store.get_fact(&fact_id).await.unwrap();
    assert!(fact.is_none(), "Deleted fact should not exist in store");
}
```

- [ ] **Step 4: Run all handler tests**

Run: `cargo test -p alephcore --lib memory::events::handler`
Expected: All tests PASS (9 existing + 2 new)

- [ ] **Step 5: Commit**

```bash
git add src/memory/events/handler.rs
git commit -m "memory: add dual-write integration tests for handler"
```

---

### Task 5: MemoryTimelineTool

**Files:**
- Create: `src/builtin_tools/memory_timeline.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Create the tool file**

Create `src/builtin_tools/memory_timeline.rs`:

```rust
//! memory_timeline tool — view the complete lifecycle of a memory fact.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;
use crate::memory::events::traveler::MemoryTimeTraveler;
use crate::sync_primitives::Arc;

/// Arguments for the memory_timeline tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryTimelineArgs {
    /// The fact ID to inspect
    pub fact_id: String,
}

/// Tool to view the complete lifecycle of a memory fact
pub struct MemoryTimelineTool {
    traveler: Arc<MemoryTimeTraveler>,
}

impl MemoryTimelineTool {
    pub const DESCRIPTION: &'static str =
        "View the complete lifecycle of a memory fact — creation, modification, decay, invalidation timeline";

    pub fn new(traveler: Arc<MemoryTimeTraveler>) -> Self {
        Self { traveler }
    }

    pub async fn call_json(&self, arguments: &serde_json::Value) -> Result<String, AlephError> {
        let args: MemoryTimelineArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AlephError::tool(format!("Invalid arguments: {e}")))?;

        let explanation = self.traveler.explain_fact(&args.fact_id).await?;

        serde_json::to_string_pretty(&explanation)
            .map_err(|e| AlephError::other(format!("Failed to serialize explanation: {e}")))
    }
}
```

- [ ] **Step 2: Export from builtin_tools/mod.rs**

Add to `src/builtin_tools/mod.rs`:

```rust
pub mod memory_timeline;
pub use memory_timeline::MemoryTimelineTool;
```

- [ ] **Step 3: Verify compile**

Run: `cargo check -p alephcore`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/memory_timeline.rs src/builtin_tools/mod.rs
git commit -m "memory: add memory_timeline builtin tool"
```

---

### Task 6: Register memory_timeline in Builtin Registry

**Files:**
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/groups.rs`
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/builder.rs`

- [ ] **Step 1: Add tool definition**

In `src/executor/builtin_registry/definitions.rs`, add after the `memory_explore` entry:

```rust
BuiltinToolDefinition {
    name: "memory_timeline",
    description: "View the complete lifecycle of a memory fact — creation, modification, decay, invalidation timeline",
    requires_config: true, // Requires StateDatabase
},
```

- [ ] **Step 2: Add to memory_knowledge group**

In `src/executor/builtin_registry/groups.rs`, add `"memory_timeline"` to the `memory_knowledge` tools array, after `"memory_explore"`:

```rust
ToolCategory {
    id: "memory_knowledge",
    name: "记忆与知识",
    tools: &[
        "memory_search",
        "memory_browse",
        "memory_explore",
        "memory_timeline",
        "wiki_manage",
        "session_search",
        "skill_list",
        "skill_read",
    ],
},
```

- [ ] **Step 3: Add field to BuiltinToolRegistry**

In `src/executor/builtin_registry/registry.rs`, add a field after `memory_explore_tool`:

```rust
/// Memory timeline tool instance (optional - requires StateDatabase)
pub(crate) memory_timeline_tool: Option<crate::builtin_tools::MemoryTimelineTool>,
```

- [ ] **Step 4: Add execute_tool match arm**

In the `execute_tool` method of `BuiltinToolRegistry` in `registry.rs`, add after the `"memory_explore"` arm:

```rust
"memory_timeline" => Box::pin(async move {
    let tool = self.memory_timeline_tool.as_ref().ok_or_else(|| {
        AlephError::tool("memory_timeline not available: no event store configured")
    })?;
    tool.call_json(arguments).await
}),
```

- [ ] **Step 5: Create and register tool in builder**

In `src/executor/builtin_registry/builder.rs`, in the `build()` method where memory tools are created (around line 172-182):

After `let explore_tool = MemoryExploreTool::new(...)`, add:

```rust
let timeline_tool = {
    let state_db = crate::resilience::database::StateDatabase::from_memory_backend(db);
    state_db.map(|sdb| {
        let traveler = Arc::new(crate::memory::events::traveler::MemoryTimeTraveler::new(
            Arc::new(sdb),
        ));
        crate::builtin_tools::MemoryTimelineTool::new(traveler)
    })
};
```

Wait — the MemoryTimeTraveler needs `Arc<StateDatabase>`. We need to check how to get StateDatabase from the builder context. The builder has `config.memory_db` (MemoryBackend) but not StateDatabase directly. We need to check if StateDatabase is available or needs to be passed through.

Let me check what's available.

- [ ] **Step 5 (revised): Determine StateDatabase availability in builder**

The builder needs `Arc<StateDatabase>` to create `MemoryTimeTraveler`. This same `StateDatabase` is needed for `MemoryCommandHandler`. Both should be passed into the builder config. Add a new optional field to `BuiltinToolConfig`:

In `src/executor/builtin_registry/builder.rs`, add to `BuiltinToolConfig`:

```rust
pub state_db: Option<Arc<crate::resilience::database::StateDatabase>>,
```

Then in the memory tool creation block:

```rust
let timeline_tool = config.state_db.as_ref().map(|sdb| {
    let traveler = Arc::new(crate::memory::events::traveler::MemoryTimeTraveler::new(
        Arc::clone(sdb),
    ));
    crate::builtin_tools::MemoryTimelineTool::new(traveler)
});
```

Add `memory_timeline_tool: timeline_tool,` to the struct initialization.

Register the tool metadata in `register_optional_tools`:

```rust
if memory_timeline_tool.is_some() {
    reg(
        tools,
        "memory_timeline",
        crate::builtin_tools::MemoryTimelineTool::DESCRIPTION,
        serde_json::to_value(schemars::schema_for!(
            crate::builtin_tools::memory_timeline::MemoryTimelineArgs
        ))
        .unwrap_or_default(),
    );
    info!("Registered memory_timeline tool in BuiltinToolRegistry");
}
```

- [ ] **Step 6: Verify compile**

Run: `cargo check -p alephcore`
Expected: Clean compile (state_db defaults to None in existing callers)

- [ ] **Step 7: Commit**

```bash
git add src/executor/builtin_registry/definitions.rs \
        src/executor/builtin_registry/groups.rs \
        src/executor/builtin_registry/registry.rs \
        src/executor/builtin_registry/builder.rs
git commit -m "memory: register memory_timeline in builtin tool registry"
```

---

### Task 7: Server Init — Create Handler + Run Migration

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs`

- [ ] **Step 1: Add init_command_handler function in handlers.rs**

Add a new init function after `init_compression_service`:

```rust
// ─── init_command_handler ───────────────────────────────────────────────────

pub(in crate::commands::start) async fn init_command_handler(
    state_db: &std::sync::Arc<alephcore::resilience::database::StateDatabase>,
    memory_db: &alephcore::memory::store::MemoryBackend,
    daemon: bool,
) -> std::sync::Arc<alephcore::memory::events::handler::MemoryCommandHandler> {
    use alephcore::memory::events::handler::MemoryCommandHandler;
    use alephcore::memory::events::migration::EventSourcingMigration;
    use alephcore::memory::store::MemoryStore;

    let handler = std::sync::Arc::new(MemoryCommandHandler::new(
        std::sync::Arc::clone(state_db),
        Some(memory_db.clone()),
    ));

    // Run one-shot migration (idempotent)
    let migration = EventSourcingMigration::new(std::sync::Arc::clone(state_db));
    match memory_db.get_all_facts(true, None).await {
        Ok(facts) => {
            match migration.run_if_needed(&facts).await {
                Ok(report) => {
                    if report.migrated > 0 && !daemon {
                        println!(
                            "Event sourcing migration: {} facts migrated, {} skipped",
                            report.migrated, report.skipped
                        );
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Event sourcing migration failed (non-fatal)");
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to load facts for event sourcing migration (non-fatal)");
        }
    }

    handler
}
```

- [ ] **Step 2: Modify init_compression_service to accept handler**

Change the function signature to accept an optional handler:

```rust
pub(in crate::commands::start) fn init_compression_service(
    memory_db: &alephcore::memory::store::MemoryBackend,
    provider: std::sync::Arc<dyn alephcore::providers::AiProvider>,
    embedder: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
    policy: &alephcore::CompressionPolicy,
    daemon: bool,
    command_handler: Option<std::sync::Arc<alephcore::memory::events::handler::MemoryCommandHandler>>,
) -> std::sync::Arc<alephcore::memory::compression::CompressionService> {
```

And after `CompressionService::new(...)`, chain `.with_command_handler()`:

```rust
let mut service = CompressionService::new(memory_db.clone(), provider, embedder, config);
if let Some(handler) = command_handler {
    service = service.with_command_handler(handler);
}
let service = std::sync::Arc::new(service);
```

- [ ] **Step 3: Update agent_init.rs call site**

In `agent_init.rs`, where `init_compression_service` is called, pass the handler. This requires:

1. Add `command_handler: Option<Arc<alephcore::memory::events::handler::MemoryCommandHandler>>` field to `AgentInitResult`
2. Call `init_command_handler` before `init_compression_service`
3. Pass `command_handler.clone()` to `init_compression_service`
4. Store `command_handler` in `AgentInitResult`

Also pass `state_db` into the `BuiltinToolConfig` for the `memory_timeline` tool.

- [ ] **Step 4: Verify compile**

Run: `cargo check`
Expected: Clean compile

- [ ] **Step 5: Start server and verify migration runs**

Run: `cargo run --bin aleph-server -- start 2>&1 | head -20`
Expected: Migration message in output (or "already completed" log on subsequent runs)

- [ ] **Step 6: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/handlers.rs \
        src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "memory: wire event sourcing handler into server init with migration"
```

---

### Task 8: Wire DreamDaemon (if applicable)

**Files:**
- Modify: `src/memory/dreaming/mod.rs` (caller site where DreamDaemon is constructed)

- [ ] **Step 1: Find DreamDaemon construction site**

Search for where `DreamDaemon::from_config` is called in production code and chain `.with_command_handler(handler.clone())`.

The `ensure_dream_daemon` function in `src/memory/dreaming/mod.rs` is the likely entry point. It constructs `DreamDaemon` and may need access to the handler.

- [ ] **Step 2: Add handler parameter to ensure_dream_daemon**

If `ensure_dream_daemon` takes a config, add an optional handler parameter:

```rust
pub fn ensure_dream_daemon(
    database: MemoryBackend,
    config: &MemoryConfig,
    command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
) -> Result<...> {
    let mut daemon = DreamDaemon::from_config(database, config)?;
    if let Some(handler) = command_handler {
        daemon = daemon.with_command_handler(handler);
    }
    // ...
}
```

- [ ] **Step 3: Update callers of ensure_dream_daemon**

Pass `command_handler` from wherever `ensure_dream_daemon` is called (likely in server init or ingestion pipeline).

- [ ] **Step 4: Verify compile and test**

Run: `cargo check && cargo test -p alephcore --lib memory::dreaming`
Expected: Clean compile, tests pass

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/mod.rs
git commit -m "memory: wire command handler into DreamDaemon"
```

---

### Task 9: Final Verification

**Files:** None (verification only)

- [ ] **Step 1: Run full core test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p alephcore --test '*'`
Expected: All tests pass (or no integration test binaries, which is fine)

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -5`
Expected: No warnings

- [ ] **Step 4: Verify server starts cleanly**

Run: Start and immediately stop the server to verify no panics.

- [ ] **Step 5: Commit plan completion marker**

No code changes — this is a verification-only task.
