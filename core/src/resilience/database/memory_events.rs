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
                    seq: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
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
                    seq: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
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
                    seq: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
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
                    seq: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
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

    /// Check if any FactMigrated events exist (indicates migration has been run).
    pub async fn has_migration_events(&self) -> Result<bool, AlephError> {
        let count = self.count_memory_events(Some("FactMigrated")).await?;
        Ok(count > 0)
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
    async fn test_unique_constraint() {
        let db = make_test_db();
        let e1 = MemoryEventEnvelope::new(
            "fact-dup".into(), 1, make_created_event("fact-dup"), EventActor::Agent, None,
        );
        db.append_memory_event(&e1).await.unwrap();

        // Same fact_id + seq should fail
        let e2 = MemoryEventEnvelope::new(
            "fact-dup".into(), 1, make_created_event("fact-dup"), EventActor::Agent, None,
        );
        assert!(db.append_memory_event(&e2).await.is_err());
    }
}
