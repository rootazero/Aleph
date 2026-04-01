//! SQLite-backed implementation of [`TeamStore`].
//!
//! Uses `Arc<tokio::sync::Mutex<rusqlite::Connection>>` for thread-safe
//! async access to the team management tables.

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::types::{NewTeam, NewTeamMember, Team, TeamMember, TeamStatus, TeamSummary};
use crate::error::AlephError;

// ---------------------------------------------------------------------------
// FromSql implementations — propagate parse errors instead of unwrap_or_default
// ---------------------------------------------------------------------------

impl rusqlite::types::FromSql for TeamStatus {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        s.parse::<TeamStatus>().map_err(|e| {
            rusqlite::types::FromSqlError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e,
            )))
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_epoch() -> i64 {
    chrono::Utc::now().timestamp()
}

fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("TeamStore: {e}"),
        suggestion: None,
    }
}

// ---------------------------------------------------------------------------
// TeamStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for the team management system.
#[async_trait]
pub trait TeamStore: Send + Sync {
    /// Create a new active team and return it.
    async fn create_team(&self, input: NewTeam) -> crate::error::Result<Team>;

    /// Fetch a team by its ID. Returns `None` if not found.
    async fn get_team(&self, id: &str) -> crate::error::Result<Option<Team>>;

    /// List all teams as lightweight summaries (with member/task counts).
    async fn list_teams(&self) -> crate::error::Result<Vec<TeamSummary>>;

    /// Mark a team as disbanded. Sets status to `Disbanded` and records `disbanded_at`.
    async fn disband_team(&self, id: &str) -> crate::error::Result<()>;

    /// Permanently delete a team. Only disbanded teams may be deleted.
    async fn delete_team(&self, id: &str) -> crate::error::Result<()>;

    /// Add a member to an existing team.
    async fn add_member(&self, input: NewTeamMember) -> crate::error::Result<TeamMember>;

    /// Return all members of a team.
    async fn get_members(&self, team_id: &str) -> crate::error::Result<Vec<TeamMember>>;

    /// Return summary records for all teams that an agent belongs to (as leader or member).
    async fn get_agent_teams(&self, agent_id: &str) -> crate::error::Result<Vec<TeamSummary>>;
}

// ---------------------------------------------------------------------------
// SqliteTeamStore
// ---------------------------------------------------------------------------

pub struct SqliteTeamStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTeamStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Run schema migration — creates the `teams`, `team_members`, and `team_tasks` tables.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(db_err)?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS teams (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                leader_id   TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'active',
                created_at  INTEGER NOT NULL,
                disbanded_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS team_members (
                team_id   TEXT NOT NULL,
                agent_id  TEXT NOT NULL,
                role      TEXT NOT NULL DEFAULT '',
                joined_at INTEGER NOT NULL,
                PRIMARY KEY (team_id, agent_id),
                FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS team_tasks (
                id           TEXT PRIMARY KEY,
                team_id      TEXT NOT NULL,
                agent_id     TEXT NOT NULL,
                subject      TEXT NOT NULL,
                status       TEXT NOT NULL DEFAULT 'pending',
                result       TEXT,
                created_at   INTEGER NOT NULL,
                completed_at INTEGER,
                FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_team_members_agent ON team_members(agent_id);
            CREATE INDEX IF NOT EXISTS idx_team_tasks_team    ON team_tasks(team_id);
            CREATE INDEX IF NOT EXISTS idx_team_tasks_agent   ON team_tasks(agent_id);
            "#,
        )
        .map_err(db_err)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper row readers
// ---------------------------------------------------------------------------

fn read_team_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Team> {
    Ok(Team {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        leader_id: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        disbanded_at: row.get(6)?,
    })
}

fn read_member_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamMember> {
    Ok(TeamMember {
        team_id: row.get(0)?,
        agent_id: row.get(1)?,
        role: row.get(2)?,
        joined_at: row.get(3)?,
    })
}

fn read_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamSummary> {
    Ok(TeamSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        leader_id: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        disbanded_at: row.get(6)?,
        member_count: row.get(7)?,
        task_count: row.get(8)?,
    })
}

// ---------------------------------------------------------------------------
// TeamStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl TeamStore for SqliteTeamStore {
    async fn create_team(&self, input: NewTeam) -> crate::error::Result<Team> {
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_epoch();

        conn.execute(
            r#"
            INSERT INTO teams (id, name, description, leader_id, status, created_at)
            VALUES (?1, ?2, ?3, ?4, 'active', ?5)
            "#,
            params![id, input.name, input.description, input.leader_id, now],
        )
        .map_err(db_err)?;

        Ok(Team {
            id,
            name: input.name,
            description: input.description,
            leader_id: input.leader_id,
            status: TeamStatus::Active,
            created_at: now,
            disbanded_at: None,
        })
    }

    async fn get_team(&self, id: &str) -> crate::error::Result<Option<Team>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, name, description, leader_id, status, created_at, disbanded_at FROM teams WHERE id = ?1",
            )
            .map_err(db_err)?;
        stmt.query_row(params![id], read_team_row)
            .optional()
            .map_err(db_err)
    }

    async fn list_teams(&self) -> crate::error::Result<Vec<TeamSummary>> {
        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare_cached(
                r#"
                SELECT t.id, t.name, t.description, t.leader_id, t.status,
                       t.created_at, t.disbanded_at,
                       COUNT(DISTINCT m.agent_id) AS member_count,
                       COUNT(DISTINCT k.id) AS task_count
                FROM teams t
                LEFT JOIN team_members m ON m.team_id = t.id
                LEFT JOIN team_tasks k ON k.team_id = t.id
                GROUP BY t.id
                ORDER BY t.created_at ASC
                "#,
            )
            .map_err(db_err)?;

        let summaries = stmt
            .query_map([], read_summary_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(summaries)
    }

    async fn disband_team(&self, id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        let now = now_epoch();

        // Only disband active teams — prevents overwriting disbanded_at on re-disband
        let affected = conn
            .execute(
                "UPDATE teams SET status = 'disbanded', disbanded_at = ?1 WHERE id = ?2 AND status = 'active'",
                params![now, id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            // Distinguish "not found" from "already disbanded"
            let exists: bool = conn
                .prepare_cached("SELECT 1 FROM teams WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![id], |_| Ok(true))
                .optional()
                .map_err(db_err)?
                .unwrap_or(false);
            if exists {
                return Err(db_err(format!("team already disbanded: {id}")));
            }
            return Err(db_err(format!("team not found: {id}")));
        }
        Ok(())
    }

    async fn delete_team(&self, id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        // Only disbanded teams may be deleted
        let status: Option<String> = conn
            .prepare_cached("SELECT status FROM teams WHERE id = ?1")
            .map_err(db_err)?
            .query_row(params![id], |r| r.get(0))
            .optional()
            .map_err(db_err)?;

        match status.as_deref() {
            None => return Err(db_err(format!("team not found: {id}"))),
            Some("active") => {
                return Err(AlephError::ConfigError {
                    message: format!("team {id} must be disbanded before deletion"),
                    suggestion: Some("call disband_team first".into()),
                })
            }
            _ => {}
        }

        conn.execute("DELETE FROM teams WHERE id = ?1", params![id])
            .map_err(db_err)?;
        Ok(())
    }

    async fn add_member(&self, input: NewTeamMember) -> crate::error::Result<TeamMember> {
        let conn = self.conn.lock().await;

        // Reject adding members to disbanded teams
        let status: Option<String> = conn
            .prepare_cached("SELECT status FROM teams WHERE id = ?1")
            .map_err(db_err)?
            .query_row(params![input.team_id], |r| r.get(0))
            .optional()
            .map_err(db_err)?;

        match status.as_deref() {
            None => return Err(db_err(format!("team not found: {}", input.team_id))),
            Some("disbanded") => {
                return Err(db_err(format!(
                    "cannot add member to disbanded team: {}",
                    input.team_id
                )))
            }
            _ => {}
        }

        let now = now_epoch();

        let affected = conn
            .execute(
                r#"
            INSERT OR IGNORE INTO team_members (team_id, agent_id, role, joined_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
                params![input.team_id, input.agent_id, input.role, now],
            )
            .map_err(db_err)?;

        if affected == 0 {
            // Already a member — return the existing record
            let member = conn
                .prepare_cached(
                    "SELECT team_id, agent_id, role, joined_at FROM team_members WHERE team_id = ?1 AND agent_id = ?2",
                )
                .map_err(db_err)?
                .query_row(params![input.team_id, input.agent_id], read_member_row)
                .map_err(db_err)?;
            return Ok(member);
        }

        Ok(TeamMember {
            team_id: input.team_id,
            agent_id: input.agent_id,
            role: input.role,
            joined_at: now,
        })
    }

    async fn get_members(&self, team_id: &str) -> crate::error::Result<Vec<TeamMember>> {
        let conn = self.conn.lock().await;

        let members = conn
            .prepare_cached(
                "SELECT team_id, agent_id, role, joined_at FROM team_members WHERE team_id = ?1 ORDER BY joined_at ASC",
            )
            .map_err(db_err)?
            .query_map(params![team_id], read_member_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(members)
    }

    async fn get_agent_teams(&self, agent_id: &str) -> crate::error::Result<Vec<TeamSummary>> {
        let conn = self.conn.lock().await;

        // Single query: find teams where agent is leader or member, with counts
        let mut stmt = conn
            .prepare_cached(
                r#"
                SELECT t.id, t.name, t.description, t.leader_id, t.status,
                       t.created_at, t.disbanded_at,
                       COUNT(DISTINCT am.agent_id) AS member_count,
                       COUNT(DISTINCT k.id) AS task_count
                FROM teams t
                LEFT JOIN team_members fm ON fm.team_id = t.id AND fm.agent_id = ?1
                LEFT JOIN team_members am ON am.team_id = t.id
                LEFT JOIN team_tasks k ON k.team_id = t.id
                WHERE t.leader_id = ?1 OR fm.agent_id IS NOT NULL
                GROUP BY t.id
                ORDER BY t.created_at ASC
                "#,
            )
            .map_err(db_err)?;

        let summaries = stmt
            .query_map(params![agent_id], read_summary_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(summaries)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_store() -> SqliteTeamStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteTeamStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    #[tokio::test]
    async fn test_create_and_get_team() {
        let store = setup_store().await;

        let team = store
            .create_team(NewTeam {
                name: "Alpha".into(),
                description: "Test team".into(),
                leader_id: "leader-1".into(),
            })
            .await
            .unwrap();

        assert_eq!(team.name, "Alpha");
        assert_eq!(team.leader_id, "leader-1");
        assert_eq!(team.status, TeamStatus::Active);
        assert!(team.disbanded_at.is_none());

        let fetched = store.get_team(&team.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, team.id);
        assert_eq!(fetched.name, "Alpha");
    }

    #[tokio::test]
    async fn test_list_teams_summary_counts() {
        let store = setup_store().await;

        let team = store
            .create_team(NewTeam {
                name: "Beta".into(),
                description: "".into(),
                leader_id: "agent-1".into(),
            })
            .await
            .unwrap();

        // Add a member
        {
            let conn = store.conn.lock().await;
            let now = now_epoch();
            conn.execute(
                "INSERT INTO team_members (team_id, agent_id, role, joined_at) VALUES (?1, ?2, ?3, ?4)",
                params![team.id, "agent-2", "worker", now],
            )
            .unwrap();
        }

        // Add a task directly via SQL (team_tasks table kept for backward compat)
        {
            let conn = store.conn.lock().await;
            let now = now_epoch();
            conn.execute(
                "INSERT INTO team_tasks (id, team_id, agent_id, subject, status, created_at) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params!["task-1", team.id, "agent-2", "Do something", now],
            )
            .unwrap();
        }

        let summaries = store.list_teams().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].member_count, 1);
        assert_eq!(summaries[0].task_count, 1);
    }

    #[tokio::test]
    async fn test_disband_and_delete_team() {
        let store = setup_store().await;

        let team = store
            .create_team(NewTeam {
                name: "Gamma".into(),
                description: "".into(),
                leader_id: "leader-2".into(),
            })
            .await
            .unwrap();

        // Cannot delete active team
        let err = store.delete_team(&team.id).await;
        assert!(err.is_err());

        // Disband first
        store.disband_team(&team.id).await.unwrap();

        let fetched = store.get_team(&team.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TeamStatus::Disbanded);
        assert!(fetched.disbanded_at.is_some());

        // Now delete should succeed
        store.delete_team(&team.id).await.unwrap();
        assert!(store.get_team(&team.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_get_members() {
        let store = setup_store().await;

        let team = store
            .create_team(NewTeam {
                name: "Delta".into(),
                description: "".into(),
                leader_id: "leader-3".into(),
            })
            .await
            .unwrap();

        {
            let conn = store.conn.lock().await;
            let now = now_epoch();
            conn.execute(
                "INSERT INTO team_members (team_id, agent_id, role, joined_at) VALUES (?1, ?2, ?3, ?4)",
                params![team.id, "agent-a", "worker", now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO team_members (team_id, agent_id, role, joined_at) VALUES (?1, ?2, ?3, ?4)",
                params![team.id, "agent-b", "reviewer", now],
            )
            .unwrap();
        }

        let members = store.get_members(&team.id).await.unwrap();
        assert_eq!(members.len(), 2);
        let ids: Vec<&str> = members.iter().map(|m| m.agent_id.as_str()).collect();
        assert!(ids.contains(&"agent-a"));
        assert!(ids.contains(&"agent-b"));
    }

    #[tokio::test]
    async fn test_get_agent_teams() {
        let store = setup_store().await;

        // Team 1: agent-x is leader
        let t1 = store
            .create_team(NewTeam {
                name: "Team One".into(),
                description: "".into(),
                leader_id: "agent-x".into(),
            })
            .await
            .unwrap();

        // Team 2: agent-x is member
        let t2 = store
            .create_team(NewTeam {
                name: "Team Two".into(),
                description: "".into(),
                leader_id: "agent-y".into(),
            })
            .await
            .unwrap();

        {
            let conn = store.conn.lock().await;
            let now = now_epoch();
            conn.execute(
                "INSERT INTO team_members (team_id, agent_id, role, joined_at) VALUES (?1, ?2, ?3, ?4)",
                params![t2.id, "agent-x", "helper", now],
            )
            .unwrap();
        }

        // Team 3: no relation to agent-x
        store
            .create_team(NewTeam {
                name: "Team Three".into(),
                description: "".into(),
                leader_id: "agent-z".into(),
            })
            .await
            .unwrap();

        let agent_teams = store.get_agent_teams("agent-x").await.unwrap();
        assert_eq!(agent_teams.len(), 2);
        let ids: Vec<&str> = agent_teams.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&t1.id.as_str()));
        assert!(ids.contains(&t2.id.as_str()));
    }
}
