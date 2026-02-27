# POE Full Evolution Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Connect all broken wires in the POE system to create a complete event-driven closed loop — experiences are recorded, replayed, and drive progressive trust-based auto-approval.

**Architecture:** Event-driven with Skeleton/Pulse tiering. PoeManager emits domain events to PoeEventBus (tokio::broadcast). Four projectors consume events: CrystallizationProjector (LanceDB), MemoryProjector (Memory facts), TrustProjector (StateDB), MetricsProjector (skill_metrics). ExperienceReplayLayer queries LanceDB for pattern matching. TrustEvaluator drives auto-approval.

**Tech Stack:** Rust, tokio (broadcast channels), rusqlite (StateDatabase), LanceDB (poe_experiences table), Arrow (schema), serde (event serialization)

**Design Doc:** `docs/plans/2026-02-27-poe-full-evolution-design.md`

---

## Phase 1: Event Foundation + Crystallizer Wiring

### Task 1: Define PoeEvent types and PoeEventEnvelope

**Files:**
- Create: `core/src/poe/events.rs`
- Modify: `core/src/poe/mod.rs` (add `pub mod events;` and re-exports)

**Context:** The Memory system uses `MemoryEvent` enum with `#[serde(tag = "type")]` and `MemoryEventEnvelope` wrapper. We follow the exact same pattern. See `core/src/memory/events/mod.rs` for reference.

**Step 1: Write the failing test**

Add to `core/src/poe/events.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outcome_kind_serialization() {
        let kind = PoeOutcomeKind::Success;
        let json = serde_json::to_string(&kind).unwrap();
        let deserialized: PoeOutcomeKind = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, PoeOutcomeKind::Success));
    }

    #[test]
    fn test_event_type_tag() {
        let event = PoeEvent::ManifestCreated {
            task_id: "t1".into(),
            objective: "build auth".into(),
            hard_constraints_count: 3,
            soft_metrics_count: 1,
        };
        assert_eq!(event.event_type_tag(), "ManifestCreated");
        assert!(event.is_skeleton());
    }

    #[test]
    fn test_pulse_event_classification() {
        let event = PoeEvent::OperationAttempted {
            task_id: "t1".into(),
            attempt: 1,
            tokens_used: 5000,
        };
        assert_eq!(event.event_type_tag(), "OperationAttempted");
        assert!(!event.is_skeleton());
    }

    #[test]
    fn test_envelope_serialization_roundtrip() {
        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 2,
                total_tokens: 10000,
                duration_ms: 5000,
                best_distance: 0.1,
            },
            None,
        );
        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: PoeEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.task_id, "task-1");
        assert_eq!(deserialized.seq, 0);
        assert!(matches!(deserialized.tier, EventTier::Skeleton));
    }

    #[test]
    fn test_outcome_kind_to_satisfaction() {
        assert_eq!(PoeOutcomeKind::Success.to_satisfaction(), 1.0);
        assert_eq!(PoeOutcomeKind::StrategySwitch.to_satisfaction(), 0.3);
        assert_eq!(PoeOutcomeKind::BudgetExhausted.to_satisfaction(), 0.0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test poe::events::tests --lib 2>&1 | tail -5`
Expected: FAIL — module `events` not found

**Step 3: Implement PoeEvent, PoeOutcomeKind, EventTier, PoeEventEnvelope**

```rust
//! POE domain events — fact records of all POE lifecycle changes.
//!
//! Follows the Skeleton/Pulse classification from Memory events:
//! - Skeleton: Must persist immediately (ManifestCreated, ContractSigned, ValidationCompleted, OutcomeRecorded)
//! - Pulse: Can be buffered/dropped (OperationAttempted, TrustUpdated)

use serde::{Deserialize, Serialize};

/// POE domain event variants
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PoeEvent {
    // --- Skeleton Events (immediate persist) ---
    ManifestCreated {
        task_id: String,
        objective: String,
        hard_constraints_count: usize,
        soft_metrics_count: usize,
    },
    ContractSigned {
        task_id: String,
        auto_approved: bool,
        trust_score: Option<f32>,
    },
    ValidationCompleted {
        task_id: String,
        attempt: u8,
        passed: bool,
        distance_score: f32,
        hard_passed: usize,
        hard_total: usize,
    },
    OutcomeRecorded {
        task_id: String,
        outcome: PoeOutcomeKind,
        attempts: u8,
        total_tokens: u32,
        duration_ms: u64,
        best_distance: f32,
    },

    // --- Pulse Events (can be buffered) ---
    OperationAttempted {
        task_id: String,
        attempt: u8,
        tokens_used: u32,
    },
    TrustUpdated {
        pattern_id: String,
        old_score: f32,
        new_score: f32,
    },
}

impl PoeEvent {
    /// Returns the serde tag string for this event variant.
    pub fn event_type_tag(&self) -> &'static str {
        match self {
            PoeEvent::ManifestCreated { .. } => "ManifestCreated",
            PoeEvent::ContractSigned { .. } => "ContractSigned",
            PoeEvent::ValidationCompleted { .. } => "ValidationCompleted",
            PoeEvent::OutcomeRecorded { .. } => "OutcomeRecorded",
            PoeEvent::OperationAttempted { .. } => "OperationAttempted",
            PoeEvent::TrustUpdated { .. } => "TrustUpdated",
        }
    }

    /// Returns true if this is a Skeleton event (must persist immediately).
    pub fn is_skeleton(&self) -> bool {
        !matches!(
            self,
            PoeEvent::OperationAttempted { .. } | PoeEvent::TrustUpdated { .. }
        )
    }

    /// Extract the task_id from any event variant (if present).
    pub fn task_id(&self) -> Option<&str> {
        match self {
            PoeEvent::ManifestCreated { task_id, .. }
            | PoeEvent::ContractSigned { task_id, .. }
            | PoeEvent::ValidationCompleted { task_id, .. }
            | PoeEvent::OutcomeRecorded { task_id, .. }
            | PoeEvent::OperationAttempted { task_id, .. } => Some(task_id),
            PoeEvent::TrustUpdated { .. } => None,
        }
    }
}

/// Simplified outcome kind for event serialization
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PoeOutcomeKind {
    Success,
    StrategySwitch,
    BudgetExhausted,
}

impl PoeOutcomeKind {
    /// Map outcome to satisfaction score (0.0-1.0)
    pub fn to_satisfaction(self) -> f32 {
        match self {
            PoeOutcomeKind::Success => 1.0,
            PoeOutcomeKind::StrategySwitch => 0.3,
            PoeOutcomeKind::BudgetExhausted => 0.0,
        }
    }
}

/// Event persistence tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventTier {
    Skeleton,
    Pulse,
}

/// Event envelope — wraps a domain event with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoeEventEnvelope {
    pub id: i64,
    pub task_id: String,
    pub seq: u32,
    pub event: PoeEvent,
    pub tier: EventTier,
    pub timestamp: i64,
    pub correlation_id: Option<String>,
}

impl PoeEventEnvelope {
    pub fn new(
        task_id: String,
        seq: u32,
        event: PoeEvent,
        correlation_id: Option<String>,
    ) -> Self {
        let tier = if event.is_skeleton() {
            EventTier::Skeleton
        } else {
            EventTier::Pulse
        };
        Self {
            id: 0,
            task_id,
            seq,
            event,
            tier,
            timestamp: chrono::Utc::now().timestamp_millis(),
            correlation_id,
        }
    }
}
```

**Step 4: Register module in `core/src/poe/mod.rs`**

Add `pub mod events;` to module declarations. Add re-exports:

```rust
pub use events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind, EventTier};
```

**Step 5: Run tests**

Run: `cd core && cargo test poe::events::tests --lib 2>&1 | tail -10`
Expected: 4 tests passed

**Step 6: Commit**

```bash
git add core/src/poe/events.rs core/src/poe/mod.rs
git commit -m "poe: add domain event types with Skeleton/Pulse classification"
```

---

### Task 2: Implement PoeEventBus

**Files:**
- Create: `core/src/poe/event_bus.rs`
- Modify: `core/src/poe/mod.rs` (add `pub mod event_bus;` and re-export)

**Context:** Follow the `DaemonEventBus` pattern in `core/src/daemon/event_bus.rs`: broadcast channel, `new(capacity)`, `emit()`, `subscribe()`.

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};

    #[tokio::test]
    async fn test_emit_and_receive() {
        let bus = PoeEventBus::new(64);
        let mut rx = bus.subscribe();

        let envelope = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::ManifestCreated {
                task_id: "t1".into(),
                objective: "test".into(),
                hard_constraints_count: 1,
                soft_metrics_count: 0,
            },
            None,
        );

        bus.emit(envelope.clone());
        let received = rx.recv().await.unwrap();
        assert_eq!(received.task_id, "t1");
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = PoeEventBus::new(64);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let envelope = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "t1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 1000,
                duration_ms: 500,
                best_distance: 0.1,
            },
            None,
        );

        bus.emit(envelope);
        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert_eq!(r1.task_id, r2.task_id);
    }

    #[test]
    fn test_default_capacity() {
        let bus = PoeEventBus::default();
        // Should not panic
        let _rx = bus.subscribe();
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test poe::event_bus::tests --lib 2>&1 | tail -5`

**Step 3: Implement PoeEventBus**

```rust
//! POE event bus — broadcast channel for domain events.

use tokio::sync::broadcast;
use tracing::debug;

use super::events::PoeEventEnvelope;

const DEFAULT_CAPACITY: usize = 1024;

/// Broadcast-based event bus for POE domain events.
#[derive(Debug, Clone)]
pub struct PoeEventBus {
    sender: broadcast::Sender<PoeEventEnvelope>,
}

impl PoeEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn emit(&self, envelope: PoeEventEnvelope) {
        let event_type = envelope.event.event_type_tag();
        // Ignore error if no receivers (not an error condition)
        let _ = self.sender.send(envelope);
        debug!("POE event emitted: {}", event_type);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PoeEventEnvelope> {
        self.sender.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for PoeEventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}
```

**Step 4: Register in mod.rs**

Add `pub mod event_bus;` and `pub use event_bus::PoeEventBus;`

**Step 5: Run tests**

Run: `cd core && cargo test poe::event_bus::tests --lib 2>&1 | tail -10`
Expected: 3 tests passed

**Step 6: Commit**

```bash
git add core/src/poe/event_bus.rs core/src/poe/mod.rs
git commit -m "poe: add PoeEventBus (broadcast-based domain event bus)"
```

---

### Task 3: StateDatabase poe_events table + CRUD

**Files:**
- Modify: `core/src/resilience/database/state_database.rs` (add table creation in `initialize()`)
- Create: `core/src/resilience/database/poe_events.rs` (CRUD operations)
- Modify: `core/src/resilience/database/mod.rs` (add `pub mod poe_events;`)

**Context:** Follow the exact pattern of `core/src/resilience/database/memory_events.rs`: async methods using `spawn_blocking`, `MemoryEventRow` helper struct, `Result<T, AlephError>`.

**Step 1: Add poe_events table to StateDatabase::initialize()**

In `state_database.rs`, find the `initialize()` or table creation section. Add:

```sql
CREATE TABLE IF NOT EXISTS poe_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    event_json TEXT NOT NULL,
    tier TEXT NOT NULL CHECK(tier IN ('skeleton', 'pulse')),
    timestamp INTEGER NOT NULL,
    correlation_id TEXT,
    UNIQUE(task_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_pe_task_id ON poe_events(task_id);
CREATE INDEX IF NOT EXISTS idx_pe_event_type ON poe_events(event_type);
CREATE INDEX IF NOT EXISTS idx_pe_timestamp ON poe_events(timestamp);
```

**Step 2: Create poe_events.rs CRUD**

Implement these methods on `StateDatabase`:

```rust
pub async fn append_poe_event(&self, envelope: &PoeEventEnvelope) -> Result<i64, AlephError>
pub async fn get_poe_events_for_task(&self, task_id: &str) -> Result<Vec<PoeEventEnvelope>, AlephError>
pub async fn get_poe_events_in_range(&self, from: i64, to: i64, limit: usize) -> Result<Vec<PoeEventEnvelope>, AlephError>
pub async fn count_poe_events(&self, event_type_filter: Option<&str>) -> Result<usize, AlephError>
```

Follow the `memory_events.rs` pattern: internal `PoeEventRow` struct for deserialization, `spawn_blocking` with `Arc<Mutex<Connection>>`.

**Step 3: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::events::*;
    use crate::resilience::database::StateDatabase;

    #[tokio::test]
    async fn test_append_and_query_poe_event() {
        let db = StateDatabase::in_memory().unwrap();
        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::ManifestCreated {
                task_id: "task-1".into(),
                objective: "test".into(),
                hard_constraints_count: 2,
                soft_metrics_count: 1,
            },
            Some("session-1".into()),
        );
        let id = db.append_poe_event(&envelope).await.unwrap();
        assert!(id > 0);

        let events = db.get_poe_events_for_task("task-1").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task_id, "task-1");
    }

    #[tokio::test]
    async fn test_unique_constraint_on_task_seq() {
        let db = StateDatabase::in_memory().unwrap();
        let e1 = PoeEventEnvelope::new("t1".into(), 0, PoeEvent::OperationAttempted {
            task_id: "t1".into(), attempt: 1, tokens_used: 100,
        }, None);
        let e2 = PoeEventEnvelope::new("t1".into(), 0, PoeEvent::OperationAttempted {
            task_id: "t1".into(), attempt: 2, tokens_used: 200,
        }, None);

        db.append_poe_event(&e1).await.unwrap();
        assert!(db.append_poe_event(&e2).await.is_err());
    }

    #[tokio::test]
    async fn test_count_poe_events() {
        let db = StateDatabase::in_memory().unwrap();
        let e1 = PoeEventEnvelope::new("t1".into(), 0, PoeEvent::ManifestCreated {
            task_id: "t1".into(), objective: "a".into(),
            hard_constraints_count: 0, soft_metrics_count: 0,
        }, None);
        let e2 = PoeEventEnvelope::new("t1".into(), 1, PoeEvent::OutcomeRecorded {
            task_id: "t1".into(), outcome: PoeOutcomeKind::Success,
            attempts: 1, total_tokens: 1000, duration_ms: 500, best_distance: 0.1,
        }, None);

        db.append_poe_event(&e1).await.unwrap();
        db.append_poe_event(&e2).await.unwrap();

        assert_eq!(db.count_poe_events(None).await.unwrap(), 2);
        assert_eq!(db.count_poe_events(Some("ManifestCreated")).await.unwrap(), 1);
    }
}
```

**Step 4: Run tests**

Run: `cd core && cargo test resilience::database::poe_events::tests --lib 2>&1 | tail -10`
Expected: 3 tests passed

**Step 5: Commit**

```bash
git add core/src/resilience/database/state_database.rs core/src/resilience/database/poe_events.rs core/src/resilience/database/mod.rs
git commit -m "resilience: add poe_events table and CRUD operations"
```

---

### Task 4: PoeManager emits domain events

**Files:**
- Modify: `core/src/poe/manager.rs` (add PoeEventBus field, emit events at lifecycle points)

**Context:** PoeManager already has `meta_cognition: Option<Arc<dyn MetaCognitionCallback>>`. Add optional `event_bus: Option<Arc<PoeEventBus>>` with a builder method `with_event_bus()`. Emit events at key points without changing the P→O→E loop structure.

**Step 1: Add event_bus field and builder**

Add to PoeManager struct:
```rust
event_bus: Option<Arc<PoeEventBus>>,
event_seq: std::sync::atomic::AtomicU32,
```

Add builder:
```rust
pub fn with_event_bus(mut self, bus: Arc<PoeEventBus>) -> Self {
    self.event_bus = Some(bus);
    self
}
```

**Step 2: Add emit helper**

```rust
fn emit_event(&self, task_id: &str, event: PoeEvent) {
    if let Some(ref bus) = self.event_bus {
        let seq = self.event_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        bus.emit(PoeEventEnvelope::new(task_id.into(), seq, event, None));
    }
}
```

**Step 3: Emit events at lifecycle points**

In `execute()`:
- After receiving manifest: emit `ManifestCreated`
- After each worker execution: emit `OperationAttempted`
- After each validation: emit `ValidationCompleted`
- At the end (success/failure): emit `OutcomeRecorded`

**Step 4: Write test**

```rust
#[tokio::test]
async fn test_manager_emits_events() {
    let bus = Arc::new(PoeEventBus::default());
    let mut rx = bus.subscribe();
    // ... create manager with_event_bus(bus) ...
    // ... execute a task ...
    // Verify at least ManifestCreated and OutcomeRecorded events received
}
```

**Step 5: Run tests**

Run: `cd core && cargo test poe::manager::tests --lib 2>&1 | tail -10`

**Step 6: Commit**

```bash
git add core/src/poe/manager.rs
git commit -m "poe: PoeManager emits domain events via PoeEventBus"
```

---

### Task 5: Wire Crystallizer in PoeRunManager

**Files:**
- Modify: `core/src/poe/services/run_service.rs` (add recorder field to PoeRunManager, pass to PoeManager)

**Context:** `PoeRunManager` currently creates `PoeManager` without a recorder. Add `recorder: Arc<dyn ExperienceRecorder>` field. In `start_run()`, pass recorder to PoeManager via existing `with_recorder()` or constructor.

**Step 1: Add recorder field to PoeRunManager**

```rust
pub struct PoeRunManager<W: Worker + 'static> {
    // ... existing fields ...
    recorder: Arc<dyn ExperienceRecorder>,
    poe_event_bus: Arc<PoeEventBus>,
}
```

**Step 2: Update constructor**

```rust
pub fn new(
    event_bus: Arc<GatewayEventBus>,
    poe_event_bus: Arc<PoeEventBus>,
    recorder: Arc<dyn ExperienceRecorder>,
    worker_factory: WorkerFactory<W>,
    validator_factory: ValidatorFactory,
    default_config: PoeConfig,
) -> Self
```

**Step 3: Pass recorder and event_bus to PoeManager in execute_poe_task()**

When creating PoeManager inside the spawned task, add:
```rust
let manager = PoeManager::new(worker, validator, config)
    .with_event_bus(self.poe_event_bus.clone())
    // recorder is already passed through existing mechanism
```

**Step 4: Update all call sites** that construct PoeRunManager (gateway handlers, tests).

For tests/stubs, use `Arc::new(NoOpRecorder)` and `Arc::new(PoeEventBus::default())`.

**Step 5: Run tests**

Run: `cd core && cargo test poe::services --lib 2>&1 | tail -10`

**Step 6: Commit**

```bash
git add core/src/poe/services/run_service.rs core/src/gateway/handlers/poe.rs
git commit -m "poe: wire Crystallizer and PoeEventBus into PoeRunManager"
```

---

### Task 6: LanceDB poe_experiences schema + CrystallizationProjector

**Files:**
- Create: `core/src/poe/projectors/mod.rs`
- Create: `core/src/poe/projectors/crystallization.rs`
- Modify: `core/src/memory/store/lance/schema.rs` (add `poe_experiences_schema()`)
- Modify: `core/src/poe/mod.rs` (add `pub mod projectors;`)

**Context:** Follow `facts_schema()` pattern in `core/src/memory/store/lance/schema.rs`. Arrow Schema with typed fields including vector field for embeddings.

**Step 1: Define poe_experiences_schema()**

```rust
pub fn poe_experiences_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("task_id", DataType::Utf8, false),
        Field::new("objective", DataType::Utf8, false),
        Field::new("pattern_id", DataType::Utf8, false),
        Field::new("tool_sequence_json", DataType::Utf8, false),
        Field::new("parameter_mapping", DataType::Utf8, true),
        Field::new("satisfaction", DataType::Float32, false),
        Field::new("distance_score", DataType::Float32, false),
        Field::new("attempts", DataType::UInt8, false),
        Field::new("duration_ms", DataType::UInt64, false),
        Field::new("created_at", DataType::Int64, false),
        // Multi-dimension vectors (same pattern as facts table)
        vector_field("vec_768", 768),
        vector_field("vec_1024", 1024),
        vector_field("vec_1536", 1536),
    ]))
}
```

**Step 2: Create CrystallizationProjector**

```rust
pub struct CrystallizationProjector {
    embedder: Arc<dyn EmbeddingProvider>,
    // LanceDB table handle (same pattern as LanceMemoryBackend)
}

impl CrystallizationProjector {
    pub async fn handle(&self, envelope: &PoeEventEnvelope) -> Result<()> {
        if let PoeEvent::OutcomeRecorded { task_id, outcome, attempts, total_tokens, duration_ms, best_distance } = &envelope.event {
            // Generate embedding, write to poe_experiences table
        }
    }
}
```

**Step 3: Write tests**

Test that:
- Schema has correct number of columns (15)
- Schema includes vector fields
- CrystallizationProjector ignores non-OutcomeRecorded events

**Step 4: Run tests and commit**

```bash
git add core/src/poe/projectors/ core/src/memory/store/lance/schema.rs core/src/poe/mod.rs
git commit -m "poe: add poe_experiences schema and CrystallizationProjector"
```

---

## Phase 2: Learning Feedback Loop

### Task 7: ExperienceStore trait + LanceDB implementation

**Files:**
- Create: `core/src/poe/crystallization/experience_store.rs`
- Modify: `core/src/poe/crystallization/mod.rs` (add module)

**Context:** Follow the `MemoryStore` trait pattern from `core/src/memory/store/`. Async trait with `Send + Sync` bounds. The LanceDB implementation uses the table handle pattern from `LanceMemoryBackend`.

**Step 1: Define PoeExperience struct**

```rust
pub struct PoeExperience {
    pub id: String,
    pub task_id: String,
    pub objective: String,
    pub pattern_id: String,
    pub tool_sequence_json: String,
    pub parameter_mapping: Option<String>,
    pub satisfaction: f32,
    pub distance_score: f32,
    pub attempts: u8,
    pub duration_ms: u64,
    pub created_at: i64,
}
```

**Step 2: Define ExperienceStore trait**

```rust
#[async_trait]
pub trait ExperienceStore: Send + Sync {
    async fn insert(&self, experience: PoeExperience, embedding: &[f32]) -> Result<()>;
    async fn vector_search(
        &self,
        query: &[f32],
        limit: usize,
        threshold: f64,
    ) -> Result<Vec<(PoeExperience, f64)>>;
    async fn get_by_pattern_id(&self, pattern_id: &str) -> Result<Vec<PoeExperience>>;
}
```

**Step 3: Implement LanceExperienceStore**

Use `lancedb::Table` handle, Arrow RecordBatch for inserts, vector search with distance filter.

**Step 4: Write unit tests with mock**

**Step 5: Run tests and commit**

```bash
git commit -m "poe: add ExperienceStore trait and LanceDB implementation"
```

---

### Task 8: Wire ExperienceReplayLayer to LanceDB

**Files:**
- Modify: `core/src/dispatcher/experience_replay_layer.rs` (replace empty vec with real search)

**Context:** Currently `try_match()` always returns `Ok(None)` because `candidates` is hardcoded to `Vec::new()`. Replace with real `ExperienceStore::vector_search()` call.

**Step 1: Add ExperienceStore to ExperienceReplayLayer**

Replace `_db: MemoryBackend` with `experience_store: Arc<dyn ExperienceStore>`.

**Step 2: Implement real search in try_match()**

```rust
let candidates = self.experience_store
    .vector_search(&intent_vector, self.config.max_candidates, self.config.similarity_threshold)
    .await?;
```

Map `PoeExperience` to the existing `Experience` type used by `select_best_match()`.

**Step 3: Update tests**

**Step 4: Run tests and commit**

```bash
git commit -m "dispatcher: wire ExperienceReplayLayer to LanceDB experience store"
```

---

### Task 9: Trust scores table + TrustProjector

**Files:**
- Modify: `core/src/resilience/database/state_database.rs` (add poe_trust_scores table)
- Create: `core/src/resilience/database/poe_trust.rs` (CRUD)
- Create: `core/src/poe/projectors/trust.rs`

**Context:** Trust projector consumes `OutcomeRecorded` events, updates pattern-level success metrics, calculates trust score with time decay.

**Step 1: Add poe_trust_scores table**

```sql
CREATE TABLE IF NOT EXISTS poe_trust_scores (
    pattern_id TEXT PRIMARY KEY,
    total_executions INTEGER NOT NULL DEFAULT 0,
    successful_executions INTEGER NOT NULL DEFAULT 0,
    trust_score REAL NOT NULL DEFAULT 0.0,
    last_updated INTEGER NOT NULL
);
```

**Step 2: CRUD methods**

```rust
pub async fn upsert_trust_score(&self, pattern_id: &str, success: bool) -> Result<f32, AlephError>
pub async fn get_trust_score(&self, pattern_id: &str) -> Result<Option<TrustScoreRow>, AlephError>
```

**Step 3: TrustProjector**

```rust
pub struct TrustProjector {
    db: Arc<StateDatabase>,
}

impl TrustProjector {
    pub async fn handle(&self, envelope: &PoeEventEnvelope) -> Result<()> {
        if let PoeEvent::OutcomeRecorded { outcome, .. } = &envelope.event {
            let pattern_id = extract_pattern_id_from_task_id(&envelope.task_id);
            let success = matches!(outcome, PoeOutcomeKind::Success);
            self.db.upsert_trust_score(&pattern_id, success).await?;
        }
        Ok(())
    }
}
```

**Step 4: Tests + Commit**

```bash
git commit -m "poe: add trust scoring system with TrustProjector"
```

---

### Task 10: Wire TrustEvaluator into ContractService

**Files:**
- Modify: `core/src/poe/services/contract_service.rs` (add TrustEvaluator field, call in prepare())
- Modify: `core/src/poe/trust.rs` (update ExperienceTrustEvaluator to query StateDatabase)

**Context:** `ExperienceTrustEvaluator` already exists with the right interface but hardcodes results. Wire it to query `poe_trust_scores` table. In `PoeContractService::prepare()`, call evaluator before storing pending contract.

**Step 1: Update ExperienceTrustEvaluator to use StateDatabase**

```rust
pub struct ExperienceTrustEvaluator {
    db: Arc<StateDatabase>,
}

impl TrustEvaluator for ExperienceTrustEvaluator {
    fn evaluate(&self, manifest: &SuccessManifest, context: &TrustContext) -> AutoApprovalDecision {
        // Query trust_scores by pattern_id
        // Apply thresholds: >= 0.8 + 5 execs → AutoApprove, etc.
    }
}
```

**Step 2: Add trust_evaluator to PoeContractService**

**Step 3: Call in prepare() before storing pending contract**

**Step 4: Tests + Commit**

```bash
git commit -m "poe: wire TrustEvaluator into ContractService for progressive auto-approval"
```

---

### Task 11: Contract persistence (SQLite)

**Files:**
- Modify: `core/src/resilience/database/state_database.rs` (add poe_contracts table)
- Create: `core/src/resilience/database/poe_contracts.rs` (CRUD)
- Modify: `core/src/poe/contract_store.rs` (replace HashMap with StateDatabase)

**Context:** `PendingContractStore` currently uses `Arc<RwLock<HashMap<String, PendingContract>>>`. Replace with SQLite-backed persistence via StateDatabase.

**Step 1: Add poe_contracts table**

```sql
CREATE TABLE IF NOT EXISTS poe_contracts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'signed', 'rejected', 'expired')),
    created_at INTEGER NOT NULL,
    signed_at INTEGER,
    expires_at INTEGER,
    amendments_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_pc_status ON poe_contracts(status);
CREATE INDEX IF NOT EXISTS idx_pc_task_id ON poe_contracts(task_id);
```

**Step 2: CRUD methods on StateDatabase**

```rust
pub async fn insert_poe_contract(&self, id: &str, task_id: &str, manifest_json: &str, created_at: i64, expires_at: Option<i64>) -> Result<(), AlephError>
pub async fn get_poe_contract(&self, id: &str) -> Result<Option<PoeContractRow>, AlephError>
pub async fn update_poe_contract_status(&self, id: &str, status: &str, signed_at: Option<i64>) -> Result<bool, AlephError>
pub async fn list_pending_poe_contracts(&self) -> Result<Vec<PoeContractRow>, AlephError>
pub async fn delete_expired_poe_contracts(&self, now: i64) -> Result<usize, AlephError>
```

**Step 3: Refactor PendingContractStore**

Replace `HashMap` internals with `StateDatabase` calls. Keep the same public API.

**Step 4: Update PoeContractService** — construct PendingContractStore with `db` parameter

**Step 5: Tests + Commit**

```bash
git commit -m "poe: persist contracts in SQLite via StateDatabase"
```

---

## Phase 3: Deep Integration

### Task 12: MemoryProjector implementation

**Files:**
- Create: `core/src/poe/projectors/memory.rs`
- Modify: `core/src/poe/projectors/mod.rs`

**Context:** Consumes `OutcomeRecorded` events. On success → create "poe_experience" fact (core tier). On failure → create "lessons_learned" fact (working tier). Uses `MemoryStore` trait.

**Step 1: Implement MemoryProjector**

```rust
pub struct MemoryProjector {
    memory_backend: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl MemoryProjector {
    pub async fn handle(&self, envelope: &PoeEventEnvelope) -> Result<()> {
        if let PoeEvent::OutcomeRecorded { task_id, outcome, best_distance, .. } = &envelope.event {
            match outcome {
                PoeOutcomeKind::Success => { /* create core fact */ }
                PoeOutcomeKind::BudgetExhausted => { /* create lessons_learned fact */ }
                _ => {}
            }
        }
        Ok(())
    }
}
```

**Step 2: Tests + Commit**

```bash
git commit -m "poe: add MemoryProjector for POE outcome → Memory fact projection"
```

---

### Task 13: Projector runner (spawn all projectors)

**Files:**
- Create: `core/src/poe/projectors/runner.rs`
- Modify: `core/src/poe/projectors/mod.rs`

**Context:** A background task that subscribes to PoeEventBus and dispatches events to all projectors. Spawned once at Gateway startup.

**Step 1: Implement ProjectorRunner**

```rust
pub struct ProjectorRunner {
    crystallization: Option<CrystallizationProjector>,
    memory: Option<MemoryProjector>,
    trust: Option<TrustProjector>,
}

impl ProjectorRunner {
    pub async fn run(self, mut rx: broadcast::Receiver<PoeEventEnvelope>) {
        while let Ok(envelope) = rx.recv().await {
            // Dispatch to each projector, log errors but don't fail
            if let Some(ref c) = self.crystallization {
                if let Err(e) = c.handle(&envelope).await {
                    tracing::warn!("CrystallizationProjector error: {}", e);
                }
            }
            // ... same for memory, trust
        }
    }
}
```

**Step 2: Tests + Commit**

```bash
git commit -m "poe: add ProjectorRunner for event dispatch to all projectors"
```

---

### Task 14: Dispatcher hint injection (experience replay)

**Files:**
- Modify: `core/src/poe/services/run_service.rs` (query experience replay before starting task)

**Context:** Before starting a POE task, query ExperienceReplayLayer. If a match is found, inject a hint into PoePromptContext.

**Step 1: Add ExperienceReplayLayer to PoeRunManager**

Optional field. If present, query before `start_run()`.

**Step 2: Inject hint**

```rust
if let Some(ref replay) = self.experience_replay {
    if let Ok(Some(match_result)) = replay.try_match(&objective).await {
        poe_context.current_hint = Some(format!(
            "Similar task matched (confidence: {:.0}%): {}",
            match_result.confidence * 100.0,
            match_result.tool_sequence,
        ));
    }
}
```

**Step 3: Tests + Commit**

```bash
git commit -m "poe: inject experience replay hints into PoePromptContext"
```

---

### Task 15: Git-based StateSnapshot capture/restore

**Files:**
- Modify: `core/src/poe/worker/mod.rs` (update StateSnapshot with git-based capture/restore)

**Context:** Current StateSnapshot only stores file hashes for verification. Add git-based capture (stash create) and restore (checkout + stash apply).

**Step 1: Add stash_hash field**

```rust
pub struct StateSnapshot {
    pub timestamp: DateTime<Utc>,
    pub workspace: PathBuf,
    pub file_hashes: Vec<(PathBuf, String)>,
    pub stash_hash: Option<String>,  // NEW: git stash object hash
}
```

**Step 2: Implement capture()**

```rust
pub async fn capture(workspace: &Path) -> Result<Self> {
    let output = tokio::process::Command::new("git")
        .args(["stash", "create", "--include-untracked"])
        .current_dir(workspace)
        .output()
        .await?;
    let stash_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(Self {
        timestamp: Utc::now(),
        workspace: workspace.to_path_buf(),
        file_hashes: Vec::new(),
        stash_hash: if stash_hash.is_empty() { None } else { Some(stash_hash) },
    })
}
```

**Step 3: Implement restore()**

```rust
pub async fn restore(&self) -> Result<()> {
    if let Some(ref hash) = self.stash_hash {
        tokio::process::Command::new("git")
            .args(["checkout", "--", "."])
            .current_dir(&self.workspace)
            .output().await?;
        tokio::process::Command::new("git")
            .args(["stash", "apply", hash])
            .current_dir(&self.workspace)
            .output().await?;
    }
    Ok(())
}
```

**Step 4: Add has_git() helper**

```rust
pub async fn has_git(workspace: &Path) -> bool {
    tokio::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(workspace)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
```

**Step 5: Tests + Commit**

```bash
git commit -m "poe: implement git-based StateSnapshot capture/restore"
```

---

### Task 16: PoeManager snapshot integration

**Files:**
- Modify: `core/src/poe/manager.rs` (capture snapshot before operation, restore on retry)

**Context:** In `execute()`, capture a StateSnapshot before the Operation phase. On validation failure + retry, restore the snapshot to clean workspace.

**Step 1: Add workspace field to PoeManager**

```rust
workspace: Option<PathBuf>,
```

Builder: `pub fn with_workspace(mut self, path: PathBuf) -> Self`

**Step 2: In execute(), capture before worker.execute()**

```rust
let snapshot = if let Some(ref ws) = self.workspace {
    StateSnapshot::capture(ws).await.ok()
} else {
    None
};
```

**Step 3: On retry, restore snapshot**

```rust
if let Some(ref snap) = snapshot {
    if let Err(e) = snap.restore().await {
        tracing::warn!("Snapshot restore failed: {}", e);
    }
}
```

**Step 4: Tests + Commit**

```bash
git commit -m "poe: integrate StateSnapshot into PoeManager retry loop"
```

---

## Phase 4: Verification

### Task 17: Integration test — event loop round-trip

**Files:**
- Create: `core/tests/poe_event_loop_integration.rs`

**Context:** End-to-end test: create PoeEventBus → subscribe → PoeManager with event_bus → execute task → verify events received in correct order.

**Step 1: Write integration test**

```rust
#[tokio::test]
async fn test_poe_event_loop_round_trip() {
    let bus = Arc::new(PoeEventBus::default());
    let mut rx = bus.subscribe();

    // Create PoeManager with mock worker, bus
    // Execute a simple task
    // Collect all events from rx
    // Assert: ManifestCreated → OperationAttempted → ValidationCompleted → OutcomeRecorded
}
```

**Step 2: Run and commit**

```bash
git commit -m "test: add POE event loop integration test"
```

---

### Task 18: Integration test — trust and contract round-trip

**Files:**
- Create: `core/tests/poe_trust_integration.rs`

**Context:** Test trust evaluation flow: insert trust scores → create contract → verify auto-approval decision.

**Step 1: Write integration test**

```rust
#[tokio::test]
async fn test_trust_auto_approval_with_history() {
    let db = StateDatabase::in_memory().unwrap();
    // Insert 5 successful executions for pattern "poe-create-rust-file"
    // Create ExperienceTrustEvaluator with db
    // Evaluate manifest → should return AutoApprove
}

#[tokio::test]
async fn test_trust_requires_signature_for_new_pattern() {
    // No history → should return RequireSignature
}
```

**Step 2: Run and commit**

```bash
git commit -m "test: add POE trust evaluation integration test"
```

---

### Task 19: Full build and test verification

**Files:** None (verification only)

**Step 1: Run full test suite**

```bash
cd core && cargo test 2>&1 | tail -20
```

Expected: All tests pass (except pre-existing flaky benchmark)

**Step 2: Run integration tests**

```bash
cargo test --test poe_event_loop_integration --test poe_trust_integration --test poe_interceptor_integration --test poe_prompt_layer_integration 2>&1 | tail -20
```

Expected: All integration tests pass

**Step 3: Run doctests**

```bash
cargo test --doc 2>&1 | tail -10
```

Expected: All doctests pass

**Step 4: Verify no warnings in POE module**

```bash
cargo build 2>&1 | grep -i "warning.*poe"
```

Expected: No POE-related warnings (other module warnings OK)

**Step 5: Commit any final fixes**

```bash
git commit -m "poe: final verification — all tests pass"
```
