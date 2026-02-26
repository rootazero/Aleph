//! Memory Event Sourcing — Memory Time Traveler
//!
//! [`MemoryTimeTraveler`] replays events up to a given point in time
//! to reconstruct the state of a fact (or the entire memory) as it was
//! at that moment. Useful for debugging, auditing, and undo operations.
//!
//! The traveler delegates to [`EventProjector::fold_events_to_fact`] for
//! the actual state reconstruction, and provides higher-level query
//! methods for timeline and explanation use cases.

use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::audit::{ExplainedEvent, FactExplanation};
use crate::memory::events::{MemoryEvent, MemoryEventEnvelope};
use crate::resilience::database::StateDatabase;

use super::projector::EventProjector;

/// Time-travel service for historical memory state reconstruction.
///
/// Wraps the [`StateDatabase`] event store and provides:
/// - **`fact_at`** — reconstruct a fact's state at a specific timestamp
/// - **`fact_timeline`** — get the complete event stream for a fact
/// - **`events_between`** — get all memory changes within a time range
/// - **`explain_fact`** — human-readable lifecycle explanation (replaces
///   `AuditLogger::explain_fact` for event-sourced facts)
pub struct MemoryTimeTraveler {
    db: Arc<StateDatabase>,
}

impl MemoryTimeTraveler {
    /// Create a new time traveler backed by the given event store.
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    /// Reconstruct a fact's state at a specific timestamp.
    ///
    /// Loads all events for `fact_id` with `timestamp <= at` and folds
    /// them through the projector. Returns `Ok(None)` if the fact did
    /// not exist at the given time or was permanently deleted before then.
    pub async fn fact_at(
        &self,
        fact_id: &str,
        at: i64,
    ) -> Result<Option<crate::memory::context::MemoryFact>, AlephError> {
        let events = self.db.get_memory_events_until(fact_id, at).await?;
        EventProjector::fold_events_to_fact(&events)
    }

    /// Get the complete event timeline for a fact, ordered by sequence.
    pub async fn fact_timeline(
        &self,
        fact_id: &str,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        self.db.get_memory_events_for_fact(fact_id).await
    }

    /// Get all memory changes within a time range, across all facts.
    ///
    /// Results are ordered by global ID (insertion order) and capped at `limit`.
    pub async fn events_between(
        &self,
        from: i64,
        to: i64,
        limit: usize,
    ) -> Result<Vec<MemoryEventEnvelope>, AlephError> {
        self.db.get_memory_events_in_range(from, to, limit).await
    }

    /// Explain a fact's lifecycle by converting its event stream into
    /// a [`FactExplanation`].
    ///
    /// This replaces `AuditLogger::explain_fact` for event-sourced facts
    /// and produces the same `FactExplanation` struct for backward
    /// compatibility. The explanation is derived entirely from the event
    /// stream — no separate audit log or fact table is required.
    pub async fn explain_fact(&self, fact_id: &str) -> Result<FactExplanation, AlephError> {
        let events = self.db.get_memory_events_for_fact(fact_id).await?;
        if events.is_empty() {
            return Err(AlephError::other(format!(
                "No events found for fact {fact_id}"
            )));
        }

        // Convert events to ExplainedEvents and extract audit metadata
        let mut explained_events: Vec<ExplainedEvent> = Vec::with_capacity(events.len());
        let mut creation_source: Option<String> = None;
        let mut access_count: usize = 0;
        let mut invalidation_reason: Option<String> = None;

        for env in &events {
            explained_events.push(ExplainedEvent {
                timestamp: env.timestamp,
                action: env.event.event_type_tag().to_string(),
                description: describe_event(env),
                actor: env.actor.to_string(),
            });

            // Track audit metadata
            match &env.event {
                MemoryEvent::FactCreated { source, .. } => {
                    creation_source = Some(format!("{:?}", source));
                }
                MemoryEvent::FactMigrated { .. } => {
                    creation_source = Some("migration".to_string());
                }
                MemoryEvent::FactAccessed { .. } => {
                    access_count += 1;
                }
                MemoryEvent::FactInvalidated { reason, .. } => {
                    invalidation_reason = Some(reason.clone());
                }
                MemoryEvent::FactRestored { .. } => {
                    // Restoration clears invalidation
                    invalidation_reason = None;
                }
                _ => {}
            }
        }

        // Reconstruct current state via projector
        let current_fact = EventProjector::fold_events_to_fact(&events)?;

        let (content, is_valid) = match &current_fact {
            Some(f) => (Some(f.content.clone()), f.is_valid),
            None => (None, false), // deleted
        };

        Ok(FactExplanation {
            fact_id: fact_id.to_string(),
            content,
            is_valid,
            creation_source,
            access_count,
            invalidation_reason,
            events: explained_events,
        })
    }
}

/// Generate a human-readable description for an event envelope.
fn describe_event(env: &MemoryEventEnvelope) -> String {
    match &env.event {
        MemoryEvent::FactCreated {
            content, tier, ..
        } => {
            format!(
                "Fact created in {:?} tier: \"{}\"",
                tier,
                truncate(content, 50)
            )
        }
        MemoryEvent::FactContentUpdated { reason, .. } => {
            format!("Content updated: {}", reason)
        }
        MemoryEvent::FactMetadataUpdated {
            field,
            old_value,
            new_value,
            ..
        } => {
            format!(
                "Metadata '{}' changed from '{}' to '{}'",
                field, old_value, new_value
            )
        }
        MemoryEvent::TierTransitioned {
            from_tier,
            to_tier,
            trigger,
            ..
        } => {
            format!(
                "Tier changed from {:?} to {:?} (trigger: {})",
                from_tier, to_tier, trigger
            )
        }
        MemoryEvent::FactAccessed {
            query,
            used_in_response,
            new_access_count,
            ..
        } => {
            format!(
                "Accessed (count: {}, used: {}, query: {:?})",
                new_access_count, used_in_response, query
            )
        }
        MemoryEvent::StrengthDecayed {
            old_strength,
            new_strength,
            ..
        } => {
            format!(
                "Strength decayed from {:.2} to {:.2}",
                old_strength, new_strength
            )
        }
        MemoryEvent::FactInvalidated {
            reason, actor, ..
        } => {
            format!("Invalidated by {}: {}", actor, reason)
        }
        MemoryEvent::FactRestored { new_strength, .. } => {
            format!("Restored with strength {:.2}", new_strength)
        }
        MemoryEvent::FactDeleted { reason, .. } => {
            format!("Permanently deleted: {}", reason)
        }
        MemoryEvent::FactConsolidated {
            source_fact_ids, ..
        } => {
            format!("Consolidated from {} source facts", source_fact_ids.len())
        }
        MemoryEvent::FactMigrated { .. } => "Migrated from legacy CRUD store".to_string(),
    }
}

/// Truncate a string to `max` characters, appending "..." if truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{}...", truncated)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, FactType, MemoryScope, MemoryTier};
    use crate::memory::events::*;

    /// Helper: create an in-memory StateDatabase wrapped in Arc.
    fn make_db() -> Arc<StateDatabase> {
        Arc::new(StateDatabase::in_memory().unwrap())
    }

    /// Helper: build a FactCreated envelope with a given timestamp.
    fn make_created(fact_id: &str, seq: u64, ts: i64) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::FactCreated {
                fact_id: fact_id.into(),
                content: "User prefers Rust".into(),
                fact_type: FactType::Preference,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                path: "aleph://user/preferences/language".into(),
                namespace: "owner".into(),
                workspace: "default".into(),
                confidence: 0.9,
                source: FactSource::Extracted,
                source_memory_ids: vec![],
            },
            EventActor::Agent,
            None,
        );
        env.timestamp = ts;
        env
    }

    /// Helper: build a FactContentUpdated envelope.
    fn make_content_updated(
        fact_id: &str,
        seq: u64,
        ts: i64,
        new_content: &str,
    ) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::FactContentUpdated {
                fact_id: fact_id.into(),
                old_content: "User prefers Rust".into(),
                new_content: new_content.into(),
                reason: "correction".into(),
            },
            EventActor::User,
            None,
        );
        env.timestamp = ts;
        env
    }

    /// Helper: build a FactInvalidated envelope.
    fn make_invalidated(fact_id: &str, seq: u64, ts: i64) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::FactInvalidated {
                fact_id: fact_id.into(),
                reason: "outdated".into(),
                actor: EventActor::System,
                strength_at_invalidation: Some(0.1),
            },
            EventActor::System,
            None,
        );
        env.timestamp = ts;
        env
    }

    /// Helper: build a FactAccessed envelope.
    fn make_accessed(fact_id: &str, seq: u64, ts: i64, count: u32) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::FactAccessed {
                fact_id: fact_id.into(),
                query: Some("rust preference".into()),
                relevance_score: Some(0.95),
                used_in_response: true,
                new_access_count: count,
            },
            EventActor::Agent,
            None,
        );
        env.timestamp = ts;
        env
    }

    // --- fact_at (time travel) -----------------------------------------------

    #[tokio::test]
    async fn test_fact_at_before_creation() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-tt-1", 1, 1000))
            .await
            .unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let result = traveler.fact_at("fact-tt-1", 500).await.unwrap();
        assert!(result.is_none(), "Fact should not exist before creation");
    }

    #[tokio::test]
    async fn test_fact_at_after_creation_before_update() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-tt-2", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_content_updated(
            "fact-tt-2",
            2,
            2000,
            "User prefers Rust and Go",
        ))
        .await
        .unwrap();

        let traveler = MemoryTimeTraveler::new(db);

        // At t=1500 — only the creation event
        let fact = traveler
            .fact_at("fact-tt-2", 1500)
            .await
            .unwrap()
            .expect("Fact should exist at t=1500");
        assert_eq!(fact.content, "User prefers Rust");

        // At t=2500 — both events
        let fact = traveler
            .fact_at("fact-tt-2", 2500)
            .await
            .unwrap()
            .expect("Fact should exist at t=2500");
        assert_eq!(fact.content, "User prefers Rust and Go");
    }

    #[tokio::test]
    async fn test_fact_at_nonexistent_fact() {
        let db = make_db();
        let traveler = MemoryTimeTraveler::new(db);
        let result = traveler.fact_at("no-such-fact", 9999).await.unwrap();
        assert!(result.is_none());
    }

    // --- fact_timeline -------------------------------------------------------

    #[tokio::test]
    async fn test_fact_timeline_returns_all_events_in_order() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-tl-1", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_accessed("fact-tl-1", 2, 2000, 1))
            .await
            .unwrap();
        db.append_memory_event(&make_content_updated(
            "fact-tl-1",
            3,
            3000,
            "Updated content",
        ))
        .await
        .unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let timeline = traveler.fact_timeline("fact-tl-1").await.unwrap();

        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline[0].seq, 1);
        assert_eq!(timeline[1].seq, 2);
        assert_eq!(timeline[2].seq, 3);
        assert_eq!(timeline[0].event.event_type_tag(), "FactCreated");
        assert_eq!(timeline[1].event.event_type_tag(), "FactAccessed");
        assert_eq!(timeline[2].event.event_type_tag(), "FactContentUpdated");
    }

    #[tokio::test]
    async fn test_fact_timeline_empty_for_unknown_fact() {
        let db = make_db();
        let traveler = MemoryTimeTraveler::new(db);
        let timeline = traveler.fact_timeline("ghost").await.unwrap();
        assert!(timeline.is_empty());
    }

    // --- events_between ------------------------------------------------------

    #[tokio::test]
    async fn test_events_between_filters_by_time_range() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-eb-1", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_created("fact-eb-2", 1, 2000))
            .await
            .unwrap();
        db.append_memory_event(&make_created("fact-eb-3", 1, 3000))
            .await
            .unwrap();

        let traveler = MemoryTimeTraveler::new(db);

        // Only the t=2000 event
        let events = traveler.events_between(1500, 2500, 100).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].fact_id, "fact-eb-2");

        // All three
        let events = traveler.events_between(500, 3500, 100).await.unwrap();
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn test_events_between_respects_limit() {
        let db = make_db();
        for i in 1..=5 {
            let mut env = make_created(&format!("fact-lim-{i}"), 1, i * 1000);
            env.timestamp = i * 1000;
            db.append_memory_event(&env).await.unwrap();
        }

        let traveler = MemoryTimeTraveler::new(db);
        let events = traveler.events_between(0, 10000, 2).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    // --- explain_fact --------------------------------------------------------

    #[tokio::test]
    async fn test_explain_fact_basic_lifecycle() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-1", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_content_updated(
            "fact-ex-1",
            2,
            2000,
            "User prefers Rust and Go",
        ))
        .await
        .unwrap();
        db.append_memory_event(&make_invalidated("fact-ex-1", 3, 3000))
            .await
            .unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-1").await.unwrap();

        assert_eq!(explanation.fact_id, "fact-ex-1");
        assert_eq!(explanation.events.len(), 3);
        assert!(!explanation.is_valid);
        assert_eq!(explanation.invalidation_reason.as_deref(), Some("outdated"));
        assert!(explanation.creation_source.is_some());
        assert_eq!(explanation.access_count, 0);

        // Check event descriptions
        assert!(explanation.events[0].action.contains("FactCreated"));
        assert!(explanation.events[1].action.contains("FactContentUpdated"));
        assert!(explanation.events[2].action.contains("FactInvalidated"));
    }

    #[tokio::test]
    async fn test_explain_fact_tracks_access_count() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-2", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_accessed("fact-ex-2", 2, 2000, 1))
            .await
            .unwrap();
        db.append_memory_event(&make_accessed("fact-ex-2", 3, 3000, 2))
            .await
            .unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-2").await.unwrap();

        assert_eq!(explanation.access_count, 2);
        assert!(explanation.is_valid);
        assert_eq!(explanation.content.as_deref(), Some("User prefers Rust"));
    }

    #[tokio::test]
    async fn test_explain_fact_deleted_fact() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-3", 1, 1000))
            .await
            .unwrap();

        let mut del_env = MemoryEventEnvelope::new(
            "fact-ex-3".into(),
            2,
            MemoryEvent::FactDeleted {
                fact_id: "fact-ex-3".into(),
                reason: "user requested".into(),
            },
            EventActor::User,
            None,
        );
        del_env.timestamp = 2000;
        db.append_memory_event(&del_env).await.unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-3").await.unwrap();

        assert_eq!(explanation.fact_id, "fact-ex-3");
        assert!(!explanation.is_valid);
        assert!(explanation.content.is_none()); // deleted facts have no content
        assert_eq!(explanation.events.len(), 2);
    }

    #[tokio::test]
    async fn test_explain_nonexistent_fact() {
        let db = make_db();
        let traveler = MemoryTimeTraveler::new(db);
        let result = traveler.explain_fact("ghost").await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No events found"));
    }

    #[tokio::test]
    async fn test_explain_fact_invalidated_then_restored() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-4", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_invalidated("fact-ex-4", 2, 2000))
            .await
            .unwrap();

        let mut restore_env = MemoryEventEnvelope::new(
            "fact-ex-4".into(),
            3,
            MemoryEvent::FactRestored {
                fact_id: "fact-ex-4".into(),
                new_strength: 0.7,
            },
            EventActor::User,
            None,
        );
        restore_env.timestamp = 3000;
        db.append_memory_event(&restore_env).await.unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-4").await.unwrap();

        assert!(explanation.is_valid);
        // Restoration clears invalidation_reason
        assert!(explanation.invalidation_reason.is_none());
        assert_eq!(explanation.events.len(), 3);
    }

    // --- describe_event (unit) -----------------------------------------------

    #[test]
    fn test_describe_event_all_variants() {
        // FactCreated
        let env = make_created("f", 1, 1000);
        let desc = describe_event(&env);
        assert!(desc.contains("Fact created"));
        assert!(desc.contains("ShortTerm"));

        // FactContentUpdated
        let env = make_content_updated("f", 2, 2000, "new stuff");
        let desc = describe_event(&env);
        assert!(desc.contains("Content updated"));
        assert!(desc.contains("correction"));

        // FactInvalidated
        let env = make_invalidated("f", 3, 3000);
        let desc = describe_event(&env);
        assert!(desc.contains("Invalidated"));
        assert!(desc.contains("outdated"));
    }

    // --- truncate (unit) -----------------------------------------------------

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        let result = truncate("hello world this is a long string", 10);
        assert_eq!(result, "hello worl...");
    }

    #[test]
    fn test_truncate_unicode() {
        // Unicode-safe truncation
        let result = truncate("你好世界测试", 4);
        assert_eq!(result, "你好世界...");
    }
}
