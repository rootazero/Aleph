//! Event log system for team activity tracking.
//!
//! Provides structured event types and a SQLite-backed store with retention
//! policy (time-based pruning) for auditing team interactions.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::AlephError;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("EventLogStore: {e}"),
        suggestion: None,
    }
}

// ---------------------------------------------------------------------------
// TeamEventType
// ---------------------------------------------------------------------------

/// Categorizes the kind of activity that occurred within a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamEventType {
    MessageSent,
    MessageRead,
    TaskCreated,
    TaskCompleted,
    TaskFailed,
    ArtifactSubmitted,
    SessionStarted,
    SessionConcluded,
    SessionDeadlocked,
    DigestGenerated,
    ShutdownRequested,
    ShutdownResolved,
    PlanSubmitted,
    PlanResolved,

    // Team lifecycle events (new - EventBus integration)
    TeamCreated,
    MemberAdded,
    MemberRemoved,
    TaskAssigned,
    TaskUpdated,
    TeamDisbanded,
}

impl TeamEventType {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::MessageSent => "message_sent",
            Self::MessageRead => "message_read",
            Self::TaskCreated => "task_created",
            Self::TaskCompleted => "task_completed",
            Self::TaskFailed => "task_failed",
            Self::ArtifactSubmitted => "artifact_submitted",
            Self::SessionStarted => "session_started",
            Self::SessionConcluded => "session_concluded",
            Self::SessionDeadlocked => "session_deadlocked",
            Self::DigestGenerated => "digest_generated",
            Self::ShutdownRequested => "shutdown_requested",
            Self::ShutdownResolved => "shutdown_resolved",
            Self::PlanSubmitted => "plan_submitted",
            Self::PlanResolved => "plan_resolved",
            Self::TeamCreated => "team_created",
            Self::MemberAdded => "member_added",
            Self::MemberRemoved => "member_removed",
            Self::TaskAssigned => "task_assigned",
            Self::TaskUpdated => "task_updated",
            Self::TeamDisbanded => "team_disbanded",
        }
    }

    #[must_use]
    pub fn from_stored(s: &str) -> Option<Self> {
        match s {
            "message_sent" => Some(Self::MessageSent),
            "message_read" => Some(Self::MessageRead),
            "task_created" => Some(Self::TaskCreated),
            "task_completed" => Some(Self::TaskCompleted),
            "task_failed" => Some(Self::TaskFailed),
            "artifact_submitted" => Some(Self::ArtifactSubmitted),
            "session_started" => Some(Self::SessionStarted),
            "session_concluded" => Some(Self::SessionConcluded),
            "session_deadlocked" => Some(Self::SessionDeadlocked),
            "digest_generated" => Some(Self::DigestGenerated),
            "shutdown_requested" => Some(Self::ShutdownRequested),
            "shutdown_resolved" => Some(Self::ShutdownResolved),
            "plan_submitted" => Some(Self::PlanSubmitted),
            "plan_resolved" => Some(Self::PlanResolved),
            "team_created" => Some(Self::TeamCreated),
            "member_added" => Some(Self::MemberAdded),
            "member_removed" => Some(Self::MemberRemoved),
            "task_assigned" => Some(Self::TaskAssigned),
            "task_updated" => Some(Self::TaskUpdated),
            "team_disbanded" => Some(Self::TeamDisbanded),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TeamEvent / NewTeamEvent
// ---------------------------------------------------------------------------

/// A recorded team event with assigned ID and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamEvent {
    pub id: String,
    pub team_id: String,
    pub event_type: TeamEventType,
    pub agent_id: String,
    pub payload: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

/// Input for creating a new team event (ID and timestamp assigned by the store).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTeamEvent {
    pub team_id: String,
    pub event_type: TeamEventType,
    pub agent_id: String,
    pub payload: serde_json::Value,
}

// ---------------------------------------------------------------------------
// EventLogStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for team event logging.
#[async_trait]
pub trait EventLogStore: Send + Sync {
    /// Record a new event and return the stored event with assigned ID and timestamp.
    async fn log_event(&self, input: NewTeamEvent) -> crate::error::Result<TeamEvent>;

    /// Query events for a team, optionally filtered by a time range.
    async fn get_events(
        &self,
        team_id: &str,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> crate::error::Result<Vec<TeamEvent>>;

    /// Delete events older than `max_age` for a team. Returns the number of pruned rows.
    async fn prune_events(
        &self,
        team_id: &str,
        max_age: chrono::Duration,
    ) -> crate::error::Result<usize>;

    /// Hard-delete all events for a team. Returns rows deleted.
    async fn delete_team_events(&self, team_id: &str) -> crate::error::Result<usize>;
}

// ---------------------------------------------------------------------------
// SqliteEventLogStore
// ---------------------------------------------------------------------------

pub struct SqliteEventLogStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteEventLogStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Convenience constructor for tests — opens an in-memory database and migrates.
    #[cfg(test)]
    pub async fn new_in_memory() -> Self {
        // rust-doctor-disable-next-line unwrap-in-production
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = Self::new(conn);
        // rust-doctor-disable-next-line unwrap-in-production
        store.migrate().await.expect("migrate");
        store
    }

    /// Run schema migration — creates the `team_events` table.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS team_events (
                id         TEXT PRIMARY KEY,
                team_id    TEXT NOT NULL,
                event_type TEXT NOT NULL,
                agent_id   TEXT NOT NULL,
                payload    TEXT NOT NULL DEFAULT '{}',
                timestamp  TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_team_events_team_ts
                ON team_events(team_id, timestamp);
            "#,
        )
        .map_err(db_err)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper row reader
// ---------------------------------------------------------------------------

fn read_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<TeamEvent>> {
    let event_type_str: String = row.get(2)?;
    // Skip rows with an unrecognized event_type instead of failing the whole
    // query, mirroring the lenient artifact readers. Genuine corruption (bad
    // payload JSON / timestamp) below still surfaces as a hard error.
    let Some(event_type) = TeamEventType::from_stored(&event_type_str) else {
        return Ok(None);
    };

    let payload_str: String = row.get(4)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid payload JSON: {e}"),
            )),
        )
    })?;

    let ts_str: String = row.get(5)?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
        })?;

    Ok(Some(TeamEvent {
        id: row.get(0)?,
        team_id: row.get(1)?,
        event_type,
        agent_id: row.get(3)?,
        payload,
        timestamp,
    }))
}

// ---------------------------------------------------------------------------
// EventLogStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl EventLogStore for SqliteEventLogStore {
    async fn log_event(&self, input: NewTeamEvent) -> crate::error::Result<TeamEvent> {
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let ts_str = now.to_rfc3339();
        let payload_str = serde_json::to_string(&input.payload)
            .map_err(|e| db_err(format!("failed to serialize event payload: {e}")))?;

        conn.execute(
            r#"
            INSERT INTO team_events (id, team_id, event_type, agent_id, payload, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                id,
                input.team_id,
                input.event_type.as_str(),
                input.agent_id,
                payload_str,
                ts_str,
            ],
        )
        .map_err(db_err)?;

        Ok(TeamEvent {
            id,
            team_id: input.team_id,
            event_type: input.event_type,
            agent_id: input.agent_id,
            payload: input.payload,
            timestamp: now,
        })
    }

    async fn get_events(
        &self,
        team_id: &str,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> crate::error::Result<Vec<TeamEvent>> {
        let conn = self.conn.lock().await;

        let (sql, dynamic_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (since, until) {
                (Some(s), Some(u)) => (
                    "SELECT id, team_id, event_type, agent_id, payload, timestamp \
                     FROM team_events WHERE team_id = ?1 AND timestamp >= ?2 AND timestamp <= ?3 \
                     ORDER BY timestamp ASC"
                        .into(),
                    vec![
                        Box::new(team_id.to_owned()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(s.to_rfc3339()),
                        Box::new(u.to_rfc3339()),
                    ],
                ),
                (Some(s), None) => (
                    "SELECT id, team_id, event_type, agent_id, payload, timestamp \
                     FROM team_events WHERE team_id = ?1 AND timestamp >= ?2 \
                     ORDER BY timestamp ASC"
                        .into(),
                    vec![
                        Box::new(team_id.to_owned()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(s.to_rfc3339()),
                    ],
                ),
                (None, Some(u)) => (
                    "SELECT id, team_id, event_type, agent_id, payload, timestamp \
                     FROM team_events WHERE team_id = ?1 AND timestamp <= ?2 \
                     ORDER BY timestamp ASC"
                        .into(),
                    vec![
                        Box::new(team_id.to_owned()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(u.to_rfc3339()),
                    ],
                ),
                (None, None) => (
                    "SELECT id, team_id, event_type, agent_id, payload, timestamp \
                     FROM team_events WHERE team_id = ?1 \
                     ORDER BY timestamp ASC"
                        .into(),
                    vec![Box::new(team_id.to_owned()) as Box<dyn rusqlite::types::ToSql>],
                ),
            };

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            dynamic_params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let events = stmt
            .query_map(params_refs.as_slice(), read_event_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?
            .into_iter()
            .flatten()
            .collect();

        Ok(events)
    }

    async fn prune_events(
        &self,
        team_id: &str,
        max_age: chrono::Duration,
    ) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let cutoff = Utc::now() - max_age;
        let cutoff_str = cutoff.to_rfc3339();

        let deleted = conn
            .execute(
                "DELETE FROM team_events WHERE team_id = ?1 AND timestamp < ?2",
                params![team_id, cutoff_str],
            )
            .map_err(db_err)?;

        Ok(deleted)
    }

    async fn delete_team_events(&self, team_id: &str) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let n = conn
            .execute(
                "DELETE FROM team_events WHERE team_id = ?1",
                params![team_id],
            )
            .map_err(db_err)?;
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// TeamEventLogger — EventHandler for EventBus integration
// ---------------------------------------------------------------------------

use crate::event::{AlephEvent, EventType};
use crate::event::{EventContext, EventHandler, HandlerError};

/// Event handler that listens for team-related `AlephEvents` and logs them
/// to the team's event store.
pub struct TeamEventLogger {
    store: Arc<dyn EventLogStore>,
}

impl TeamEventLogger {
    /// Create a new logger wrapping the given event store.
    pub fn new(store: Arc<dyn EventLogStore>) -> Self {
        Self { store }
    }

    /// Map an `AlephEvent` team variant to a `TeamEventType` and extract metadata.
    fn map_event(event: &AlephEvent) -> Option<(TeamEventType, String, serde_json::Value)> {
        match event {
            AlephEvent::TeamCreated {
                team_id,
                name,
                member_ids,
            } => Some((
                TeamEventType::TeamCreated,
                team_id.clone(),
                serde_json::json!({"name": name, "member_ids": member_ids}),
            )),
            AlephEvent::TeamMemberAdded {
                team_id,
                member_id,
                role,
            } => Some((
                TeamEventType::MemberAdded,
                team_id.clone(),
                serde_json::json!({"member_id": member_id, "role": role}),
            )),
            AlephEvent::TeamMemberRemoved { team_id, member_id } => Some((
                TeamEventType::MemberRemoved,
                team_id.clone(),
                serde_json::json!({"member_id": member_id}),
            )),
            AlephEvent::TeamTaskAssigned {
                team_id,
                task_id,
                assignee_id,
            } => Some((
                TeamEventType::TaskAssigned,
                team_id.clone(),
                serde_json::json!({"task_id": task_id, "assignee_id": assignee_id}),
            )),
            AlephEvent::TeamTaskUpdated {
                team_id,
                task_id,
                status,
                progress,
            } => Some((
                TeamEventType::TaskUpdated,
                team_id.clone(),
                serde_json::json!({"task_id": task_id, "status": status, "progress": progress}),
            )),
            AlephEvent::TeamTaskCompleted {
                team_id,
                task_id,
                result_summary,
            } => Some((
                TeamEventType::TaskCompleted,
                team_id.clone(),
                serde_json::json!({"task_id": task_id, "result_summary": result_summary}),
            )),
            AlephEvent::TeamTaskFailed {
                team_id,
                task_id,
                error,
            } => Some((
                TeamEventType::TaskFailed,
                team_id.clone(),
                serde_json::json!({"task_id": task_id, "error": error}),
            )),
            AlephEvent::TeamDisbanded { team_id } => Some((
                TeamEventType::TeamDisbanded,
                team_id.clone(),
                serde_json::json!({}),
            )),
            // NOTE: message sends are logged directly by `MessageRouter::send`
            // (`TeamEventType::MessageSent`) — no global-bus event exists for
            // them (the write-only `TeamMessageSent` chain was removed).
            _ => None,
        }
    }
}

#[async_trait]
impl EventHandler for TeamEventLogger {
    fn name(&self) -> &'static str {
        "TeamEventLogger"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::TeamCreated,
            EventType::TeamMemberAdded,
            EventType::TeamMemberRemoved,
            EventType::TeamTaskAssigned,
            EventType::TeamTaskUpdated,
            EventType::TeamTaskCompleted,
            EventType::TeamTaskFailed,
            EventType::TeamDisbanded,
        ]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        let Some((event_type, team_id, payload)) = Self::map_event(event) else {
            return Ok(vec![]);
        };

        let new_event = NewTeamEvent {
            team_id,
            event_type,
            agent_id: "system".to_string(),
            payload,
        };

        if let Err(e) = self.store.log_event(new_event).await {
            tracing::warn!(error = %e, "Failed to log team event");
        }

        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_log_and_query_events() {
        let store = SqliteEventLogStore::new_in_memory().await;

        let event = store
            .log_event(NewTeamEvent {
                team_id: "team-1".into(),
                event_type: TeamEventType::MessageSent,
                agent_id: "agent-a".into(),
                payload: json!({"text": "hello"}),
            })
            .await
            .unwrap();

        assert!(!event.id.is_empty());
        assert_eq!(event.team_id, "team-1");
        assert_eq!(event.event_type, TeamEventType::MessageSent);
        assert_eq!(event.agent_id, "agent-a");
        assert_eq!(event.payload, json!({"text": "hello"}));

        let events = store.get_events("team-1", None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, event.id);
        assert_eq!(events[0].event_type, TeamEventType::MessageSent);
    }

    #[tokio::test]
    async fn test_prune_old_events() {
        let store = SqliteEventLogStore::new_in_memory().await;

        // Insert with an explicit past timestamp via direct SQL to avoid timing sensitivity
        let past = (Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO team_events (id, team_id, event_type, agent_id, payload, timestamp) VALUES (?1,?2,?3,?4,?5,?6)",
                params!["prune-evt", "team-2", "task_created", "agent-b", "{}", past],
            ).unwrap();
        }

        // Prune with 5-second max_age — the 10-second-old event should be deleted
        let pruned = store
            .prune_events("team-2", chrono::Duration::seconds(5))
            .await
            .unwrap();
        assert_eq!(pruned, 1);

        let remaining = store.get_events("team-2", None, None).await.unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn test_query_events_with_time_range() {
        let store = SqliteEventLogStore::new_in_memory().await;

        // Log three events with slightly different timestamps by inserting directly
        let conn = store.conn.lock().await;
        let base = Utc::now();

        let ts1 = (base - chrono::Duration::hours(3)).to_rfc3339();
        let ts2 = (base - chrono::Duration::hours(1)).to_rfc3339();
        let ts3 = base.to_rfc3339();

        conn.execute(
            "INSERT INTO team_events (id, team_id, event_type, agent_id, payload, timestamp) VALUES (?1,?2,?3,?4,?5,?6)",
            params!["e1", "team-3", "task_created", "a1", "{}", ts1],
        ).unwrap();
        conn.execute(
            "INSERT INTO team_events (id, team_id, event_type, agent_id, payload, timestamp) VALUES (?1,?2,?3,?4,?5,?6)",
            params!["e2", "team-3", "task_completed", "a1", "{}", ts2],
        ).unwrap();
        conn.execute(
            "INSERT INTO team_events (id, team_id, event_type, agent_id, payload, timestamp) VALUES (?1,?2,?3,?4,?5,?6)",
            params!["e3", "team-3", "digest_generated", "a1", "{}", ts3],
        ).unwrap();
        drop(conn);

        // Query with since = 2 hours ago — should exclude e1
        let since = base - chrono::Duration::hours(2);
        let events = store.get_events("team-3", Some(since), None).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e2");
        assert_eq!(events[1].id, "e3");

        // Query with until = 2 hours ago — should include only e1
        let until = base - chrono::Duration::hours(2);
        let events = store.get_events("team-3", None, Some(until)).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "e1");

        // Query with both since and until — should include only e2
        let events = store
            .get_events(
                "team-3",
                Some(since),
                Some(until + chrono::Duration::hours(1)),
            )
            .await
            .unwrap();
        // since = base-2h, until = base-1h — should get e2
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "e2");
    }

    #[tokio::test]
    async fn delete_team_events_removes_team_rows_only() {
        let store = SqliteEventLogStore::new_in_memory().await;

        store
            .log_event(NewTeamEvent {
                team_id: "team-A".into(),
                event_type: TeamEventType::SessionStarted,
                agent_id: "agent-a".into(),
                payload: json!({}),
            })
            .await
            .unwrap();

        store
            .log_event(NewTeamEvent {
                team_id: "team-B".into(),
                event_type: TeamEventType::SessionStarted,
                agent_id: "agent-b".into(),
                payload: json!({}),
            })
            .await
            .unwrap();

        let n = store.delete_team_events("team-A").await.unwrap();
        assert_eq!(n, 1);
        assert!(store
            .get_events("team-A", None, None)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store.get_events("team-B", None, None).await.unwrap().len(),
            1
        );
    }
}
