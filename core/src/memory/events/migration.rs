//! Memory Event Sourcing — Legacy Migration
//!
//! One-shot migration from the legacy CRUD-based LanceDB store to the
//! event-sourced model. For each existing [`crate::memory::context::MemoryFact`],
//! emits a [`super::MemoryEvent::FactMigrated`] event containing the full
//! fact serialized as a JSON snapshot, establishing the initial event history.
//!
//! The migration is idempotent: running it twice will skip already-migrated
//! facts (checked via `get_memory_event_latest_seq`).

use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::events::{EventActor, MemoryEvent, MemoryEventEnvelope};
use crate::resilience::database::StateDatabase;

/// Summary report returned after a migration run.
#[derive(Debug, Clone)]
pub struct MigrationReport {
    /// Number of facts successfully migrated (event emitted).
    pub migrated: usize,
    /// Number of facts skipped (already had events in the store).
    pub skipped: usize,
    /// Total number of facts presented for migration.
    pub total: usize,
}

/// Migrates existing `MemoryFact`s into the event-sourced store.
///
/// Accepts a `Vec<MemoryFact>` (or slice) so callers can provide facts from
/// any source (LanceDB, a file, tests, etc.) without requiring a `MemoryStore`.
pub struct EventSourcingMigration {
    db: Arc<StateDatabase>,
}

impl EventSourcingMigration {
    /// Create a new migration backed by the given event store.
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    /// Run migration only if no migration events exist.
    ///
    /// This is designed to be called at server startup. If migration events
    /// already exist, this is a no-op.
    ///
    /// Returns the migration report (or a report with all zeros if skipped).
    pub async fn run_if_needed(&self, facts: &[MemoryFact]) -> Result<MigrationReport, AlephError> {
        let has_events = self.db.has_migration_events().await?;
        if has_events {
            tracing::info!("Event sourcing migration already completed, skipping");
            return Ok(MigrationReport {
                migrated: 0,
                skipped: facts.len(),
                total: facts.len(),
            });
        }

        let report = self.migrate_facts(facts).await?;
        tracing::info!(
            migrated = report.migrated,
            skipped = report.skipped,
            total = report.total,
            "Event sourcing migration completed"
        );
        Ok(report)
    }

    /// Migrate a list of existing MemoryFacts into event-sourced form.
    ///
    /// For each fact:
    /// 1. Check if events already exist (`get_latest_seq > 0` --> skip)
    /// 2. Serialize the full fact as a JSON snapshot
    /// 3. Create a `FactMigrated` event with `seq=1`, `actor=Migration`,
    ///    `timestamp=fact.created_at`
    /// 4. Batch-append in chunks of 100
    ///
    /// This is idempotent -- running twice will skip already-migrated facts.
    pub async fn migrate_facts(&self, facts: &[MemoryFact]) -> Result<MigrationReport, AlephError> {
        let total = facts.len();
        let mut migrated = 0;
        let mut skipped = 0;
        let mut batch = Vec::new();

        for fact in facts {
            let existing_seq = self.db.get_memory_event_latest_seq(&fact.id).await?;
            if existing_seq > 0 {
                skipped += 1;
                continue;
            }

            let snapshot = serde_json::to_value(fact)
                .map_err(|e| AlephError::other(format!("Failed to serialize fact: {e}")))?;

            let event = MemoryEvent::FactMigrated {
                fact_id: fact.id.clone(),
                snapshot,
            };

            let mut envelope = MemoryEventEnvelope::new(
                fact.id.clone(),
                1,
                event,
                EventActor::Migration,
                None,
            );
            // Preserve the original creation timestamp rather than "now".
            envelope.timestamp = fact.created_at;

            batch.push(envelope);
            migrated += 1;

            // Flush in chunks of 100
            if batch.len() >= 100 {
                self.db.append_memory_events(&batch).await?;
                batch.clear();
            }
        }

        // Flush remaining
        if !batch.is_empty() {
            self.db.append_memory_events(&batch).await?;
        }

        Ok(MigrationReport {
            migrated,
            skipped,
            total,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::*;

    fn make_test_fact(id: &str, content: &str, is_valid: bool) -> MemoryFact {
        MemoryFact {
            id: id.into(),
            content: content.into(),
            fact_type: FactType::Preference,
            embedding: None,
            source_memory_ids: vec![],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid,
            invalidation_reason: if is_valid {
                None
            } else {
                Some("test".into())
            },
            decay_invalidated_at: None,
            specificity: FactSpecificity::Pattern,
            temporal_scope: TemporalScope::Contextual,
            namespace: "owner".into(),
            workspace: "default".into(),
            similarity_score: None,
            path: "/test".into(),
            layer: MemoryLayer::L2Detail,
            category: MemoryCategory::Entities,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path: "/".into(),
            embedding_model: String::new(),
            tier: MemoryTier::ShortTerm,
            scope: MemoryScope::Global,
            persona_id: None,
            strength: 1.0,
            access_count: 5,
            last_accessed_at: Some(900),
        }
    }

    #[tokio::test]
    async fn test_migrate_empty_list() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let migration = EventSourcingMigration::new(db);
        let report = migration.migrate_facts(&[]).await.unwrap();
        assert_eq!(report.total, 0);
        assert_eq!(report.migrated, 0);
        assert_eq!(report.skipped, 0);
    }

    #[tokio::test]
    async fn test_migrate_three_facts() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let migration = EventSourcingMigration::new(db.clone());
        let facts = vec![
            make_test_fact("f1", "Fact 1", true),
            make_test_fact("f2", "Fact 2", true),
            make_test_fact("f3", "Fact 3", true),
        ];
        let report = migration.migrate_facts(&facts).await.unwrap();
        assert_eq!(report.total, 3);
        assert_eq!(report.migrated, 3);
        assert_eq!(report.skipped, 0);

        // Verify events were stored
        let events = db.get_memory_events_for_fact("f1").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.event_type_tag(), "FactMigrated");
        assert_eq!(events[0].timestamp, 1000); // preserved original
    }

    #[tokio::test]
    async fn test_migrate_idempotent() {
        // Run twice -- second run skips all
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let migration = EventSourcingMigration::new(db.clone());
        let facts = vec![make_test_fact("f1", "Fact 1", true)];

        let r1 = migration.migrate_facts(&facts).await.unwrap();
        assert_eq!(r1.migrated, 1);

        let r2 = migration.migrate_facts(&facts).await.unwrap();
        assert_eq!(r2.migrated, 0);
        assert_eq!(r2.skipped, 1);

        // Still only 1 event
        let events = db.get_memory_events_for_fact("f1").await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn test_migrate_invalid_facts() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let migration = EventSourcingMigration::new(db.clone());
        let facts = vec![make_test_fact("inv1", "Invalid fact", false)];
        let report = migration.migrate_facts(&facts).await.unwrap();
        assert_eq!(report.migrated, 1);

        // Verify the snapshot preserves is_valid = false
        let events = db.get_memory_events_for_fact("inv1").await.unwrap();
        if let MemoryEvent::FactMigrated { snapshot, .. } = &events[0].event {
            assert_eq!(snapshot["is_valid"], false);
        } else {
            panic!("Expected FactMigrated");
        }
    }

    #[tokio::test]
    async fn test_run_if_needed_skips_when_already_migrated() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let migration = EventSourcingMigration::new(db.clone());
        let facts = vec![make_test_fact("f1", "Fact 1", true)];

        // First run should migrate
        let r1 = migration.run_if_needed(&facts).await.unwrap();
        assert_eq!(r1.migrated, 1);

        // Second run should skip entirely
        let r2 = migration.run_if_needed(&facts).await.unwrap();
        assert_eq!(r2.migrated, 0);
        assert_eq!(r2.skipped, 1);
    }

    #[tokio::test]
    async fn test_migrate_roundtrip_through_projector() {
        // Migrate a fact, then fold events to rebuild -- should match original
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let migration = EventSourcingMigration::new(db.clone());
        let original = make_test_fact("rt1", "Roundtrip test", true);
        migration
            .migrate_facts(&[original.clone()])
            .await
            .unwrap();

        let events = db.get_memory_events_for_fact("rt1").await.unwrap();
        let rebuilt = super::super::projector::EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .unwrap();
        assert_eq!(rebuilt.id, "rt1");
        assert_eq!(rebuilt.content, "Roundtrip test");
        assert_eq!(rebuilt.access_count, 5);
        assert_eq!(rebuilt.is_valid, true);
    }
}
