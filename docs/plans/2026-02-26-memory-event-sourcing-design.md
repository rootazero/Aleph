# Memory Event Sourcing Design

> **Date**: 2026-02-26
> **Status**: Approved
> **Approach**: CQRS Light (Event Store + Projector)

## Background

The memory system currently uses direct CRUD operations against LanceDB for fact management, with a separate AuditStore for lifecycle logging. This design introduces Event Sourcing to achieve:

1. **Time Travel Queries** — reconstruct fact state at any historical point
2. **Evolution Audit** — complete lifecycle tracking from creation to decay
3. **State Reconstruction & Reliability** — rebuild memory state from event stream
4. **Architectural Elegance** — unify scattered audit/decay/compression logic under events

### Prior Art in Codebase

| Component | Relevance |
|-----------|-----------|
| `AuditStore` + `AuditEntry` | Already tracks Created/Accessed/Updated/Invalidated/Restored/Deleted — evolves into EventStore |
| `resilience::perception` | Skeleton/Pulse event classification, EventEmitter with dual-write — reuse classification model |
| `StateDatabase` (SQLite) | Existing event storage infrastructure — host for memory events |
| LanceDB delete-then-insert | Naturally immutable — fits projection model |

## Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                    Callers (Agent Loop, Tools, etc.) │
│                                                      │
│  Read:  MemoryStore (LanceDB) ← unchanged           │
│  Write: MemoryCommandHandler → event-first           │
│  Query: MemoryTimeTraveler → time travel             │
└──────────┬──────────────┬───────────────┬────────────┘
           │              │               │
           ▼              ▼               ▼
  ┌─────────────┐  ┌────────────┐  ┌──────────────┐
  │ EventStore  │  │ Projector  │  │ TimeTraveler │
  │  (SQLite)   │──│ (sync)     │──│  (read-only) │
  │ Source of   │  │ Event →    │  │  Replay &    │
  │   Truth     │  │ LanceDB    │  │  Reconstruct │
  └─────────────┘  └────────────┘  └──────────────┘
```

### Key Principle: CQRS Light

- **Write side**: All mutations go through `MemoryCommandHandler` → events persisted to SQLite first → projected to LanceDB
- **Read side**: Existing `MemoryStore` trait (LanceDB) unchanged — all search/retrieval works as before
- **Time travel**: `MemoryTimeTraveler` replays events from SQLite to reconstruct historical state

## 1. Event Model

### MemoryEvent Enum

```rust
/// Memory domain event — atomic unit of change in the memory system.
/// Immutable, append-only, carries enough data to reconstruct state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MemoryEvent {
    // === Fact lifecycle ===
    FactCreated {
        fact_id: String,
        content: String,
        fact_type: FactType,
        tier: MemoryTier,
        scope: MemoryScope,
        path: String,
        namespace: String,
        workspace: String,
        confidence: f32,
        source: FactSource,
        source_memory_ids: Vec<String>,
    },
    FactContentUpdated {
        fact_id: String,
        old_content: String,
        new_content: String,
        reason: String,
    },
    FactMetadataUpdated {
        fact_id: String,
        field: String,          // "tier", "scope", "path", etc.
        old_value: String,
        new_value: String,
    },

    // === Tier transitions (cognitive architecture) ===
    TierTransitioned {
        fact_id: String,
        from_tier: MemoryTier,
        to_tier: MemoryTier,
        trigger: TierTransitionTrigger,  // Consolidation | Reinforcement | Decay
    },

    // === Strength & decay ===
    FactAccessed {
        fact_id: String,
        query: Option<String>,
        relevance_score: Option<f32>,
        used_in_response: bool,
        new_access_count: u32,
    },
    StrengthDecayed {
        fact_id: String,
        old_strength: f32,
        new_strength: f32,
        decay_factor: f32,
    },

    // === Soft delete / restore ===
    FactInvalidated {
        fact_id: String,
        reason: String,
        actor: EventActor,
        strength_at_invalidation: Option<f32>,
    },
    FactRestored {
        fact_id: String,
        new_strength: f32,
    },
    FactDeleted {
        fact_id: String,
        reason: String,
    },

    // === Compression pipeline ===
    FactConsolidated {
        fact_id: String,
        source_fact_ids: Vec<String>,
        consolidated_content: String,
    },

    // === Migration ===
    FactMigrated {
        fact_id: String,
        snapshot: serde_json::Value,     // Full MemoryFact as JSON
    },
}
```

### EventEnvelope

```rust
/// Wrapper with metadata for ordering and tracing.
pub struct MemoryEventEnvelope {
    pub id: i64,                        // Auto-increment (global ordering)
    pub fact_id: String,                // Aggregate ID (partition key)
    pub seq: u64,                       // Per-fact sequence number
    pub event: MemoryEvent,             // The domain event
    pub actor: EventActor,              // Who caused this event
    pub tier: EventTier,                // Skeleton | Pulse
    pub timestamp: i64,                 // Unix seconds
    pub correlation_id: Option<String>, // Link to triggering task/session
}

pub enum EventActor {
    Agent,
    User,
    System,       // Compression, Decay, DreamDaemon
    Migration,    // One-time migration
}
```

### Event Classification (Skeleton vs Pulse)

| Event | Tier | Rationale |
|-------|------|-----------|
| FactCreated, FactDeleted, FactInvalidated, FactRestored | **Skeleton** | Irreversible state changes |
| TierTransitioned, FactConsolidated | **Skeleton** | Cognitive architecture transitions |
| FactContentUpdated, FactMetadataUpdated | **Skeleton** | Content changes need audit |
| FactAccessed | **Pulse** | High frequency, batch-writable |
| StrengthDecayed | **Pulse** | Periodic bulk operation |
| FactMigrated | **Skeleton** | One-time migration |

## 2. Event Store

### MemoryEventStore Trait

```rust
/// Append-only event log — source of truth for all memory fact mutations.
/// Stored in SQLite (StateDatabase).
#[async_trait]
pub trait MemoryEventStore: Send + Sync {
    // === Write ===
    async fn append_event(&self, envelope: &MemoryEventEnvelope) -> Result<i64, AlephError>;
    async fn append_events(&self, envelopes: &[MemoryEventEnvelope]) -> Result<(), AlephError>;

    // === Read by fact ===
    async fn get_events_for_fact(&self, fact_id: &str) -> Result<Vec<MemoryEventEnvelope>, AlephError>;
    async fn get_events_since_seq(&self, fact_id: &str, since_seq: u64) -> Result<Vec<MemoryEventEnvelope>, AlephError>;

    // === Time travel ===
    async fn get_events_until(&self, fact_id: &str, until_timestamp: i64) -> Result<Vec<MemoryEventEnvelope>, AlephError>;
    async fn get_events_in_range(&self, from: i64, to: i64, limit: usize) -> Result<Vec<MemoryEventEnvelope>, AlephError>;

    // === Statistics ===
    async fn get_latest_seq(&self, fact_id: &str) -> Result<u64, AlephError>;
    async fn count_events(&self, event_type_filter: Option<&str>) -> Result<usize, AlephError>;
}
```

### SQLite Schema

```sql
CREATE TABLE IF NOT EXISTS memory_events (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id        TEXT NOT NULL,
    seq            INTEGER NOT NULL,
    event_type     TEXT NOT NULL,
    event_json     TEXT NOT NULL,
    actor          TEXT NOT NULL,
    tier           TEXT NOT NULL,
    timestamp      INTEGER NOT NULL,
    correlation_id TEXT,

    UNIQUE(fact_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_me_fact_id ON memory_events(fact_id);
CREATE INDEX IF NOT EXISTS idx_me_timestamp ON memory_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_me_event_type ON memory_events(event_type);
CREATE INDEX IF NOT EXISTS idx_me_correlation ON memory_events(correlation_id);
```

### Implementation Location

- Trait definition: `core/src/memory/store/mod.rs` (alongside other Store traits)
- SQLite implementation: `core/src/resilience/database/memory_events.rs` (new file)
- Reuses `StateDatabase.conn: Arc<Mutex<Connection>>`

## 3. Projector & Write Path

### EventProjector

```rust
/// Projects memory events into LanceDB snapshots (read model).
pub struct EventProjector {
    event_store: Arc<dyn MemoryEventStore>,
    memory_store: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
}

impl EventProjector {
    /// Process a single event and update the read model.
    pub async fn project(&self, envelope: &MemoryEventEnvelope) -> Result<(), AlephError>;

    /// Rebuild a fact's snapshot from its entire event stream.
    pub async fn rebuild_fact(&self, fact_id: &str) -> Result<Option<MemoryFact>, AlephError>;

    /// Rebuild a fact's state at a specific point in time.
    pub async fn rebuild_fact_at(&self, fact_id: &str, at: i64) -> Result<Option<MemoryFact>, AlephError>;

    /// Pure function: fold events into a MemoryFact.
    fn fold_events_to_fact(events: &[MemoryEventEnvelope]) -> Result<Option<MemoryFact>, AlephError>;
}
```

### Write Path Transformation

**Before** (direct CRUD):
```
Caller → MemoryStore.insert_fact(fact) → LanceDB
```

**After** (event-first):
```
Caller → MemoryCommandHandler.create_fact(cmd)
    → 1. Build MemoryEvent::FactCreated
    → 2. MemoryEventStore.append_event()   [SQLite — source of truth]
    → 3. EventProjector.project()           [LanceDB — read model]
    → 4. Return fact_id
```

### MemoryCommandHandler

```rust
/// Write-side facade. All fact mutations go through this handler.
pub struct MemoryCommandHandler {
    event_store: Arc<dyn MemoryEventStore>,
    projector: Arc<EventProjector>,
}

impl MemoryCommandHandler {
    pub async fn create_fact(&self, cmd: CreateFactCommand) -> Result<String, AlephError>;
    pub async fn update_content(&self, cmd: UpdateContentCommand) -> Result<(), AlephError>;
    pub async fn invalidate_fact(&self, cmd: InvalidateFactCommand) -> Result<(), AlephError>;
    pub async fn restore_fact(&self, cmd: RestoreFactCommand) -> Result<(), AlephError>;
    pub async fn record_access(&self, cmd: RecordAccessCommand) -> Result<(), AlephError>;
    pub async fn apply_decay(&self, cmd: ApplyDecayCommand) -> Result<usize, AlephError>;
    pub async fn consolidate_facts(&self, cmd: ConsolidateCommand) -> Result<String, AlephError>;
}
```

### Read Path — Unchanged

```
Caller → MemoryStore.hybrid_search(params) → LanceDB   // unchanged
Caller → MemoryStore.get_fact(id) → LanceDB             // unchanged
```

### Consistency Guarantees

- **Synchronous projection**: Event persisted then immediately projected (same await chain)
- **Idempotent projection**: Each event carries `(fact_id, seq)` for safe replay
- **Recovery**: If LanceDB projection fails, full rebuild from event stream

## 4. Time Travel & Queries

### MemoryTimeTraveler

```rust
/// Time-travel service for historical memory state reconstruction.
pub struct MemoryTimeTraveler {
    event_store: Arc<dyn MemoryEventStore>,
    projector: Arc<EventProjector>,
}

impl MemoryTimeTraveler {
    /// Reconstruct a fact's state at a specific timestamp.
    pub async fn fact_at(&self, fact_id: &str, at: i64) -> Result<Option<MemoryFact>, AlephError>;

    /// Get the complete event timeline for a fact.
    pub async fn fact_timeline(&self, fact_id: &str) -> Result<Vec<MemoryEventEnvelope>, AlephError>;

    /// Get all memory changes within a time range.
    pub async fn events_between(&self, from: i64, to: i64, limit: usize) -> Result<Vec<MemoryEventEnvelope>, AlephError>;

    /// Explain a fact's lifecycle (replaces AuditLogger.explain_fact).
    pub async fn explain_fact(&self, fact_id: &str) -> Result<FactExplanation, AlephError>;
}
```

## 5. Migration Strategy

### Phase 1: FactMigrated Synthesis

For all existing facts (including invalidated ones), generate a `FactMigrated` event as the starting point:

```rust
pub struct EventSourcingMigration {
    memory_store: MemoryBackend,
    event_store: Arc<dyn MemoryEventStore>,
}

impl EventSourcingMigration {
    pub async fn run(&self) -> Result<MigrationReport, AlephError> {
        let all_facts = self.memory_store.get_all_facts(true).await?;
        for fact in &all_facts {
            // Skip if already migrated (idempotent)
            // Emit FactMigrated with full snapshot, preserving original timestamp
        }
    }
}
```

### Rollout Sequence

```
1. Deploy new code (EventStore + Projector + CommandHandler)
2. Run EventSourcingMigration — generate FactMigrated for all existing facts
3. Switch write path: old CRUD → CommandHandler (feature flag)
4. Verify consistency: event-rebuilt state == LanceDB state
5. Stabilize: remove feature flag, deprecate direct CRUD
```

### AuditStore Evolution Path

```
Phase 1: MemoryEventStore implemented, AuditStore retained but #[deprecated]
Phase 2: AuditLogger reads from event stream instead of AuditStore
Phase 3: Remove AuditStore trait, AuditLogger queries MemoryEventStore directly
```

## 6. File Organization

| File | Purpose |
|------|---------|
| `core/src/memory/events/mod.rs` | MemoryEvent enum, EventEnvelope, EventActor |
| `core/src/memory/events/commands.rs` | Command structs (CreateFactCommand, etc.) |
| `core/src/memory/events/projector.rs` | EventProjector — event → LanceDB |
| `core/src/memory/events/handler.rs` | MemoryCommandHandler — write facade |
| `core/src/memory/events/traveler.rs` | MemoryTimeTraveler — time travel queries |
| `core/src/memory/events/migration.rs` | EventSourcingMigration |
| `core/src/memory/store/mod.rs` | MemoryEventStore trait (added alongside others) |
| `core/src/resilience/database/memory_events.rs` | SQLite implementation of MemoryEventStore |

## 7. Impact Analysis

### What Changes

- All fact write operations route through `MemoryCommandHandler`
- `AuditLogger` evolves to read from event stream
- Compression service emits `FactCreated` + `FactConsolidated` events
- Decay service emits `StrengthDecayed` + `FactInvalidated` events
- DreamDaemon emits events through CommandHandler

### What Stays the Same

- All 6 Store traits (MemoryStore, GraphStore, SessionStore, DreamStore, AuditStore, CompressionStore) — read side
- LanceDB as the search/retrieval engine
- Hybrid search, vector search, text search — all unchanged
- VFS path operations — unchanged
- MemoryFact struct — unchanged
- Embedding pipeline — unchanged

### Estimated Work

~12-15 implementation tasks, organized in phases:
1. Foundation: Event model + EventStore trait + SQLite schema (~3 tasks)
2. Core: Projector + CommandHandler + write path rewiring (~4 tasks)
3. Migration: FactMigrated synthesis + AuditStore deprecation (~2 tasks)
4. Time Travel: TimeTraveler + explain_fact evolution (~2 tasks)
5. Integration: Wire up compression/decay/dream services (~3 tasks)
