# Memory Event Sourcing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Event Sourcing to the memory system using CQRS Light — events in SQLite as source of truth, LanceDB as read-side projection.

**Architecture:** All fact mutations route through `MemoryCommandHandler` → append event to SQLite → project to LanceDB. Read path (MemoryStore trait) unchanged. `MemoryTimeTraveler` reconstructs historical state by replaying events.

**Tech Stack:** Rust, rusqlite (SQLite), serde_json, async-trait, LanceDB (existing)

**Design Doc:** `docs/plans/2026-02-26-memory-event-sourcing-design.md`

---

## Phase 1: Foundation — Event Model + EventStore + SQLite Schema

### Task 1: Create MemoryEvent enum and EventEnvelope types

**Files:**
- Create: `core/src/memory/events/mod.rs`
- Modify: `core/src/memory/mod.rs:17-58` (add `pub mod events;`)
- Modify: `core/src/memory/mod.rs:63-90` (add re-exports)
- Test: inline `#[cfg(test)]` in `core/src/memory/events/mod.rs`

**Step 1: Write the failing test**

Create `core/src/memory/events/mod.rs` with a test that exercises the event model:

```rust
// core/src/memory/events/mod.rs

//! Memory Event Sourcing
//!
//! Domain events for the memory system. Each event is an immutable,
//! append-only record of a fact mutation. Events are the source of truth;
//! LanceDB is a read-side projection.

pub mod commands;
pub mod handler;
pub mod migration;
pub mod projector;
pub mod traveler;

use serde::{Deserialize, Serialize};

use crate::memory::context::{
    FactSource, FactType, MemoryScope, MemoryTier,
};

// ---------------------------------------------------------------------------
// EventActor
// ---------------------------------------------------------------------------

/// Who or what caused a memory event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventActor {
    Agent,
    User,
    System,
    Migration,
}

impl std::fmt::Display for EventActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Agent => write!(f, "agent"),
            Self::User => write!(f, "user"),
            Self::System => write!(f, "system"),
            Self::Migration => write!(f, "migration"),
        }
    }
}

impl std::str::FromStr for EventActor {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "agent" => Ok(Self::Agent),
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            "migration" => Ok(Self::Migration),
            other => Err(format!("Unknown EventActor: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// TierTransitionTrigger
// ---------------------------------------------------------------------------

/// What caused a tier transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TierTransitionTrigger {
    Consolidation,
    Reinforcement,
    Decay,
}

impl std::fmt::Display for TierTransitionTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Consolidation => write!(f, "consolidation"),
            Self::Reinforcement => write!(f, "reinforcement"),
            Self::Decay => write!(f, "decay"),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryEvent
// ---------------------------------------------------------------------------

/// Memory domain event — the atomic unit of change in the memory system.
///
/// Each variant carries enough data to reconstruct the fact's state
/// when replayed in sequence.
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
        field: String,
        old_value: String,
        new_value: String,
    },

    // === Tier transitions ===
    TierTransitioned {
        fact_id: String,
        from_tier: MemoryTier,
        to_tier: MemoryTier,
        trigger: TierTransitionTrigger,
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
        snapshot: serde_json::Value,
    },
}

impl MemoryEvent {
    /// Extract the fact_id from any event variant.
    pub fn fact_id(&self) -> &str {
        match self {
            Self::FactCreated { fact_id, .. }
            | Self::FactContentUpdated { fact_id, .. }
            | Self::FactMetadataUpdated { fact_id, .. }
            | Self::TierTransitioned { fact_id, .. }
            | Self::FactAccessed { fact_id, .. }
            | Self::StrengthDecayed { fact_id, .. }
            | Self::FactInvalidated { fact_id, .. }
            | Self::FactRestored { fact_id, .. }
            | Self::FactDeleted { fact_id, .. }
            | Self::FactConsolidated { fact_id, .. }
            | Self::FactMigrated { fact_id, .. } => fact_id,
        }
    }

    /// Return the serde tag for this variant (used for DB storage).
    pub fn event_type_tag(&self) -> &'static str {
        match self {
            Self::FactCreated { .. } => "FactCreated",
            Self::FactContentUpdated { .. } => "FactContentUpdated",
            Self::FactMetadataUpdated { .. } => "FactMetadataUpdated",
            Self::TierTransitioned { .. } => "TierTransitioned",
            Self::FactAccessed { .. } => "FactAccessed",
            Self::StrengthDecayed { .. } => "StrengthDecayed",
            Self::FactInvalidated { .. } => "FactInvalidated",
            Self::FactRestored { .. } => "FactRestored",
            Self::FactDeleted { .. } => "FactDeleted",
            Self::FactConsolidated { .. } => "FactConsolidated",
            Self::FactMigrated { .. } => "FactMigrated",
        }
    }

    /// Skeleton events must be persisted immediately; Pulse events may be buffered.
    pub fn is_skeleton(&self) -> bool {
        !matches!(self, Self::FactAccessed { .. } | Self::StrengthDecayed { .. })
    }
}

// ---------------------------------------------------------------------------
// MemoryEventEnvelope
// ---------------------------------------------------------------------------

/// Wrapper around a domain event with ordering and tracing metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEventEnvelope {
    /// Auto-increment global ID (assigned by SQLite on insert; 0 before insert).
    pub id: i64,
    /// Aggregate ID — the fact this event belongs to.
    pub fact_id: String,
    /// Per-fact sequence number (monotonically increasing per fact_id).
    pub seq: u64,
    /// The domain event payload.
    pub event: MemoryEvent,
    /// Who caused this event.
    pub actor: EventActor,
    /// Unix timestamp (seconds).
    pub timestamp: i64,
    /// Optional correlation to a task or session.
    pub correlation_id: Option<String>,
}

impl MemoryEventEnvelope {
    /// Build a new envelope. `id` is set to 0 (assigned by DB on insert).
    pub fn new(
        fact_id: String,
        seq: u64,
        event: MemoryEvent,
        actor: EventActor,
        correlation_id: Option<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            id: 0,
            fact_id,
            seq,
            event,
            actor,
            timestamp: now,
            correlation_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_event_serde_roundtrip() {
        let event = MemoryEvent::FactCreated {
            fact_id: "fact-001".into(),
            content: "User prefers Rust".into(),
            fact_type: FactType::Preference,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            path: "aleph://user/preferences/language".into(),
            namespace: "owner".into(),
            workspace: "default".into(),
            confidence: 0.85,
            source: FactSource::Extracted,
            source_memory_ids: vec!["mem-001".into()],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("FactCreated"));
        let parsed: MemoryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fact_id(), "fact-001");
    }

    #[test]
    fn test_event_type_tag() {
        let event = MemoryEvent::FactAccessed {
            fact_id: "f1".into(),
            query: Some("rust".into()),
            relevance_score: Some(0.9),
            used_in_response: true,
            new_access_count: 5,
        };
        assert_eq!(event.event_type_tag(), "FactAccessed");
        assert!(!event.is_skeleton()); // Pulse event
    }

    #[test]
    fn test_skeleton_classification() {
        let skeleton = MemoryEvent::FactCreated {
            fact_id: "f".into(),
            content: "c".into(),
            fact_type: FactType::Learning,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            path: "p".into(),
            namespace: "owner".into(),
            workspace: "default".into(),
            confidence: 1.0,
            source: FactSource::Extracted,
            source_memory_ids: vec![],
        };
        assert!(skeleton.is_skeleton());

        let pulse = MemoryEvent::StrengthDecayed {
            fact_id: "f".into(),
            old_strength: 1.0,
            new_strength: 0.9,
            decay_factor: 0.95,
        };
        assert!(!pulse.is_skeleton());
    }

    #[test]
    fn test_envelope_creation() {
        let event = MemoryEvent::FactDeleted {
            fact_id: "fact-002".into(),
            reason: "user request".into(),
        };
        let envelope = MemoryEventEnvelope::new(
            "fact-002".into(),
            1,
            event,
            EventActor::User,
            None,
        );
        assert_eq!(envelope.fact_id, "fact-002");
        assert_eq!(envelope.seq, 1);
        assert!(envelope.timestamp > 0);
        assert_eq!(envelope.id, 0); // Not yet assigned
    }

    #[test]
    fn test_event_actor_display_and_parse() {
        assert_eq!(EventActor::Agent.to_string(), "agent");
        assert_eq!("system".parse::<EventActor>().unwrap(), EventActor::System);
        assert!("unknown".parse::<EventActor>().is_err());
    }

    #[test]
    fn test_all_event_variants_have_fact_id() {
        let events: Vec<MemoryEvent> = vec![
            MemoryEvent::FactCreated {
                fact_id: "a".into(), content: "c".into(), fact_type: FactType::Other,
                tier: MemoryTier::ShortTerm, scope: MemoryScope::Global,
                path: "p".into(), namespace: "n".into(), workspace: "w".into(),
                confidence: 1.0, source: FactSource::Manual, source_memory_ids: vec![],
            },
            MemoryEvent::FactContentUpdated {
                fact_id: "b".into(), old_content: "o".into(),
                new_content: "n".into(), reason: "r".into(),
            },
            MemoryEvent::FactMetadataUpdated {
                fact_id: "c".into(), field: "tier".into(),
                old_value: "ShortTerm".into(), new_value: "LongTerm".into(),
            },
            MemoryEvent::TierTransitioned {
                fact_id: "d".into(), from_tier: MemoryTier::ShortTerm,
                to_tier: MemoryTier::LongTerm, trigger: TierTransitionTrigger::Consolidation,
            },
            MemoryEvent::FactAccessed {
                fact_id: "e".into(), query: None, relevance_score: None,
                used_in_response: false, new_access_count: 0,
            },
            MemoryEvent::StrengthDecayed {
                fact_id: "f".into(), old_strength: 1.0, new_strength: 0.5, decay_factor: 0.5,
            },
            MemoryEvent::FactInvalidated {
                fact_id: "g".into(), reason: "r".into(),
                actor: EventActor::Decay, strength_at_invalidation: Some(0.05),
            },
            MemoryEvent::FactRestored { fact_id: "h".into(), new_strength: 0.8 },
            MemoryEvent::FactDeleted { fact_id: "i".into(), reason: "r".into() },
            MemoryEvent::FactConsolidated {
                fact_id: "j".into(), source_fact_ids: vec![], consolidated_content: "c".into(),
            },
            MemoryEvent::FactMigrated {
                fact_id: "k".into(), snapshot: serde_json::json!({}),
            },
        ];
        // Every variant returns its fact_id
        let expected = ["a","b","c","d","e","f","g","h","i","j","k"];
        for (evt, exp) in events.iter().zip(expected.iter()) {
            assert_eq!(evt.fact_id(), *exp);
        }
    }
}
```

**Step 2: Create stub submodule files**

Create empty placeholder files so `mod` declarations compile:

```rust
// core/src/memory/events/commands.rs
//! Command structs for memory mutations.
// To be implemented in Task 5.

// core/src/memory/events/handler.rs
//! MemoryCommandHandler — write-side facade.
// To be implemented in Task 5.

// core/src/memory/events/projector.rs
//! EventProjector — materializes events into LanceDB snapshots.
// To be implemented in Task 4.

// core/src/memory/events/traveler.rs
//! MemoryTimeTraveler — time-travel query service.
// To be implemented in Task 8.

// core/src/memory/events/migration.rs
//! EventSourcingMigration — one-time migration for existing facts.
// To be implemented in Task 7.
```

**Step 3: Register the module**

In `core/src/memory/mod.rs`, add after line 20 (`pub mod audit;`):

```rust
pub mod events;
```

And add re-exports after line 69 (after the `AuditLogger` re-export block):

```rust
pub use events::{
    EventActor, MemoryEvent, MemoryEventEnvelope, TierTransitionTrigger,
};
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test memory::events::tests -- --nocapture`
Expected: All 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/memory/events/ core/src/memory/mod.rs
git commit -m "memory: add MemoryEvent enum and EventEnvelope types"
```

---

### Task 2: Add MemoryEventStore trait to store module

**Files:**
- Modify: `core/src/memory/store/mod.rs:449-495` (add trait after AuditStore)
- Test: compile-check only (trait has no impl yet)

**Step 1: Add the trait definition**

In `core/src/memory/store/mod.rs`, add after the `CompressionStore` trait (after line 495):

```rust
// ---------------------------------------------------------------------------
// MemoryEventStore -- Event sourcing persistence trait
// ---------------------------------------------------------------------------

/// Append-only event log for memory domain events.
///
/// This is the **source of truth** for all fact mutations. Events are
/// stored in SQLite and projected to LanceDB for search.
#[async_trait]
pub trait MemoryEventStore: Send + Sync {
    // -- Write ---------------------------------------------------------------

    /// Append a single event. Returns the assigned global ID.
    async fn append_event(
        &self,
        envelope: &crate::memory::events::MemoryEventEnvelope,
    ) -> Result<i64, AlephError>;

    /// Batch-append events (for Pulse flush and migration).
    async fn append_events(
        &self,
        envelopes: &[crate::memory::events::MemoryEventEnvelope],
    ) -> Result<(), AlephError>;

    // -- Read by fact --------------------------------------------------------

    /// Load all events for a fact, ordered by seq.
    async fn get_events_for_fact(
        &self,
        fact_id: &str,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    /// Load events for a fact since a given sequence number.
    async fn get_events_since_seq(
        &self,
        fact_id: &str,
        since_seq: u64,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    // -- Time travel ---------------------------------------------------------

    /// Load all events for a fact up to a given timestamp.
    async fn get_events_until(
        &self,
        fact_id: &str,
        until_timestamp: i64,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    /// Load all events within a time range (across all facts).
    async fn get_events_in_range(
        &self,
        from_timestamp: i64,
        to_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<crate::memory::events::MemoryEventEnvelope>, AlephError>;

    // -- Statistics ----------------------------------------------------------

    /// Get the latest sequence number for a fact (0 if no events).
    async fn get_latest_seq(&self, fact_id: &str) -> Result<u64, AlephError>;

    /// Count total events, optionally filtered by event type tag.
    async fn count_events(
        &self,
        event_type_filter: Option<&str>,
    ) -> Result<usize, AlephError>;
}
```

**Step 2: Run compile check**

Run: `cd core && cargo check`
Expected: Compiles successfully (trait defined, no impl required yet)

**Step 3: Commit**

```bash
git add core/src/memory/store/mod.rs
git commit -m "memory: add MemoryEventStore trait definition"
```

---

### Task 3: Add memory_events SQLite table + implement MemoryEventStore

**Files:**
- Modify: `core/src/resilience/database/state_database.rs:267-268` (add table to schema)
- Create: `core/src/resilience/database/memory_events.rs` (CRUD impl)
- Modify: `core/src/resilience/database/mod.rs:16` (add module)
- Test: inline `#[cfg(test)]` in `memory_events.rs`

**Step 1: Add the memory_events table to the schema**

In `core/src/resilience/database/state_database.rs`, add after line 267 (after the `idx_audit_action` index), before line 269 (the sqlite-vec comment):

```sql
            -- ================================================================
            -- Memory Event Sourcing (append-only event log)
            -- ================================================================

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

            CREATE INDEX IF NOT EXISTS idx_me_fact_id
                ON memory_events(fact_id);
            CREATE INDEX IF NOT EXISTS idx_me_timestamp
                ON memory_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_me_event_type
                ON memory_events(event_type);
            CREATE INDEX IF NOT EXISTS idx_me_correlation
                ON memory_events(correlation_id);
```

**Step 2: Write the failing test first**

Create `core/src/resilience/database/memory_events.rs`:

```rust
//! CRUD operations for memory_events table
//!
//! Implements MemoryEventStore for StateDatabase.
//! Follows the same pattern as events.rs (agent_events).

use crate::error::AlephError;
use crate::memory::events::{EventActor, MemoryEvent, MemoryEventEnvelope};
use super::StateDatabase;
use rusqlite::params;

impl StateDatabase {
    // =========================================================================
    // Memory Events CRUD (MemoryEventStore implementation)
    // =========================================================================

    /// Append a single memory event. Returns the assigned global ID.
    pub async fn append_memory_event(
        &self,
        envelope: &MemoryEventEnvelope,
    ) -> Result<i64, AlephError> {
        let event_json = serde_json::to_string(&envelope.event)
            .map_err(|e| AlephError::other(format!("Failed to serialize event: {e}")))?;
        let tier = if envelope.event.is_skeleton() { "skeleton" } else { "pulse" };

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO memory_events (fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                envelope.fact_id,
                envelope.seq,
                envelope.event.event_type_tag(),
                event_json,
                envelope.actor.to_string(),
                tier,
                envelope.timestamp,
                envelope.correlation_id,
            ],
        )
        .map_err(|e| AlephError::other(format!("Failed to append memory event: {e}")))?;

        Ok(conn.last_insert_rowid())
    }

    /// Batch-append memory events.
    pub async fn append_memory_events(
        &self,
        envelopes: &[MemoryEventEnvelope],
    ) -> Result<(), AlephError> {
        if envelopes.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                INSERT INTO memory_events (fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        for envelope in envelopes {
            let event_json = serde_json::to_string(&envelope.event)
                .map_err(|e| AlephError::other(format!("Failed to serialize event: {e}")))?;
            let tier = if envelope.event.is_skeleton() { "skeleton" } else { "pulse" };

            stmt.execute(params![
                envelope.fact_id,
                envelope.seq,
                envelope.event.event_type_tag(),
                event_json,
                envelope.actor.to_string(),
                tier,
                envelope.timestamp,
                envelope.correlation_id,
            ])
            .map_err(|e| AlephError::other(format!("Failed to append memory event: {e}")))?;
        }

        Ok(())
    }

    /// Get all events for a fact, ordered by seq.
    pub async fn get_memory_events_for_fact(
        &self,
        fact_id: &str,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id
                FROM memory_events
                WHERE fact_id = ?1
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map(params![fact_id], |row| {
                Ok(MemoryEventRow {
                    id: row.get(0)?,
                    fact_id: row.get(1)?,
                    seq: row.get::<_, i64>(2)? as u64,
                    _event_type: row.get::<_, String>(3)?,
                    event_json: row.get(4)?,
                    actor: row.get(5)?,
                    _tier: row.get::<_, String>(6)?,
                    timestamp: row.get(7)?,
                    correlation_id: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query events: {e}")))?;

        let mut envelopes = Vec::new();
        for row in rows {
            let row = row.map_err(|e| AlephError::other(format!("Row error: {e}")))?;
            envelopes.push(row.into_envelope()?);
        }
        Ok(envelopes)
    }

    /// Get events for a fact since a given sequence number.
    pub async fn get_memory_events_since_seq(
        &self,
        fact_id: &str,
        since_seq: u64,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id
                FROM memory_events
                WHERE fact_id = ?1 AND seq > ?2
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map(params![fact_id, since_seq as i64], |row| {
                Ok(MemoryEventRow {
                    id: row.get(0)?,
                    fact_id: row.get(1)?,
                    seq: row.get::<_, i64>(2)? as u64,
                    _event_type: row.get::<_, String>(3)?,
                    event_json: row.get(4)?,
                    actor: row.get(5)?,
                    _tier: row.get::<_, String>(6)?,
                    timestamp: row.get(7)?,
                    correlation_id: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query events: {e}")))?;

        let mut envelopes = Vec::new();
        for row in rows {
            let row = row.map_err(|e| AlephError::other(format!("Row error: {e}")))?;
            envelopes.push(row.into_envelope()?);
        }
        Ok(envelopes)
    }

    /// Get events for a fact up to a given timestamp (for time travel).
    pub async fn get_memory_events_until(
        &self,
        fact_id: &str,
        until_timestamp: i64,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id
                FROM memory_events
                WHERE fact_id = ?1 AND timestamp <= ?2
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map(params![fact_id, until_timestamp], |row| {
                Ok(MemoryEventRow {
                    id: row.get(0)?,
                    fact_id: row.get(1)?,
                    seq: row.get::<_, i64>(2)? as u64,
                    _event_type: row.get::<_, String>(3)?,
                    event_json: row.get(4)?,
                    actor: row.get(5)?,
                    _tier: row.get::<_, String>(6)?,
                    timestamp: row.get(7)?,
                    correlation_id: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query events: {e}")))?;

        let mut envelopes = Vec::new();
        for row in rows {
            let row = row.map_err(|e| AlephError::other(format!("Row error: {e}")))?;
            envelopes.push(row.into_envelope()?);
        }
        Ok(envelopes)
    }

    /// Get events across all facts within a time range.
    pub async fn get_memory_events_in_range(
        &self,
        from_timestamp: i64,
        to_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, fact_id, seq, event_type, event_json, actor, tier, timestamp, correlation_id
                FROM memory_events
                WHERE timestamp >= ?1 AND timestamp <= ?2
                ORDER BY id ASC
                LIMIT ?3
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map(params![from_timestamp, to_timestamp, limit as i64], |row| {
                Ok(MemoryEventRow {
                    id: row.get(0)?,
                    fact_id: row.get(1)?,
                    seq: row.get::<_, i64>(2)? as u64,
                    _event_type: row.get::<_, String>(3)?,
                    event_json: row.get(4)?,
                    actor: row.get(5)?,
                    _tier: row.get::<_, String>(6)?,
                    timestamp: row.get(7)?,
                    correlation_id: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query events: {e}")))?;

        let mut envelopes = Vec::new();
        for row in rows {
            let row = row.map_err(|e| AlephError::other(format!("Row error: {e}")))?;
            envelopes.push(row.into_envelope()?);
        }
        Ok(envelopes)
    }

    /// Get the latest sequence number for a fact.
    pub async fn get_memory_event_latest_seq(&self, fact_id: &str) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result: Option<i64> = conn
            .query_row(
                "SELECT MAX(seq) FROM memory_events WHERE fact_id = ?1",
                params![fact_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::other(format!("Failed to get latest seq: {e}")))?;

        Ok(result.unwrap_or(0) as u64)
    }

    /// Count total memory events, optionally filtered by event type.
    pub async fn count_memory_events(
        &self,
        event_type_filter: Option<&str>,
    ) -> Result<usize, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = match event_type_filter {
            Some(et) => conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_events WHERE event_type = ?1",
                    params![et],
                    |row| row.get(0),
                )
                .map_err(|e| AlephError::other(format!("Failed to count events: {e}")))?,
            None => conn
                .query_row("SELECT COUNT(*) FROM memory_events", [], |row| row.get(0))
                .map_err(|e| AlephError::other(format!("Failed to count events: {e}")))?,
        };
        Ok(count as usize)
    }
}

// ---------------------------------------------------------------------------
// Internal helper for row mapping
// ---------------------------------------------------------------------------

struct MemoryEventRow {
    id: i64,
    fact_id: String,
    seq: u64,
    _event_type: String,
    event_json: String,
    actor: String,
    _tier: String,
    timestamp: i64,
    correlation_id: Option<String>,
}

impl MemoryEventRow {
    fn into_envelope(self) -> Result<MemoryEventEnvelope, AlephError> {
        let event: MemoryEvent = serde_json::from_str(&self.event_json)
            .map_err(|e| AlephError::other(format!("Failed to deserialize event: {e}")))?;
        let actor: EventActor = self.actor.parse()
            .map_err(|e: String| AlephError::other(e))?;
        Ok(MemoryEventEnvelope {
            id: self.id,
            fact_id: self.fact_id,
            seq: self.seq,
            event,
            actor,
            timestamp: self.timestamp,
            correlation_id: self.correlation_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, FactType, MemoryScope, MemoryTier};
    use crate::resilience::database::StateDatabase;

    fn make_test_db() -> StateDatabase {
        StateDatabase::in_memory().unwrap()
    }

    fn make_created_event(fact_id: &str) -> MemoryEvent {
        MemoryEvent::FactCreated {
            fact_id: fact_id.into(),
            content: "User prefers Rust".into(),
            fact_type: FactType::Preference,
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            path: "aleph://user/preferences/language".into(),
            namespace: "owner".into(),
            workspace: "default".into(),
            confidence: 0.85,
            source: FactSource::Extracted,
            source_memory_ids: vec!["mem-001".into()],
        }
    }

    #[tokio::test]
    async fn test_append_and_retrieve_event() {
        let db = make_test_db();
        let event = make_created_event("fact-001");
        let envelope = MemoryEventEnvelope::new(
            "fact-001".into(), 1, event, EventActor::Agent, None,
        );

        let id = db.append_memory_event(&envelope).await.unwrap();
        assert!(id > 0);

        let events = db.get_memory_events_for_fact("fact-001").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].fact_id, "fact-001");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].id, id);
    }

    #[tokio::test]
    async fn test_batch_append() {
        let db = make_test_db();
        let envelopes: Vec<_> = (1..=5).map(|i| {
            MemoryEventEnvelope::new(
                "fact-002".into(),
                i,
                MemoryEvent::FactAccessed {
                    fact_id: "fact-002".into(),
                    query: Some(format!("query-{i}")),
                    relevance_score: Some(0.9),
                    used_in_response: true,
                    new_access_count: i as u32,
                },
                EventActor::Agent,
                None,
            )
        }).collect();

        db.append_memory_events(&envelopes).await.unwrap();

        let events = db.get_memory_events_for_fact("fact-002").await.unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[4].seq, 5);
    }

    #[tokio::test]
    async fn test_get_events_since_seq() {
        let db = make_test_db();
        for i in 1..=3 {
            let envelope = MemoryEventEnvelope::new(
                "fact-003".into(), i,
                make_created_event("fact-003"),
                EventActor::Agent, None,
            );
            db.append_memory_event(&envelope).await.unwrap();
        }

        let events = db.get_memory_events_since_seq("fact-003", 1).await.unwrap();
        assert_eq!(events.len(), 2); // seq 2 and 3
        assert_eq!(events[0].seq, 2);
    }

    #[tokio::test]
    async fn test_get_events_until_timestamp() {
        let db = make_test_db();
        // Insert events with specific timestamps
        let mut e1 = MemoryEventEnvelope::new(
            "fact-004".into(), 1, make_created_event("fact-004"),
            EventActor::Agent, None,
        );
        e1.timestamp = 1000;
        db.append_memory_event(&e1).await.unwrap();

        let mut e2 = MemoryEventEnvelope::new(
            "fact-004".into(), 2,
            MemoryEvent::FactContentUpdated {
                fact_id: "fact-004".into(),
                old_content: "old".into(),
                new_content: "new".into(),
                reason: "correction".into(),
            },
            EventActor::User, None,
        );
        e2.timestamp = 2000;
        db.append_memory_event(&e2).await.unwrap();

        // Time travel to before the update
        let events = db.get_memory_events_until("fact-004", 1500).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 1);

        // Time travel to after the update
        let events = db.get_memory_events_until("fact-004", 2500).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn test_get_events_in_range() {
        let db = make_test_db();
        for (i, ts) in [1000i64, 2000, 3000].iter().enumerate() {
            let mut envelope = MemoryEventEnvelope::new(
                format!("fact-range-{i}"), 1,
                make_created_event(&format!("fact-range-{i}")),
                EventActor::Agent, None,
            );
            envelope.timestamp = *ts;
            db.append_memory_event(&envelope).await.unwrap();
        }

        let events = db.get_memory_events_in_range(1500, 2500, 100).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].fact_id, "fact-range-1");
    }

    #[tokio::test]
    async fn test_latest_seq() {
        let db = make_test_db();
        assert_eq!(db.get_memory_event_latest_seq("nonexistent").await.unwrap(), 0);

        for i in 1..=3 {
            let envelope = MemoryEventEnvelope::new(
                "fact-seq".into(), i,
                make_created_event("fact-seq"),
                EventActor::Agent, None,
            );
            db.append_memory_event(&envelope).await.unwrap();
        }
        assert_eq!(db.get_memory_event_latest_seq("fact-seq").await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_count_events() {
        let db = make_test_db();
        let e1 = MemoryEventEnvelope::new(
            "f1".into(), 1, make_created_event("f1"), EventActor::Agent, None,
        );
        let e2 = MemoryEventEnvelope::new(
            "f2".into(), 1,
            MemoryEvent::FactAccessed {
                fact_id: "f2".into(), query: None, relevance_score: None,
                used_in_response: false, new_access_count: 1,
            },
            EventActor::Agent, None,
        );
        db.append_memory_event(&e1).await.unwrap();
        db.append_memory_event(&e2).await.unwrap();

        assert_eq!(db.count_memory_events(None).await.unwrap(), 2);
        assert_eq!(db.count_memory_events(Some("FactCreated")).await.unwrap(), 1);
        assert_eq!(db.count_memory_events(Some("FactAccessed")).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_unique_constraint_on_fact_seq() {
        let db = make_test_db();
        let e1 = MemoryEventEnvelope::new(
            "dup".into(), 1, make_created_event("dup"), EventActor::Agent, None,
        );
        db.append_memory_event(&e1).await.unwrap();

        // Duplicate (fact_id, seq) should fail
        let e2 = MemoryEventEnvelope::new(
            "dup".into(), 1, make_created_event("dup"), EventActor::Agent, None,
        );
        assert!(db.append_memory_event(&e2).await.is_err());
    }
}
```

**Step 3: Register the module**

In `core/src/resilience/database/mod.rs`, add after line 16 (`mod traces;`):

```rust
mod memory_events;
```

**Step 4: Run tests**

Run: `cd core && cargo test resilience::database::memory_events::tests -- --nocapture`
Expected: All 8 tests PASS

**Step 5: Commit**

```bash
git add core/src/resilience/database/memory_events.rs core/src/resilience/database/mod.rs core/src/resilience/database/state_database.rs
git commit -m "memory: implement MemoryEventStore in SQLite"
```

---

## Phase 2: Core — Projector + CommandHandler

### Task 4: Implement EventProjector (event → MemoryFact fold)

**Files:**
- Modify: `core/src/memory/events/projector.rs` (replace stub)
- Test: inline `#[cfg(test)]`

**Step 1: Implement the projector**

Replace `core/src/memory/events/projector.rs` with the `EventProjector` that:
- `fold_events_to_fact()` — pure function that replays events into a `MemoryFact`
- `project()` — applies a single event to the LanceDB read model
- `rebuild_fact()` — loads events and folds to reconstruct current state
- `rebuild_fact_at()` — loads events up to timestamp and folds

Key implementation notes:
- `FactCreated` initializes all MemoryFact fields from event data
- `FactContentUpdated` updates `content` and `content_hash`
- `FactMetadataUpdated` updates the specific field
- `TierTransitioned` updates `tier`
- `FactAccessed` increments `access_count` and `last_accessed_at`
- `StrengthDecayed` updates `strength`
- `FactInvalidated` sets `is_valid = false`, `invalidation_reason`
- `FactRestored` sets `is_valid = true`, resets strength
- `FactDeleted` returns `None` (fact is gone)
- `FactConsolidated` updates `content` with consolidated text
- `FactMigrated` deserializes the full snapshot

The `project()` method calls through to the existing `MemoryStore` trait methods (insert_fact, update_fact, invalidate_fact, delete_fact) on the LanceDB backend.

**Step 2: Write tests for `fold_events_to_fact`**

Test the pure fold function extensively:
- Empty events → None
- Single FactCreated → valid MemoryFact
- FactCreated + FactContentUpdated → updated content
- FactCreated + FactInvalidated → is_valid = false
- FactCreated + FactDeleted → None
- FactCreated + multiple FactAccessed → correct access_count
- FactMigrated → fact from snapshot

**Step 3: Run tests**

Run: `cd core && cargo test memory::events::projector::tests -- --nocapture`
Expected: All fold tests PASS

**Step 4: Commit**

```bash
git add core/src/memory/events/projector.rs
git commit -m "memory: implement EventProjector with fold and projection"
```

---

### Task 5: Implement Command structs + MemoryCommandHandler

**Files:**
- Modify: `core/src/memory/events/commands.rs` (replace stub)
- Modify: `core/src/memory/events/handler.rs` (replace stub)
- Test: inline `#[cfg(test)]` in handler.rs

**Step 1: Define command structs**

Replace `core/src/memory/events/commands.rs`:

```rust
//! Command structs for memory mutations.
//!
//! Each command maps to one or more MemoryEvents.

use crate::memory::context::{FactSource, FactType, MemoryScope, MemoryTier};
use crate::memory::events::EventActor;

pub struct CreateFactCommand {
    pub content: String,
    pub fact_type: FactType,
    pub tier: MemoryTier,
    pub scope: MemoryScope,
    pub path: String,
    pub namespace: String,
    pub workspace: String,
    pub confidence: f32,
    pub source: FactSource,
    pub source_memory_ids: Vec<String>,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

pub struct UpdateContentCommand {
    pub fact_id: String,
    pub new_content: String,
    pub reason: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}

pub struct InvalidateFactCommand {
    pub fact_id: String,
    pub reason: String,
    pub actor: EventActor,
    pub strength_at_invalidation: Option<f32>,
    pub correlation_id: Option<String>,
}

pub struct RestoreFactCommand {
    pub fact_id: String,
    pub new_strength: f32,
    pub correlation_id: Option<String>,
}

pub struct RecordAccessCommand {
    pub fact_id: String,
    pub query: Option<String>,
    pub relevance_score: Option<f32>,
    pub used_in_response: bool,
    pub correlation_id: Option<String>,
}

pub struct ApplyDecayCommand {
    pub fact_ids_with_strength: Vec<(String, f32, f32)>, // (fact_id, old_strength, new_strength)
    pub decay_factor: f32,
    pub correlation_id: Option<String>,
}

pub struct ConsolidateCommand {
    pub source_fact_ids: Vec<String>,
    pub consolidated_content: String,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}
```

**Step 2: Implement MemoryCommandHandler**

Replace `core/src/memory/events/handler.rs`. The handler:
1. Gets the latest seq for the fact from MemoryEventStore
2. Builds a `MemoryEvent` from the command
3. Wraps in `MemoryEventEnvelope` with seq+1
4. Appends to EventStore (SQLite)
5. Projects to LanceDB via EventProjector

For `create_fact`: generates UUID, creates FactCreated event, appends, projects.
For `update_content`: reads current fact to get old_content, creates FactContentUpdated, appends, projects.
For `invalidate_fact`: creates FactInvalidated, appends, projects.
For `record_access`: reads current access_count, creates FactAccessed, appends, projects.
For `apply_decay`: for each fact, creates StrengthDecayed, batch appends, batch projects.

**Step 3: Write tests for MemoryCommandHandler**

Test with a real in-memory StateDatabase and mock/simple MemoryStore:
- `create_fact` → event appended + fact in store
- `update_content` → content changed + event trail
- `invalidate_fact` → fact invalidated + event trail
- `record_access` → access_count incremented + pulse event

**Step 4: Run tests**

Run: `cd core && cargo test memory::events::handler::tests -- --nocapture`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/memory/events/commands.rs core/src/memory/events/handler.rs
git commit -m "memory: implement MemoryCommandHandler with event-first write path"
```

---

### Task 6: Wire MemoryCommandHandler into compression service

**Files:**
- Modify: `core/src/memory/compression/service.rs:179` (replace direct insert_fact)
- Test: existing compression tests should still pass

**Step 1: Add MemoryCommandHandler to CompressionService**

In the CompressionService struct, add an optional `command_handler: Option<Arc<MemoryCommandHandler>>` field. When present, use it instead of direct `database.insert_fact()`.

At line 179 of `core/src/memory/compression/service.rs`, replace:
```rust
match self.database.insert_fact(&fact).await {
```

With logic that:
1. If `command_handler` is Some, call `command_handler.create_fact(CreateFactCommand { ... })`.
2. Else, fall back to `self.database.insert_fact(&fact)` (for backward compatibility during rollout).

**Step 2: Update CompressionService constructor**

Add a `with_command_handler()` builder method or update `new()` to accept an optional handler.

**Step 3: Run existing compression tests**

Run: `cd core && cargo test compression -- --nocapture`
Expected: All existing tests PASS (fallback path used)

**Step 4: Commit**

```bash
git add core/src/memory/compression/service.rs
git commit -m "memory: wire CommandHandler into compression service"
```

---

## Phase 3: Migration

### Task 7: Implement EventSourcingMigration

**Files:**
- Modify: `core/src/memory/events/migration.rs` (replace stub)
- Test: inline `#[cfg(test)]`

**Step 1: Implement the migration**

Replace `core/src/memory/events/migration.rs`:

The migration:
1. Calls `memory_store.get_all_facts(true)` to get all facts (including invalid)
2. For each fact, checks if events already exist (idempotent)
3. If no events, creates a `FactMigrated` event with the full fact serialized as JSON
4. Uses the fact's `created_at` as the event timestamp (preserve history)
5. Sets `seq = 1` and `actor = EventActor::Migration`
6. Batch-appends in chunks of 100
7. Returns a `MigrationReport { migrated, skipped, total }`

**Step 2: Write tests**

Test with:
- Empty database → 0 migrated
- 3 facts → 3 FactMigrated events
- Run twice → second run skips all (idempotent)
- Invalid facts are also migrated

**Step 3: Run tests**

Run: `cd core && cargo test memory::events::migration::tests -- --nocapture`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/memory/events/migration.rs
git commit -m "memory: implement EventSourcingMigration for existing facts"
```

---

## Phase 4: Time Travel

### Task 8: Implement MemoryTimeTraveler

**Files:**
- Modify: `core/src/memory/events/traveler.rs` (replace stub)
- Test: inline `#[cfg(test)]`

**Step 1: Implement the time traveler**

Replace `core/src/memory/events/traveler.rs`:

```rust
pub struct MemoryTimeTraveler { ... }

impl MemoryTimeTraveler {
    pub async fn fact_at(&self, fact_id: &str, at: i64) -> Result<Option<MemoryFact>>;
    pub async fn fact_timeline(&self, fact_id: &str) -> Result<Vec<MemoryEventEnvelope>>;
    pub async fn events_between(&self, from: i64, to: i64, limit: usize) -> Result<Vec<MemoryEventEnvelope>>;
    pub async fn explain_fact(&self, fact_id: &str) -> Result<FactExplanation>;
}
```

The `explain_fact` method converts the event stream into a `FactExplanation` (the same struct used by `AuditLogger::explain_fact` in `core/src/memory/audit.rs:312`). This provides backward compatibility.

Implementation:
- `fact_at` → calls `event_store.get_events_until(fact_id, at)` → `EventProjector::fold_events_to_fact()`
- `fact_timeline` → calls `event_store.get_events_for_fact(fact_id)`
- `events_between` → calls `event_store.get_events_in_range(from, to, limit)`
- `explain_fact` → loads events, maps each to an `ExplainedEvent`, builds `FactExplanation`

**Step 2: Write tests**

Test:
- `fact_at` with events at t=1000, t=2000, t=3000 → query at t=1500 returns state after first event only
- `explain_fact` produces correct FactExplanation with events list
- Empty fact → error

**Step 3: Run tests**

Run: `cd core && cargo test memory::events::traveler::tests -- --nocapture`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/memory/events/traveler.rs
git commit -m "memory: implement MemoryTimeTraveler for historical queries"
```

---

## Phase 5: Integration — Wire up remaining services

### Task 9: Wire CommandHandler into DreamDaemon decay

**Files:**
- Modify: `core/src/memory/dreaming.rs:415-418` (replace direct apply_fact_decay)

**Step 1: Add optional CommandHandler to DreamDaemon**

Add `command_handler: Option<Arc<MemoryCommandHandler>>` to the DreamDaemon struct.

At line 415 of `core/src/memory/dreaming.rs`, replace the direct `apply_fact_decay` call with:
- If command_handler present: iterate facts, calculate decay per-fact, emit StrengthDecayed events via handler
- Else: fallback to existing `self.database.apply_fact_decay(...)` call

This is the most complex integration because the current `apply_fact_decay` is a bulk operation in LanceDB. The event-sourced version needs per-fact events. Consider:
1. First get all valid facts
2. Calculate new strength for each
3. Emit StrengthDecayed events for those that changed
4. Emit FactInvalidated for those below threshold
5. Project all events

**Step 2: Run existing dream tests**

Run: `cd core && cargo test dreaming -- --nocapture`
Expected: All existing tests PASS (fallback path)

**Step 3: Commit**

```bash
git add core/src/memory/dreaming.rs
git commit -m "memory: wire CommandHandler into DreamDaemon decay path"
```

---

### Task 10: Deprecate AuditStore + bridge AuditLogger to events

**Files:**
- Modify: `core/src/memory/store/mod.rs:449-472` (add #[deprecated] to AuditStore)
- Modify: `core/src/memory/audit.rs` (AuditLogger can optionally read from EventStore)
- Modify: `core/src/memory/mod.rs:67-69` (update re-exports)

**Step 1: Add deprecation notice to AuditStore trait**

In `core/src/memory/store/mod.rs`, add `#[deprecated]` attribute to the AuditStore trait:

```rust
#[deprecated(
    since = "0.2.0",
    note = "Use MemoryEventStore for event-sourced audit. AuditStore will be removed in a future release."
)]
#[async_trait]
pub trait AuditStore: Send + Sync { ... }
```

**Step 2: Add EventStore-backed path to AuditLogger**

Add an alternative constructor `AuditLogger::from_event_store(...)` that reads from the event stream instead of AuditStore.

Update `explain_fact` to delegate to `MemoryTimeTraveler::explain_fact` when available.

**Step 3: Suppress deprecation warnings in existing code**

Add `#[allow(deprecated)]` at call sites that still use AuditStore directly.

**Step 4: Run full test suite**

Run: `cd core && cargo test -- --nocapture`
Expected: All tests PASS (with deprecation warnings)

**Step 5: Commit**

```bash
git add core/src/memory/store/mod.rs core/src/memory/audit.rs core/src/memory/mod.rs
git commit -m "memory: deprecate AuditStore, bridge AuditLogger to event stream"
```

---

### Task 11: Add re-exports and update module documentation

**Files:**
- Modify: `core/src/memory/mod.rs:63-90` (add events re-exports)
- Modify: `core/src/memory/events/mod.rs` (update doc comments)

**Step 1: Add public re-exports**

In `core/src/memory/mod.rs`, add after the existing events re-export:

```rust
pub use events::{
    commands::{
        ApplyDecayCommand, ConsolidateCommand, CreateFactCommand,
        InvalidateFactCommand, RecordAccessCommand, RestoreFactCommand,
        UpdateContentCommand,
    },
    handler::MemoryCommandHandler,
    migration::EventSourcingMigration,
    projector::EventProjector,
    traveler::MemoryTimeTraveler,
    EventActor, MemoryEvent, MemoryEventEnvelope, TierTransitionTrigger,
};
```

**Step 2: Run full compile check**

Run: `cd core && cargo check`
Expected: Compiles with only AuditStore deprecation warnings

**Step 3: Commit**

```bash
git add core/src/memory/mod.rs core/src/memory/events/mod.rs
git commit -m "memory: finalize event sourcing module exports and docs"
```

---

### Task 12: Integration test — full event sourcing round-trip

**Files:**
- Modify: `core/src/memory/integration_tests.rs` (add ES round-trip test)

**Step 1: Write an integration test**

Add a test that exercises the complete flow:
1. Create a `StateDatabase::in_memory()` and a mock/test MemoryStore
2. Create `MemoryCommandHandler` with real EventStore + Projector
3. `create_fact` → verify event in SQLite + fact in store
4. `update_content` → verify event trail + updated fact
5. `record_access` → verify pulse event + access count
6. `invalidate_fact` → verify event + fact invalidated
7. Use `MemoryTimeTraveler::fact_at()` to reconstruct state at each step
8. Verify `explain_fact()` returns correct timeline

**Step 2: Run the integration test**

Run: `cd core && cargo test memory::integration_tests::test_event_sourcing_round_trip -- --nocapture`
Expected: PASS

**Step 3: Run the full test suite**

Run: `cd core && cargo test`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/memory/integration_tests.rs
git commit -m "memory: add event sourcing integration test"
```

---

### Task 13: Run EventSourcingMigration on existing data (manual step)

**This task is a manual operation, not automated code.**

After deploying all previous tasks, run the migration:

```rust
// In server startup or a one-time CLI command:
let migration = EventSourcingMigration::new(memory_backend.clone(), event_store.clone());
let report = migration.run().await?;
tracing::info!(
    migrated = report.migrated,
    skipped = report.skipped,
    total = report.total,
    "Event sourcing migration complete"
);
```

Add a CLI subcommand or server startup hook that triggers this migration once. Consider adding a schema_info flag like `event_sourcing_migrated = true` to prevent re-running.

**Step 1: Add migration trigger**

Add to server initialization (or a CLI command) a conditional migration call.

**Step 2: Test locally**

Run the server with existing memory data, verify migration report shows correct counts.

**Step 3: Commit**

```bash
git add <relevant files>
git commit -m "memory: add event sourcing migration trigger to server startup"
```
