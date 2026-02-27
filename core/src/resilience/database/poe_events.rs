//! CRUD operations for poe_events table
//!
//! Follows the same pattern as memory_events.rs.

use crate::error::AlephError;
use crate::poe::events::{EventTier, PoeEvent, PoeEventEnvelope};
use super::StateDatabase;
use rusqlite::params;

impl StateDatabase {
    // =========================================================================
    // POE Events CRUD
    // =========================================================================

    /// Append a single POE event. Returns the assigned global ID.
    pub async fn append_poe_event(
        &self,
        envelope: &PoeEventEnvelope,
    ) -> Result<i64, AlephError> {
        let event_json = serde_json::to_string(&envelope.event)
            .map_err(|e| AlephError::other(format!("Failed to serialize POE event: {e}")))?;
        let tier = if envelope.event.is_skeleton() { "skeleton" } else { "pulse" };

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO poe_events (task_id, seq, event_type, event_json, tier, timestamp, correlation_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                envelope.task_id,
                envelope.seq,
                envelope.event.event_type_tag(),
                event_json,
                tier,
                envelope.timestamp,
                envelope.correlation_id,
            ],
        )
        .map_err(|e| AlephError::other(format!("Failed to append POE event: {e}")))?;

        Ok(conn.last_insert_rowid())
    }

    /// Get all events for a task, ordered by seq.
    pub async fn get_poe_events_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<PoeEventEnvelope>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, seq, event_type, event_json, tier, timestamp, correlation_id
                FROM poe_events
                WHERE task_id = ?1
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map(params![task_id], |row| {
                Ok(PoeEventRow {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get::<_, i64>(2)? as u32,
                    _event_type: row.get::<_, String>(3)?,
                    event_json: row.get(4)?,
                    _tier: row.get::<_, String>(5)?,
                    timestamp: row.get(6)?,
                    correlation_id: row.get(7)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query POE events: {e}")))?;

        let mut envelopes = Vec::new();
        for row in rows {
            let row = row.map_err(|e| AlephError::other(format!("Row error: {e}")))?;
            envelopes.push(row.into_envelope()?);
        }
        Ok(envelopes)
    }

    /// Get events across all tasks within a time range.
    pub async fn get_poe_events_in_range(
        &self,
        from_timestamp: i64,
        to_timestamp: i64,
        limit: usize,
    ) -> Result<Vec<PoeEventEnvelope>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, seq, event_type, event_json, tier, timestamp, correlation_id
                FROM poe_events
                WHERE timestamp >= ?1 AND timestamp <= ?2
                ORDER BY id ASC
                LIMIT ?3
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map(params![from_timestamp, to_timestamp, limit as i64], |row| {
                Ok(PoeEventRow {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get::<_, i64>(2)? as u32,
                    _event_type: row.get::<_, String>(3)?,
                    event_json: row.get(4)?,
                    _tier: row.get::<_, String>(5)?,
                    timestamp: row.get(6)?,
                    correlation_id: row.get(7)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to query POE events: {e}")))?;

        let mut envelopes = Vec::new();
        for row in rows {
            let row = row.map_err(|e| AlephError::other(format!("Row error: {e}")))?;
            envelopes.push(row.into_envelope()?);
        }
        Ok(envelopes)
    }

    /// Count total POE events, optionally filtered by event type.
    pub async fn count_poe_events(
        &self,
        event_type_filter: Option<&str>,
    ) -> Result<usize, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = match event_type_filter {
            Some(et) => conn
                .query_row(
                    "SELECT COUNT(*) FROM poe_events WHERE event_type = ?1",
                    params![et],
                    |row| row.get(0),
                )
                .map_err(|e| AlephError::other(format!("Failed to count POE events: {e}")))?,
            None => conn
                .query_row("SELECT COUNT(*) FROM poe_events", [], |row| row.get(0))
                .map_err(|e| AlephError::other(format!("Failed to count POE events: {e}")))?,
        };
        Ok(count as usize)
    }
}

// ---------------------------------------------------------------------------
// Internal helper for row mapping
// ---------------------------------------------------------------------------

struct PoeEventRow {
    id: i64,
    task_id: String,
    seq: u32,
    _event_type: String,
    event_json: String,
    _tier: String,
    timestamp: i64,
    correlation_id: Option<String>,
}

impl PoeEventRow {
    fn into_envelope(self) -> Result<PoeEventEnvelope, AlephError> {
        let event: PoeEvent = serde_json::from_str(&self.event_json)
            .map_err(|e| AlephError::other(format!("Failed to deserialize POE event: {e}")))?;
        let tier = if event.is_skeleton() { EventTier::Skeleton } else { EventTier::Pulse };
        Ok(PoeEventEnvelope {
            id: self.id,
            task_id: self.task_id,
            seq: self.seq,
            event,
            tier,
            timestamp: self.timestamp,
            correlation_id: self.correlation_id,
        })
    }
}

#[cfg(test)]
mod tests {
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
        let e1 = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OperationAttempted {
                task_id: "t1".into(),
                attempt: 1,
                tokens_used: 100,
            },
            None,
        );
        let e2 = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OperationAttempted {
                task_id: "t1".into(),
                attempt: 2,
                tokens_used: 200,
            },
            None,
        );

        db.append_poe_event(&e1).await.unwrap();
        assert!(db.append_poe_event(&e2).await.is_err());
    }

    #[tokio::test]
    async fn test_count_poe_events() {
        let db = StateDatabase::in_memory().unwrap();
        let e1 = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::ManifestCreated {
                task_id: "t1".into(),
                objective: "a".into(),
                hard_constraints_count: 0,
                soft_metrics_count: 0,
            },
            None,
        );
        let e2 = PoeEventEnvelope::new(
            "t1".into(),
            1,
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

        db.append_poe_event(&e1).await.unwrap();
        db.append_poe_event(&e2).await.unwrap();

        assert_eq!(db.count_poe_events(None).await.unwrap(), 2);
        assert_eq!(
            db.count_poe_events(Some("ManifestCreated")).await.unwrap(),
            1
        );
    }
}
