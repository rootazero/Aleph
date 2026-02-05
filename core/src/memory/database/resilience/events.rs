//! CRUD operations for agent_events table
//!
//! Provides database operations for tiered event persistence
//! using the Skeleton & Pulse model.

use crate::error::AlephError;
use crate::memory::database::resilience::AgentEvent;
use crate::memory::database::VectorDatabase;
use rusqlite::params;
use rusqlite::OptionalExtension;

impl VectorDatabase {
    // =========================================================================
    // Agent Events CRUD
    // =========================================================================

    /// Insert a single event
    pub async fn insert_event(&self, event: &AgentEvent) -> Result<i64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO agent_events (task_id, seq, event_type, payload_json, is_structural, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                event.task_id,
                event.seq,
                event.event_type,
                event.payload_json,
                event.is_structural as i32,
                event.timestamp,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert event: {}", e)))?;

        Ok(conn.last_insert_rowid())
    }

    /// Bulk insert events (for pulse buffering)
    pub async fn bulk_insert_events(&self, events: &[AgentEvent]) -> Result<(), AlephError> {
        if events.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                INSERT INTO agent_events (task_id, seq, event_type, payload_json, is_structural, timestamp)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare statement: {}", e)))?;

        for event in events {
            stmt.execute(params![
                event.task_id,
                event.seq,
                event.event_type,
                event.payload_json,
                event.is_structural as i32,
                event.timestamp,
            ])
            .map_err(|e| AlephError::config(format!("Failed to insert event: {}", e)))?;
        }

        Ok(())
    }

    /// Get all events for a task (ordered by seq)
    pub async fn get_events_by_task(&self, task_id: &str) -> Result<Vec<AgentEvent>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, seq, event_type, payload_json, is_structural, timestamp
                FROM agent_events
                WHERE task_id = ?1
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let events = stmt
            .query_map(params![task_id], |row| {
                Ok(AgentEvent {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get(2)?,
                    event_type: row.get(3)?,
                    payload_json: row.get(4)?,
                    is_structural: row.get::<_, i32>(5)? != 0,
                    timestamp: row.get(6)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query events: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect events: {}", e)))?;

        Ok(events)
    }

    /// Get events since a specific seq (for Gap-Fill logic)
    pub async fn get_events_since_seq(
        &self,
        task_id: &str,
        since_seq: u64,
    ) -> Result<Vec<AgentEvent>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, seq, event_type, payload_json, is_structural, timestamp
                FROM agent_events
                WHERE task_id = ?1 AND seq > ?2
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let events = stmt
            .query_map(params![task_id, since_seq], |row| {
                Ok(AgentEvent {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get(2)?,
                    event_type: row.get(3)?,
                    payload_json: row.get(4)?,
                    is_structural: row.get::<_, i32>(5)? != 0,
                    timestamp: row.get(6)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query events: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect events: {}", e)))?;

        Ok(events)
    }

    /// Get events in a range (for Gap-Fill: fill gaps between seq numbers)
    pub async fn get_events_in_range(
        &self,
        task_id: &str,
        from_seq: u64,
        to_seq: u64,
    ) -> Result<Vec<AgentEvent>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, seq, event_type, payload_json, is_structural, timestamp
                FROM agent_events
                WHERE task_id = ?1 AND seq >= ?2 AND seq <= ?3
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let events = stmt
            .query_map(params![task_id, from_seq, to_seq], |row| {
                Ok(AgentEvent {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get(2)?,
                    event_type: row.get(3)?,
                    payload_json: row.get(4)?,
                    is_structural: row.get::<_, i32>(5)? != 0,
                    timestamp: row.get(6)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query events: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect events: {}", e)))?;

        Ok(events)
    }

    /// Get only structural (skeleton) events for a task
    pub async fn get_structural_events(
        &self,
        task_id: &str,
    ) -> Result<Vec<AgentEvent>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, seq, event_type, payload_json, is_structural, timestamp
                FROM agent_events
                WHERE task_id = ?1 AND is_structural = 1
                ORDER BY seq ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let events = stmt
            .query_map(params![task_id], |row| {
                Ok(AgentEvent {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get(2)?,
                    event_type: row.get(3)?,
                    payload_json: row.get(4)?,
                    is_structural: true,
                    timestamp: row.get(6)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query events: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect events: {}", e)))?;

        Ok(events)
    }

    /// Get the latest seq number for a task
    pub async fn get_latest_event_seq(&self, task_id: &str) -> Result<Option<u64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result: Option<u64> = conn
            .query_row(
                "SELECT MAX(seq) FROM agent_events WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get latest seq: {}", e)))?
            .flatten();

        Ok(result)
    }

    /// Delete all events for a task (cleanup)
    pub async fn delete_events_for_task(&self, task_id: &str) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count = conn
            .execute(
                "DELETE FROM agent_events WHERE task_id = ?1",
                params![task_id],
            )
            .map_err(|e| AlephError::config(format!("Failed to delete events: {}", e)))?;
        Ok(count as u64)
    }

    /// Get event count for a task
    pub async fn get_event_count(&self, task_id: &str) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_events WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to count events: {}", e)))?;
        Ok(count as u64)
    }
}
