# Memory Event Sourcing Wiring Design

## Status

Approved (2026-04-10)

## Context

The memory module completed a comprehensive logic chain fix (L1-L4 + P1-P3) on 2026-04-10. Event sourcing is the last disconnected subsystem. All components are fully implemented and tested but only used in test code:

- `MemoryCommandHandler` — write-side facade, 8 commands
- `EventProjector` — pure fold from events to MemoryFact
- `MemoryTimeTraveler` — time travel queries + explain_fact
- `StateDatabase` — memory_events table CRUD
- `EventSourcingMigration` — idempotent legacy-to-ES migration

Two subsystems have `with_command_handler()` slots that are never injected in production:
- `CompressionService` (has fallback pattern)
- `DreamDaemon` (decay stage uses handler for StrengthDecayed events)

## Problem

The handler only writes to the event log (`memory_events` table). All read paths query the facts table (`memory_facts`). If the handler is activated as-is, facts created through it would "disappear" — written to event log but invisible to retrieval, VFS, browse, and all other read paths.

## Design

### 1. Handler Dual-Write

Augment `MemoryCommandHandler` to write both event log AND facts table:

```rust
pub struct MemoryCommandHandler {
    db: Arc<StateDatabase>,              // event log (existing)
    memory_store: Option<MemoryBackend>, // facts table projection (new)
}
```

`Option` because existing tests use `StateDatabase::in_memory()` without a facts table. Production injects `Some(memory_backend)`.

Constructor signature changes from `new(db: Arc<StateDatabase>)` to `new(db: Arc<StateDatabase>, memory_store: Option<MemoryBackend>)`. Existing test helper `make_handler()` passes `None` — zero test breakage.

#### Dual-write flow (create_fact example)

1. Build `MemoryEvent::FactCreated`
2. `append_memory_event()` → write `memory_events` table
3. `EventProjector::fold_events_to_fact()` → rebuild MemoryFact from events
4. `memory_store.insert_fact()` → write `memory_facts` table

Step 3 uses the projector to reconstruct the fact rather than manual construction, ensuring the facts table is always consistent with the event log. If step 4 fails, the event log already has the record — event replay can recover later.

#### Command-to-projection mapping

| Handler Method | Event | Facts Table Op |
|---|---|---|
| `create_fact` | FactCreated | `insert_fact()` |
| `update_content` | FactContentUpdated | `update_fact()` |
| `invalidate_fact` | FactInvalidated | `update_fact()` |
| `restore_fact` | FactRestored | `update_fact()` |
| `record_access` | FactAccessed | `update_fact()` |
| `apply_decay` | StrengthDecayed (bulk) | `update_fact()` per fact |
| `consolidate_facts` | FactConsolidated | `insert_fact()` |
| `delete_fact` | FactDeleted | `delete_fact()` |

### 2. Server Init Wiring + Migration

Initialization order in `agent_init.rs` / `handlers.rs`:

```
1. (existing) StateDatabase created
2. (existing) MemoryBackend (SqliteMemoryBackend) created
3. (new)      MemoryCommandHandler::new(state_db, Some(memory_backend))
4. (new)      EventSourcingMigration::new(state_db).run_if_needed(existing_facts)
5. (modified) CompressionService::new(...).with_command_handler(handler.clone())
6. (new)      DreamDaemon::from_config(...).with_command_handler(handler.clone())
```

#### Migration details

- `run_if_needed()` loads all facts via `memory_backend.get_all_facts(true, None)` (including invalid)
- First run generates `FactMigrated` events; subsequent runs are instant (idempotent via `has_migration_events()`)
- Migration only writes event log — facts table already has the data

#### Handler sharing

`Arc<MemoryCommandHandler>` stored in `AgentInitResult` for downstream consumers.

### 3. Phased Write-Point Migration

#### Phase 1 (this implementation): Activate existing slots

Only 2 consumers — they already have `with_command_handler()` and fallback patterns:

- **CompressionService** — `create_fact` handler path (service.rs:290), activate by injection
- **DreamDaemon decay stage** — `apply_decay` handler path (decay.rs:42), activate by injection

#### Phase 2 (future): High-volume subsystems

By write frequency:

1. **SessionCompactor** (3x insert + 1x invalidate) — highest write volume
2. **DreamDaemon remaining stages** — synthesis(insert), consolidate(invalidate + update), drift(update_content)
3. **ConflictDetector** (1x invalidate) — follows CompressionService

#### Phase 3 (future): Tools & CLI

4. wiki_manage (3 points)
5. tool_index coordinator (3 points)
6. CLI commands (3 points)
7. TranscriptIndexer (1x insert)
8. L1Generator (2 points)
9. reembed (1x update)

Each migration follows the same pattern: obtain `Arc<MemoryCommandHandler>`, construct the corresponding Command struct, call handler method instead of direct `database.xxx_fact()`.

#### Degradation strategy

Phase 2/3 write points continue writing directly to facts table until migrated. These writes won't appear in the event log — acceptable as the log becomes complete progressively.

### 4. MemoryTimeTraveler Tool

#### Tool definition

Registered as builtin tool `memory_timeline`:

- **Name:** `memory_timeline`
- **Description:** View the complete lifecycle of a memory fact — creation, modification, decay, invalidation timeline
- **Parameters:** `fact_id` (string, required)
- **Returns:** `FactExplanation` JSON

#### Implementation

Reuses `MemoryTimeTraveler::explain_fact()`. Returns `FactExplanation` containing:
- `fact_id`, `content`, `is_valid`
- `creation_source` — origin (extracted / manual / migration)
- `access_count` — retrieval count
- `invalidation_reason` — if invalidated, the reason
- `events[]` — full event list, each with timestamp + action + description + actor

#### Registration

Same pattern as `memory_explore` (RippleTask):
- `builtin_registry/definitions.rs` — tool definition + JSON Schema
- `builtin_registry/registry.rs` — executor registration
- `builtin_registry/groups.rs` — add to memory tool group

#### Dependency

Tool executor needs `Arc<MemoryTimeTraveler>`, created in server init alongside the handler (both share `Arc<StateDatabase>`).

### 5. Error Handling

#### Dual-write failure

- **Event log write fails** → return `Err`. Facts table not written. Event is the source of truth; if it fails, the operation did not happen.
- **Event log succeeds, facts table write fails** → `tracing::error!`, return `Ok`. Event log has the record; facts table recoverable via replay. No panic, no event loss.

#### Migration failure

- `run_if_needed()` returns Err → log but do not block server startup. Empty event log means `memory_timeline` has no history for old facts, but doesn't affect other functionality.

### 6. Testing

#### Unit tests (handler.rs)

- Existing 7 tests preserved (use `memory_store: None`, behavior unchanged)
- New dual-write tests: construct handler with `Some(MemoryBackend)`, verify event log and facts table written simultaneously

#### Integration tests (memory/integration_tests/)

- Existing handler + projector + traveler integration tests pass
- New: handler dual-write → verify `database.get_fact()` result matches `EventProjector::fold_events_to_fact()` output

#### memory_timeline tool tests

- Same structure as `memory_explore` tests: construct test env, call tool executor, verify returned JSON structure

No E2E tests needed — internal subsystem, no external API boundary.

## Key Files

```
src/memory/events/handler.rs              — MemoryCommandHandler (modify: add dual-write)
src/memory/events/mod.rs                  — event types (no change)
src/memory/events/projector.rs            — EventProjector (no change)
src/memory/events/traveler.rs             — MemoryTimeTraveler (no change)
src/memory/events/commands.rs             — command structs (no change)
src/memory/events/migration.rs            — EventSourcingMigration (no change)
src/resilience/database/memory_events.rs  — StateDatabase event CRUD (no change)
src/memory/compression/service.rs         — CompressionService (no change, handler injected)
src/memory/dreaming/mod.rs                — DreamDaemon (no change, handler injected)
src/bin/aleph-server/.../agent_init.rs    — server init (modify: create & inject handler)
src/bin/aleph-server/.../handlers.rs      — init_compression_service (modify: wire handler)
src/executor/builtin_registry/definitions.rs — tool defs (modify: add memory_timeline)
src/executor/builtin_registry/registry.rs    — executors (modify: add memory_timeline)
src/executor/builtin_registry/groups.rs      — groups (modify: add to memory group)
```

## Scope Boundary

This design covers Phase 1 only. Phase 2/3 write-point migrations are future work with the same pattern. The event log becomes progressively complete as more write points are migrated.
