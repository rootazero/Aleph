//! SQLite-backed store for collaborative sessions.
//!
//! Uses `Arc<tokio::sync::Mutex<rusqlite::Connection>>` for thread-safe
//! async access, consistent with the sibling team and message stores.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::types::{
    CollaborativeSession, NewSession, SessionOutcome, SessionStatus, SessionTrigger, SessionTurn,
};
use crate::error::AlephError;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("SessionStore: {e}"),
        suggestion: None,
    }
}

fn parse_rfc3339(s: &str) -> crate::error::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| AlephError::ConfigError {
            message: format!("SessionStore: invalid RFC3339 timestamp '{s}': {e}"),
            suggestion: None,
        })
}

// ---------------------------------------------------------------------------
// SessionStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for collaborative sessions.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Create a new active session and return it.
    async fn create_session(&self, input: NewSession)
        -> crate::error::Result<CollaborativeSession>;

    /// Fetch a session by ID, fully materialised with participants and transcript.
    async fn get_session(&self, id: &str) -> crate::error::Result<Option<CollaborativeSession>>;

    /// Add a turn to an existing session. Fails if turn_number > max_rounds.
    async fn add_turn(&self, session_id: &str, turn: SessionTurn) -> crate::error::Result<()>;

    /// Conclude a session with an outcome. Sets status to Concluded.
    async fn conclude_session(
        &self,
        session_id: &str,
        outcome: SessionOutcome,
    ) -> crate::error::Result<()>;

    /// Cancel a session. Sets status to Cancelled.
    async fn cancel_session(&self, session_id: &str) -> crate::error::Result<()>;

    /// List all active sessions for a team.
    async fn list_active_sessions(
        &self,
        team_id: &str,
    ) -> crate::error::Result<Vec<CollaborativeSession>>;

    /// Cancel all active sessions for a team. Returns count of cancelled sessions.
    async fn cancel_all_for_team(&self, team_id: &str) -> crate::error::Result<usize>;
}

// ---------------------------------------------------------------------------
// SqliteSessionStore
// ---------------------------------------------------------------------------

pub struct SqliteSessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSessionStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Run schema migration — creates session tables and indices.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS collaborative_sessions (
                id         TEXT PRIMARY KEY,
                team_id    TEXT NOT NULL,
                topic      TEXT NOT NULL,
                trigger_json TEXT NOT NULL,
                thread_id  TEXT,
                max_rounds INTEGER NOT NULL DEFAULT 10,
                status     TEXT NOT NULL DEFAULT 'active',
                outcome_json TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_participants (
                session_id TEXT NOT NULL,
                agent_id   TEXT NOT NULL,
                PRIMARY KEY (session_id, agent_id)
            );

            CREATE TABLE IF NOT EXISTS session_turns (
                session_id  TEXT NOT NULL,
                turn_number INTEGER NOT NULL,
                agent_id    TEXT NOT NULL,
                content     TEXT NOT NULL,
                timestamp   TEXT NOT NULL,
                PRIMARY KEY (session_id, turn_number)
            );

            CREATE INDEX IF NOT EXISTS idx_collab_sessions_team
                ON collaborative_sessions(team_id, status);
            "#,
        )
        .map_err(db_err)?;

        Ok(())
    }

    /// Convenience constructor for tests — opens an in-memory database and migrates.
    #[cfg(test)]
    pub async fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = Self::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    /// Materialise a full `CollaborativeSession` from the main row plus sub-tables.
    fn materialise(conn: &Connection, id: &str) -> Result<CollaborativeSession, AlephError> {
        let row = conn
            .prepare_cached(
                "SELECT id, team_id, topic, trigger_json, thread_id, max_rounds, status, outcome_json, created_at \
                 FROM collaborative_sessions WHERE id = ?1",
            )
            .map_err(db_err)?
            .query_row(params![id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .optional()
            .map_err(db_err)?;

        let (
            sid,
            team_id,
            topic,
            trigger_json,
            thread_id,
            max_rounds,
            status_str,
            outcome_json,
            created_at_str,
        ) = match row {
            Some(r) => r,
            None => {
                return Err(AlephError::ConfigError {
                    message: format!("SessionStore: session not found: {id}"),
                    suggestion: None,
                });
            }
        };

        // Participants
        let participants = conn
            .prepare_cached(
                "SELECT agent_id FROM session_participants WHERE session_id = ?1 ORDER BY agent_id",
            )
            .map_err(db_err)?
            .query_map(params![sid], |r| r.get::<_, String>(0))
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        // Transcript (ordered by turn_number)
        let transcript = conn
            .prepare_cached(
                "SELECT agent_id, content, turn_number, timestamp \
                 FROM session_turns WHERE session_id = ?1 ORDER BY turn_number ASC",
            )
            .map_err(db_err)?
            .query_map(params![sid], |r| {
                Ok(SessionTurn {
                    agent_id: r.get(0)?,
                    content: r.get(1)?,
                    turn_number: r.get(2)?,
                    timestamp: parse_rfc3339(&r.get::<_, String>(3)?).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                e.to_string(),
                            )),
                        )
                    })?,
                })
            })
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        let trigger: SessionTrigger = serde_json::from_str(&trigger_json)
            .map_err(|e| db_err(format!("bad trigger_json: {e}")))?;

        let outcome: Option<SessionOutcome> = outcome_json
            .as_deref()
            .map(|s| serde_json::from_str(s).map_err(|e| db_err(format!("bad outcome_json: {e}"))))
            .transpose()?;

        Ok(CollaborativeSession {
            id: sid,
            team_id,
            participants,
            topic,
            trigger,
            thread_id,
            max_rounds,
            status: SessionStatus::from_stored(&status_str),
            transcript,
            outcome,
            created_at: parse_rfc3339(&created_at_str)?,
        })
    }
}

// ---------------------------------------------------------------------------
// SessionStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn create_session(
        &self,
        input: NewSession,
    ) -> crate::error::Result<CollaborativeSession> {
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        let trigger_json = serde_json::to_string(&input.trigger)
            .map_err(|e| db_err(format!("serialize trigger: {e}")))?;

        conn.execute(
            r#"
            INSERT INTO collaborative_sessions (id, team_id, topic, trigger_json, thread_id, max_rounds, status, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7)
            "#,
            params![id, input.team_id, input.topic, trigger_json, input.thread_id, input.max_rounds, now_str],
        )
        .map_err(db_err)?;

        // Insert participants
        for agent_id in &input.participants {
            conn.execute(
                "INSERT INTO session_participants (session_id, agent_id) VALUES (?1, ?2)",
                params![id, agent_id],
            )
            .map_err(db_err)?;
        }

        Ok(CollaborativeSession {
            id,
            team_id: input.team_id,
            participants: input.participants,
            topic: input.topic,
            trigger: input.trigger,
            thread_id: input.thread_id,
            max_rounds: input.max_rounds,
            status: SessionStatus::Active,
            transcript: vec![],
            outcome: None,
            created_at: now,
        })
    }

    async fn get_session(&self, id: &str) -> crate::error::Result<Option<CollaborativeSession>> {
        let conn = self.conn.lock().await;

        // Check existence first to distinguish not-found from errors
        let exists: bool = conn
            .prepare_cached("SELECT 1 FROM collaborative_sessions WHERE id = ?1")
            .map_err(db_err)?
            .query_row(params![id], |_| Ok(true))
            .optional()
            .map_err(db_err)?
            .unwrap_or(false);

        if !exists {
            return Ok(None);
        }

        Self::materialise(&conn, id).map(Some)
    }

    async fn add_turn(&self, session_id: &str, turn: SessionTurn) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        // Fetch max_rounds and current status
        let row: Option<(u32, String)> = conn
            .prepare_cached("SELECT max_rounds, status FROM collaborative_sessions WHERE id = ?1")
            .map_err(db_err)?
            .query_row(params![session_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(db_err)?;

        let (max_rounds, status_str) = match row {
            Some(r) => r,
            None => return Err(db_err(format!("session not found: {session_id}"))),
        };

        if status_str != "active" {
            return Err(db_err(format!(
                "cannot add turn to {status_str} session: {session_id}"
            )));
        }

        // Compute turn_number atomically within the locked connection to avoid TOCTOU races.
        // The caller-supplied turn.turn_number is ignored; the authoritative value is derived
        // from the current row count.
        let current_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        let turn_number = current_count + 1;

        if turn_number > max_rounds {
            return Err(AlephError::ConfigError {
                message: format!(
                    "SessionStore: turn {turn_number} exceeds max_rounds {max_rounds} for session {session_id}"
                ),
                suggestion: Some("conclude or cancel the session".into()),
            });
        }

        let ts_str = turn.timestamp.to_rfc3339();
        conn.execute(
            "INSERT INTO session_turns (session_id, turn_number, agent_id, content, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, turn_number, turn.agent_id, turn.content, ts_str],
        )
        .map_err(db_err)?;

        Ok(())
    }

    async fn conclude_session(
        &self,
        session_id: &str,
        outcome: SessionOutcome,
    ) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        let outcome_json = serde_json::to_string(&outcome)
            .map_err(|e| db_err(format!("serialize outcome: {e}")))?;

        let affected = conn
            .execute(
                "UPDATE collaborative_sessions SET status = 'concluded', outcome_json = ?1 WHERE id = ?2 AND status = 'active'",
                params![outcome_json, session_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            return Err(db_err(format!(
                "cannot conclude session (not found or not active): {session_id}"
            )));
        }

        Ok(())
    }

    async fn cancel_session(&self, session_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        let affected = conn
            .execute(
                "UPDATE collaborative_sessions SET status = 'cancelled' WHERE id = ?1 AND status IN ('active', 'deadlocked')",
                params![session_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            return Err(db_err(format!(
                "cannot cancel session (not found or not active/deadlocked): {session_id}"
            )));
        }

        Ok(())
    }

    async fn list_active_sessions(
        &self,
        team_id: &str,
    ) -> crate::error::Result<Vec<CollaborativeSession>> {
        let conn = self.conn.lock().await;

        // Collect IDs first, then materialise
        let ids: Vec<String> = conn
            .prepare_cached(
                "SELECT id FROM collaborative_sessions WHERE team_id = ?1 AND status = 'active' ORDER BY created_at ASC",
            )
            .map_err(db_err)?
            .query_map(params![team_id], |r| r.get::<_, String>(0))
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        let mut sessions = Vec::with_capacity(ids.len());
        for id in &ids {
            sessions.push(Self::materialise(&conn, id)?);
        }

        Ok(sessions)
    }

    async fn cancel_all_for_team(&self, team_id: &str) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;

        let affected = conn
            .execute(
                "UPDATE collaborative_sessions SET status = 'cancelled' WHERE team_id = ?1 AND status IN ('active', 'deadlocked')",
                params![team_id],
            )
            .map_err(db_err)?;

        Ok(affected)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_new_session(team_id: &str) -> NewSession {
        NewSession {
            team_id: team_id.to_string(),
            participants: vec!["agent-a".to_string(), "agent-b".to_string()],
            topic: "Design review".to_string(),
            trigger: SessionTrigger::Explicit {
                requested_by: "agent-a".to_string(),
            },
            thread_id: None,
            max_rounds: 10,
        }
    }

    fn make_turn(agent_id: &str, turn_number: u32) -> SessionTurn {
        SessionTurn {
            agent_id: agent_id.to_string(),
            content: format!("Turn {turn_number} by {agent_id}"),
            turn_number,
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_create_session_and_add_turns() {
        let store = SqliteSessionStore::new_in_memory().await;

        let session = store
            .create_session(make_new_session("team-1"))
            .await
            .unwrap();

        assert_eq!(session.team_id, "team-1");
        assert_eq!(session.status, SessionStatus::Active);
        assert_eq!(session.participants.len(), 2);
        assert!(session.transcript.is_empty());
        assert!(session.outcome.is_none());

        // Add a turn
        store
            .add_turn(&session.id, make_turn("agent-a", 1))
            .await
            .unwrap();

        // Verify transcript
        let fetched = store.get_session(&session.id).await.unwrap().unwrap();
        assert_eq!(fetched.transcript.len(), 1);
        assert_eq!(fetched.transcript[0].agent_id, "agent-a");
        assert_eq!(fetched.transcript[0].turn_number, 1);
    }

    #[tokio::test]
    async fn test_conclude_session() {
        let store = SqliteSessionStore::new_in_memory().await;

        let session = store
            .create_session(make_new_session("team-1"))
            .await
            .unwrap();

        store
            .add_turn(&session.id, make_turn("agent-a", 1))
            .await
            .unwrap();

        let outcome = SessionOutcome {
            conclusion: "We agreed on approach A".to_string(),
            agreed_by: vec!["agent-a".to_string(), "agent-b".to_string()],
            dissent: None,
        };

        store.conclude_session(&session.id, outcome).await.unwrap();

        let fetched = store.get_session(&session.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, SessionStatus::Concluded);
        assert!(fetched.outcome.is_some());
        let out = fetched.outcome.unwrap();
        assert_eq!(out.conclusion, "We agreed on approach A");
        assert_eq!(out.agreed_by.len(), 2);
    }

    #[tokio::test]
    async fn test_max_rounds_enforcement() {
        let store = SqliteSessionStore::new_in_memory().await;

        let session = store
            .create_session(NewSession {
                team_id: "team-1".to_string(),
                participants: vec!["agent-a".to_string(), "agent-b".to_string()],
                topic: "Short discussion".to_string(),
                trigger: SessionTrigger::Explicit {
                    requested_by: "agent-a".to_string(),
                },
                thread_id: None,
                max_rounds: 2,
            })
            .await
            .unwrap();

        // Turn 1 and 2 should succeed
        store
            .add_turn(&session.id, make_turn("agent-a", 1))
            .await
            .unwrap();
        store
            .add_turn(&session.id, make_turn("agent-b", 2))
            .await
            .unwrap();

        // Turn 3 should fail
        let result = store.add_turn(&session.id, make_turn("agent-a", 3)).await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("exceeds max_rounds"));
    }

    #[tokio::test]
    async fn test_cancel_session() {
        let store = SqliteSessionStore::new_in_memory().await;

        let session = store
            .create_session(make_new_session("team-1"))
            .await
            .unwrap();

        store.cancel_session(&session.id).await.unwrap();

        let fetched = store.get_session(&session.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, SessionStatus::Cancelled);

        // Cannot cancel again
        let result = store.cancel_session(&session.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_active_sessions() {
        let store = SqliteSessionStore::new_in_memory().await;

        let s1 = store
            .create_session(make_new_session("team-1"))
            .await
            .unwrap();
        let _s2 = store
            .create_session(make_new_session("team-1"))
            .await
            .unwrap();

        // Both should be active
        let active = store.list_active_sessions("team-1").await.unwrap();
        assert_eq!(active.len(), 2);

        // Cancel the first
        store.cancel_session(&s1.id).await.unwrap();

        // Only one active now
        let active = store.list_active_sessions("team-1").await.unwrap();
        assert_eq!(active.len(), 1);
        assert_ne!(active[0].id, s1.id);
    }

    #[tokio::test]
    async fn test_cancel_all_for_team() {
        let store = SqliteSessionStore::new_in_memory().await;

        store
            .create_session(make_new_session("team-1"))
            .await
            .unwrap();
        store
            .create_session(make_new_session("team-1"))
            .await
            .unwrap();
        store
            .create_session(make_new_session("team-2"))
            .await
            .unwrap();

        let cancelled = store.cancel_all_for_team("team-1").await.unwrap();
        assert_eq!(cancelled, 2);

        let active_t1 = store.list_active_sessions("team-1").await.unwrap();
        assert!(active_t1.is_empty());

        // team-2 unaffected
        let active_t2 = store.list_active_sessions("team-2").await.unwrap();
        assert_eq!(active_t2.len(), 1);
    }

    #[tokio::test]
    async fn test_get_nonexistent_session() {
        let store = SqliteSessionStore::new_in_memory().await;
        let result = store.get_session("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_auto_escalation_trigger() {
        let store = SqliteSessionStore::new_in_memory().await;

        let session = store
            .create_session(NewSession {
                team_id: "team-1".to_string(),
                participants: vec!["agent-a".to_string()],
                topic: "Escalated discussion".to_string(),
                trigger: SessionTrigger::AutoEscalation {
                    thread_id: "thread-123".to_string(),
                    message_count: 5,
                    rule: "too_many_messages".to_string(),
                },
                thread_id: Some("thread-123".to_string()),
                max_rounds: 10,
            })
            .await
            .unwrap();

        let fetched = store.get_session(&session.id).await.unwrap().unwrap();
        assert_eq!(fetched.thread_id.as_deref(), Some("thread-123"));
        match &fetched.trigger {
            SessionTrigger::AutoEscalation {
                thread_id,
                message_count,
                rule,
            } => {
                assert_eq!(thread_id, "thread-123");
                assert_eq!(*message_count, 5);
                assert_eq!(rule, "too_many_messages");
            }
            _ => panic!("expected AutoEscalation trigger"),
        }
    }
}
