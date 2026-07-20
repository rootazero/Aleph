//! SQLite-backed implementation of [`TeamStore`].
//!
//! Uses `Arc<tokio::sync::Mutex<rusqlite::Connection>>` for thread-safe
//! async access to the team management tables.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::types::{
    NewTeam, NewTeamMember, Team, TeamMember, TeamMemberKind, TeamStatus, TeamSummary,
};
use crate::error::AlephError;

/// Maximum bytes for a team's protocol text. The protocol is rendered
/// verbatim into the leader's system prompt at startup, so an unbounded
/// value would let whoever can call `set_protocol` inject arbitrary
/// instructions into the leader context. 32 KiB is well above any
/// real operating protocol and well below anything a model would read
/// in full anyway.
pub(super) const MAX_PROTOCOL_LEN: usize = 32 * 1024;

// ---------------------------------------------------------------------------
// FromSql implementations — propagate parse errors instead of unwrap_or_default
// ---------------------------------------------------------------------------

impl rusqlite::types::FromSql for TeamStatus {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        s.parse::<Self>().map_err(|e| {
            rusqlite::types::FromSqlError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e,
            )))
        })
    }
}

impl rusqlite::types::FromSql for TeamMemberKind {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        s.parse::<Self>().map_err(|e| {
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

fn not_found(msg: impl Into<String>) -> AlephError {
    AlephError::NotFound(format!("TeamStore: {}", msg.into()))
}

fn domain_err(msg: impl Into<String>) -> AlephError {
    AlephError::Other {
        message: format!("TeamStore: {}", msg.into()),
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

    async fn get_team_by_name(&self, name: &str) -> crate::error::Result<Option<Team>>;

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

    /// Remove a member from an active team. Cannot remove the leader.
    async fn remove_member(&self, team_id: &str, agent_id: &str) -> crate::error::Result<()>;

    /// Return summary records for all teams that an agent belongs to (as leader or member).
    async fn get_agent_teams(&self, agent_id: &str) -> crate::error::Result<Vec<TeamSummary>>;

    /// Set (or clear, with `None`) the team's operating protocol. Errors with
    /// `NotFound` when the team does not exist. The protocol is injected into
    /// every member's launch context by the handoff-context builder.
    async fn set_protocol(
        &self,
        team_id: &str,
        protocol: Option<String>,
    ) -> crate::error::Result<()>;

    /// Rename a team. Errors with `NotFound` when the team does not exist.
    /// Used by both manual rename (`teams.rename`) and first-message auto-name.
    async fn rename_team(&self, id: &str, name: &str) -> crate::error::Result<()>;

    /// Set (or clear) the auto-name flag. Teams created with a blank name set
    /// this to `true` so the first message can replace the provisional name.
    async fn set_name_auto(&self, id: &str, value: bool) -> crate::error::Result<()>;

    /// Atomically check-and-clear the auto-name flag. Returns `true` exactly
    /// once (on the first call when the flag was set), `false` thereafter — so
    /// it doubles as the "first meaningful message" gate, race-safe under
    /// concurrent sends.
    async fn take_auto_name_flag(&self, id: &str) -> crate::error::Result<bool>;
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

    /// Run schema migration — creates the `teams` and `team_members` tables.
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

            CREATE INDEX IF NOT EXISTS idx_team_members_agent ON team_members(agent_id);

            -- Enforce team-name uniqueness at the database layer so concurrent
            -- `create_team` calls with the same name can't both succeed and
            -- leave a first-match-wins shadow row that name-based lookups can
            -- never reach. Only active teams participate — a previously-active
            -- name may legitimately reappear after a disband.
            CREATE UNIQUE INDEX IF NOT EXISTS idx_teams_name_active
                ON teams(name) WHERE status = 'active';
            "#,
        )
        .map_err(db_err)?;

        // Additive migration: ACP-backed team member columns.
        // Older databases without these columns get them backfilled with
        // `kind = 'agent'` (default) so all existing rows continue to route
        // through the in-process registry.
        add_column_if_missing(
            &conn,
            "team_members",
            "kind",
            "TEXT NOT NULL DEFAULT 'agent'",
        )?;
        add_column_if_missing(&conn, "team_members", "acp_harness_id", "TEXT")?;
        add_column_if_missing(&conn, "team_members", "acp_cwd", "TEXT")?;
        add_column_if_missing(&conn, "team_members", "acp_session_name", "TEXT")?;

        // Additive migration: per-team operating protocol (nullable). Older
        // databases backfill NULL = no protocol in effect.
        add_column_if_missing(&conn, "teams", "protocol", "TEXT")?;

        // Additive migration: auto-name flag. Teams created from the Panel
        // compose popover with a blank name carry `name_auto = 1`; the first
        // `teams.chat.send` consumes the flag and replaces the provisional
        // name with an LLM-generated topic. Older rows backfill 0 (no-op).
        add_column_if_missing(&conn, "teams", "name_auto", "INTEGER NOT NULL DEFAULT 0")?;

        Ok(())
    }
}

/// Add a column to a table only if it does not already exist.
///
/// Idempotent on `SQLite` where `ALTER TABLE ... ADD COLUMN` raises on
/// duplicate columns. Inspected via `PRAGMA table_info` to avoid relying
/// on error string matching.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    type_decl: &str,
) -> crate::error::Result<()> {
    let pragma_sql = format!("PRAGMA table_info({})", quote_sql_identifier(table));
    let mut stmt = conn.prepare(&pragma_sql).map_err(db_err)?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_err)?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if !exists {
        let alter_sql = format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            quote_sql_identifier(table),
            quote_sql_identifier(column),
            type_decl
        );
        conn.execute(&alter_sql, []).map_err(db_err)?;
    }
    Ok(())
}

/// Quote an SQLite identifier with double quotes and escape embedded quotes.
///
/// This prevents identifier-based SQL injection when we have to build DDL
/// strings where bind parameters are not allowed (table/column names).
fn quote_sql_identifier(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
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
        // Column 7 (`protocol`) is an additive nullable column; `.ok()`
        // tolerates legacy rows / SELECTs that predate it.
        protocol: row.get::<_, Option<String>>(7).ok().flatten(),
    })
}

fn read_member_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamMember> {
    Ok(TeamMember {
        team_id: row.get(0)?,
        agent_id: row.get(1)?,
        role: row.get(2)?,
        joined_at: row.get(3)?,
        kind: row.get(4).unwrap_or(TeamMemberKind::Agent),
        acp_harness_id: row.get(5).ok(),
        acp_cwd: row.get(6).ok(),
        acp_session_name: row.get(7).ok(),
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
    })
}

/// Broadcast a team lifecycle event on the process-global bus (best-effort).
///
/// Mirrors the coord-task store pattern (`agents/swarm/tasks/store/mod.rs`):
/// `TeamEventLogger` persists these into `team_events` so the kanban drawer
/// renders team lifecycle transitions (creation, membership changes, disband)
/// alongside task events. `GlobalBus` is a singleton — no injection required —
/// and broadcasting with zero subscribers (library tests) is safe.
async fn broadcast_team_event(team_id: &str, event: crate::event::AlephEvent) {
    crate::event::GlobalBus::global()
        .broadcast("team_store", team_id, event)
        .await;
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
        .map_err(|e| match e {
            // Surface a clean duplicate-name error so `team_create` callers can
            // report it without parsing the SQLite message. The
            // `idx_teams_name_active` partial UNIQUE index is the source of
            // this constraint.
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation
                    && err
                        .message
                        .to_ascii_lowercase()
                        .contains("idx_teams_name_active") =>
            {
                crate::error::AlephError::invalid_input(format!(
                    "a team named '{}' already exists",
                    input.name
                ))
            }
            other => db_err(other),
        })?;
        drop(conn);

        // Members (including the leader) enroll via `add_member`, which emits
        // one `TeamMemberAdded` each — so `member_ids` is empty here and the
        // timeline stays complete without double-reporting membership.
        broadcast_team_event(
            &id,
            crate::event::AlephEvent::TeamCreated {
                team_id: id.clone(),
                name: input.name.clone(),
                member_ids: Vec::new(),
            },
        )
        .await;

        Ok(Team {
            id,
            name: input.name,
            description: input.description,
            leader_id: input.leader_id,
            status: TeamStatus::Active,
            created_at: now,
            disbanded_at: None,
            // Protocol is set post-creation via `set_protocol` (keeps `NewTeam`
            // — and its 20+ call-site literals — unchanged).
            protocol: None,
        })
    }

    async fn get_team(&self, id: &str) -> crate::error::Result<Option<Team>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, name, description, leader_id, status, created_at, disbanded_at, protocol FROM teams WHERE id = ?1",
            )
            .map_err(db_err)?;
        stmt.query_row(params![id], read_team_row)
            .optional()
            .map_err(db_err)
    }

    async fn get_team_by_name(&self, name: &str) -> crate::error::Result<Option<Team>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, name, description, leader_id, status, created_at, disbanded_at, protocol FROM teams WHERE name = ?1",
            )
            .map_err(db_err)?;
        stmt.query_row(params![name], read_team_row)
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
                       COUNT(DISTINCT m.agent_id) AS member_count
                FROM teams t
                LEFT JOIN team_members m ON m.team_id = t.id
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
            let exists: bool = conn
                .prepare_cached("SELECT 1 FROM teams WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![id], |_| Ok(true))
                .optional()
                .map_err(db_err)?
                .unwrap_or(false);
            if exists {
                return Err(domain_err(format!("team already disbanded: {id}")));
            }
            return Err(not_found(format!("team not found: {id}")));
        }
        drop(conn);

        broadcast_team_event(
            id,
            crate::event::AlephEvent::TeamDisbanded {
                team_id: id.to_string(),
            },
        )
        .await;
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
            None => return Err(not_found(format!("team not found: {id}"))),
            Some("active") => {
                return Err(domain_err(format!(
                    "team {id} must be disbanded before deletion"
                )))
            }
            Some("disbanded") => {}
            Some(other) => {
                return Err(domain_err(format!(
                    "team {id} has unexpected status '{other}'; cannot delete"
                )))
            }
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
            None => return Err(not_found(format!("team not found: {}", input.team_id))),
            Some("disbanded") => {
                return Err(domain_err(format!(
                    "cannot add member to disbanded team: {}",
                    input.team_id
                )))
            }
            Some("active") => {}
            Some(other) => {
                return Err(domain_err(format!(
                    "team {} has unexpected status '{other}'; cannot add member",
                    input.team_id
                )))
            }
        }

        let now = now_epoch();
        let kind_str = input.kind.as_str();

        let affected = conn
            .execute(
                r#"
            INSERT INTO team_members (
                team_id, agent_id, role, joined_at,
                kind, acp_harness_id, acp_cwd, acp_session_name
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT (team_id, agent_id) DO UPDATE SET
                role = excluded.role,
                kind = excluded.kind,
                acp_harness_id = excluded.acp_harness_id,
                acp_cwd = excluded.acp_cwd,
                acp_session_name = excluded.acp_session_name
            "#,
                params![
                    input.team_id,
                    input.agent_id,
                    input.role,
                    now,
                    kind_str,
                    input.acp_harness_id,
                    input.acp_cwd,
                    input.acp_session_name,
                ],
            )
            .map_err(db_err)?;

        if affected == 0 {
            // Already a member — return the existing record
            let member = conn
                .prepare_cached(
                    "SELECT team_id, agent_id, role, joined_at, kind, acp_harness_id, acp_cwd, acp_session_name FROM team_members WHERE team_id = ?1 AND agent_id = ?2",
                )
                .map_err(db_err)?
                .query_row(params![input.team_id, input.agent_id], read_member_row)
                .map_err(db_err)?;
            return Ok(member);
        }
        drop(conn);

        // Fires on first enrollment and on role/kind re-upsert alike — the
        // timeline records the latest membership shape either way.
        broadcast_team_event(
            &input.team_id,
            crate::event::AlephEvent::TeamMemberAdded {
                team_id: input.team_id.clone(),
                member_id: input.agent_id.clone(),
                role: input.role.clone(),
            },
        )
        .await;

        Ok(TeamMember {
            team_id: input.team_id,
            agent_id: input.agent_id,
            role: input.role,
            joined_at: now,
            kind: input.kind,
            acp_harness_id: input.acp_harness_id,
            acp_cwd: input.acp_cwd,
            acp_session_name: input.acp_session_name,
        })
    }

    async fn get_members(&self, team_id: &str) -> crate::error::Result<Vec<TeamMember>> {
        let conn = self.conn.lock().await;

        let members = conn
            .prepare_cached(
                "SELECT team_id, agent_id, role, joined_at, kind, acp_harness_id, acp_cwd, acp_session_name FROM team_members WHERE team_id = ?1 ORDER BY joined_at ASC",
            )
            .map_err(db_err)?
            .query_map(params![team_id], read_member_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(members)
    }

    async fn remove_member(&self, team_id: &str, agent_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        // Check team exists and is active
        let team = conn
            .prepare_cached(
                "SELECT id, name, description, leader_id, status, created_at, disbanded_at, protocol FROM teams WHERE id = ?1",
            )
            .map_err(db_err)?
            .query_row(params![team_id], read_team_row)
            .optional()
            .map_err(db_err)?;

        let team = match team {
            None => return Err(not_found(format!("team not found: {team_id}"))),
            Some(t) => t,
        };

        if team.status == TeamStatus::Disbanded {
            return Err(domain_err(format!(
                "cannot remove member from disbanded team: {team_id}"
            )));
        }

        if team.leader_id == agent_id {
            return Err(domain_err(format!(
                "cannot remove the team leader ({agent_id})"
            )));
        }

        let affected = conn
            .execute(
                "DELETE FROM team_members WHERE team_id = ?1 AND agent_id = ?2",
                params![team_id, agent_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            return Err(domain_err(format!(
                "agent '{agent_id}' is not a member of team '{team_id}'"
            )));
        }
        drop(conn);

        broadcast_team_event(
            team_id,
            crate::event::AlephEvent::TeamMemberRemoved {
                team_id: team_id.to_string(),
                member_id: agent_id.to_string(),
            },
        )
        .await;

        Ok(())
    }

    async fn get_agent_teams(&self, agent_id: &str) -> crate::error::Result<Vec<TeamSummary>> {
        let conn = self.conn.lock().await;

        // Single query: find teams where agent is leader or member, with counts
        let mut stmt = conn
            .prepare_cached(
                r#"
                SELECT t.id, t.name, t.description, t.leader_id, t.status,
                       t.created_at, t.disbanded_at,
                       COUNT(DISTINCT am.agent_id) AS member_count
                FROM teams t
                LEFT JOIN team_members fm ON fm.team_id = t.id AND fm.agent_id = ?1
                LEFT JOIN team_members am ON am.team_id = t.id
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

    async fn set_protocol(
        &self,
        team_id: &str,
        protocol: Option<String>,
    ) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        // Normalize empty/whitespace-only input to NULL so "clear" and "blank"
        // are the same state — the handoff builder treats both as no protocol.
        let normalized = protocol.and_then(|p| {
            let t = p.trim();
            if t.is_empty() {
                None
            } else if t.len() > MAX_PROTOCOL_LEN {
                // Cap protocol text. The value is interpolated verbatim into
                // the leader's system prompt at render time, so an unbounded
                // string gives whoever can call set_protocol an arbitrary
                // prompt-injection sink against the leader.
                Some(t[..MAX_PROTOCOL_LEN].to_string())
            } else {
                Some(t.to_string())
            }
        });
        let affected = conn
            .execute(
                "UPDATE teams SET protocol = ?1 WHERE id = ?2",
                params![normalized, team_id],
            )
            .map_err(db_err)?;
        if affected == 0 {
            return Err(not_found(format!("team not found: {team_id}")));
        }
        Ok(())
    }

    async fn rename_team(&self, id: &str, name: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        // Only rename active teams — mirrors disband_team / add_member status guard
        let affected = conn
            .execute(
                "UPDATE teams SET name = ?1 WHERE id = ?2 AND status = 'active'",
                params![name, id],
            )
            .map_err(db_err)?;
        if affected == 0 {
            let exists: bool = conn
                .prepare_cached("SELECT 1 FROM teams WHERE id = ?1")
                .map_err(db_err)?
                .query_row(params![id], |_| Ok(true))
                .optional()
                .map_err(db_err)?
                .unwrap_or(false);
            if exists {
                return Err(domain_err(format!("team already disbanded: {id}")));
            }
            return Err(not_found(format!("team not found: {id}")));
        }
        Ok(())
    }

    async fn set_name_auto(&self, id: &str, value: bool) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        let affected = conn
            .execute(
                "UPDATE teams SET name_auto = ?1 WHERE id = ?2",
                params![i64::from(value), id],
            )
            .map_err(db_err)?;
        if affected == 0 {
            return Err(not_found(format!("team not found: {id}")));
        }
        Ok(())
    }

    async fn take_auto_name_flag(&self, id: &str) -> crate::error::Result<bool> {
        let conn = self.conn.lock().await;
        let affected = conn
            .execute(
                "UPDATE teams SET name_auto = 0 WHERE id = ?1 AND name_auto = 1",
                params![id],
            )
            .map_err(db_err)?;
        Ok(affected > 0)
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

    /// `team_create` rejects duplicate names by consulting `get_team_by_name`
    /// before inserting. This locks in the lookup contract that guard relies on:
    /// an existing name resolves to its team, an absent one resolves to None.
    #[tokio::test]
    async fn get_team_by_name_finds_existing_and_none_for_absent() {
        let store = setup_store().await;
        let team = store
            .create_team(NewTeam {
                name: "Dup".into(),
                description: String::new(),
                leader_id: "lead".into(),
            })
            .await
            .unwrap();

        let found = store.get_team_by_name("Dup").await.unwrap();
        assert_eq!(found.map(|t| t.id), Some(team.id));
        assert!(store.get_team_by_name("Absent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_set_protocol_round_trip() {
        let store = setup_store().await;
        let team = store
            .create_team(NewTeam {
                name: "Proto".into(),
                description: "".into(),
                leader_id: "lead".into(),
            })
            .await
            .unwrap();

        // Fresh teams have no protocol.
        assert_eq!(
            store.get_team(&team.id).await.unwrap().unwrap().protocol,
            None
        );

        // Set it.
        store
            .set_protocol(
                &team.id,
                Some("Reviewer owns QA; merge only on green.".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .get_team(&team.id)
                .await
                .unwrap()
                .unwrap()
                .protocol
                .as_deref(),
            Some("Reviewer owns QA; merge only on green.")
        );

        // Whitespace-only normalizes back to None (clear).
        store
            .set_protocol(&team.id, Some("   ".into()))
            .await
            .unwrap();
        assert_eq!(
            store.get_team(&team.id).await.unwrap().unwrap().protocol,
            None
        );

        // Unknown team is a NotFound error.
        assert!(store
            .set_protocol("no-such-team", Some("x".into()))
            .await
            .is_err());
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

        let summaries = store.list_teams().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].member_count, 1);
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

    #[tokio::test]
    async fn test_acp_member_round_trip() {
        let store = setup_store().await;

        let team = store
            .create_team(NewTeam {
                name: "AcpRing".into(),
                description: "".into(),
                leader_id: "leader-a".into(),
            })
            .await
            .unwrap();

        let m = store
            .add_member(NewTeamMember::for_acp_session(
                team.id.clone(),
                "claude_code",
                "/work/proj",
                Some("review-bot".into()),
                "reviewer",
            ))
            .await
            .unwrap();

        assert_eq!(m.kind, TeamMemberKind::AcpSession);
        assert_eq!(m.acp_harness_id.as_deref(), Some("claude_code"));
        assert_eq!(m.acp_cwd.as_deref(), Some("/work/proj"));
        assert_eq!(m.acp_session_name.as_deref(), Some("review-bot"));
        assert_eq!(m.agent_id, "acp:claude_code:/work/proj:review-bot");

        let members = store.get_members(&team.id).await.unwrap();
        let fetched = members
            .iter()
            .find(|x| x.kind == TeamMemberKind::AcpSession)
            .expect("acp member present");
        assert_eq!(fetched.agent_id, m.agent_id);
        assert_eq!(fetched.acp_harness_id, m.acp_harness_id);

        let plain = store
            .add_member(NewTeamMember::for_agent(
                team.id.clone(),
                "leader-a",
                "leader",
            ))
            .await
            .unwrap();
        assert_eq!(plain.kind, TeamMemberKind::Agent);
        assert!(plain.acp_harness_id.is_none());
    }

    #[tokio::test]
    async fn test_remove_member() {
        let store = setup_store().await;

        let team = store
            .create_team(NewTeam {
                name: "RemoveTest".into(),
                description: "".into(),
                leader_id: "leader-r".into(),
            })
            .await
            .unwrap();

        // Add two members
        store
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: "member-1".into(),
                role: "worker".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        store
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: "member-2".into(),
                role: "reviewer".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Can remove a regular member
        store.remove_member(&team.id, "member-1").await.unwrap();
        let members = store.get_members(&team.id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].agent_id, "member-2");

        // Cannot remove the leader
        let err = store.remove_member(&team.id, "leader-r").await;
        assert!(err.is_err());
        assert!(
            format!("{:?}", err.unwrap_err()).contains("leader"),
            "error should mention leader"
        );

        // Cannot remove non-existent member
        let err = store.remove_member(&team.id, "nobody").await;
        assert!(err.is_err());
        assert!(
            format!("{:?}", err.unwrap_err()).contains("not a member"),
            "error should mention not a member"
        );
    }

    #[tokio::test]
    async fn rename_team_updates_name_and_errors_when_absent() {
        let store = setup_store().await;
        let team = store
            .create_team(NewTeam {
                name: "Old".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        store.rename_team(&team.id, "New Topic").await.unwrap();
        let got = store.get_team(&team.id).await.unwrap().unwrap();
        assert_eq!(got.name, "New Topic");

        let err = store.rename_team("nope", "X").await;
        assert!(err.is_err(), "rename of missing team must error");
    }

    #[tokio::test]
    async fn rename_team_rejects_disbanded_team() {
        let store = setup_store().await;
        let team = store
            .create_team(NewTeam {
                name: "Old".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        store.disband_team(&team.id).await.unwrap();

        let err = store.rename_team(&team.id, "New Topic").await;
        assert!(err.is_err(), "rename of disbanded team must error");

        // Name must remain unchanged after a rejected rename.
        let got = store.get_team(&team.id).await.unwrap().unwrap();
        assert_eq!(got.name, "Old");
    }

    #[tokio::test]
    async fn take_auto_name_flag_is_a_one_shot_gate() {
        let store = setup_store().await;
        let team = store
            .create_team(NewTeam {
                name: "新群聊".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        assert!(!store.take_auto_name_flag(&team.id).await.unwrap());
        store.set_name_auto(&team.id, true).await.unwrap();
        assert!(store.take_auto_name_flag(&team.id).await.unwrap());
        assert!(!store.take_auto_name_flag(&team.id).await.unwrap());
    }
}
