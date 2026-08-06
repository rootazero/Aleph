//! SQLite-backed snapshot persistence.

use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::{db_err, SnapshotMeta, TeamSnapshotPayload};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;

/// SQLite-backed snapshot persistence. Shares the connection with
/// [`crate::agents::swarm::tasks::SqliteCoordTaskStore`]; both operate on
/// `coord_team_snapshots` rows in the same database file.
pub struct SqliteSnapshotStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSnapshotStore {
    /// Wrap an existing tokio-mutexed connection.
    ///
    /// This deliberately mirrors `SqliteCoordTaskStore::new(Connection)` so
    /// callers that already hold the coord-task connection can build one in
    /// the same boot path. The schema is created via the coord-task store's
    /// `migrate` (which now adds `coord_team_snapshots`).
    pub const fn new_from_shared(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Convenience for tests — opens an in-memory db, runs the coord-task
    /// migration (which creates the snapshot table), and returns a store.
    #[cfg(test)]
    pub async fn new_in_memory() -> Arc<Self> {
        use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;
        // rust-doctor-disable-next-line unwrap-in-production
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let coord_store = SqliteCoordTaskStore::new(conn);
        // rust-doctor-disable-next-line unwrap-in-production
        coord_store.migrate().await.expect("migrate");
        Arc::new(Self::new_from_shared(coord_store.connection_handle()))
    }

    /// Persist a snapshot row. Returns the assigned id.
    pub async fn insert(
        &self,
        team_id: &str,
        tag: &str,
        payload: &TeamSnapshotPayload,
    ) -> Result<(String, i64, usize)> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let body = serde_json::to_string(payload).map_err(|e| AlephError::ConfigError {
            message: format!("snapshot serialize failed: {e}"),
            suggestion: None,
        })?;
        let size = body.len();

        let conn = self.conn.lock().await;
        conn.execute(
            r#"
            INSERT INTO coord_team_snapshots (id, team_id, tag, created_at, size_bytes, payload)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![id, team_id, tag, now, size as i64, body],
        )
        .map_err(db_err)?;
        Ok((id, now, size))
    }

    /// List snapshots for a team (or all teams when `team_id` is None),
    /// newest first.
    pub async fn list(&self, team_id: Option<&str>) -> Result<Vec<SnapshotMeta>> {
        let conn = self.conn.lock().await;
        let rows: Result<Vec<SnapshotMeta>> = match team_id {
            Some(tid) => conn
                .prepare_cached(
                    r#"
                    SELECT id, team_id, tag, created_at, size_bytes
                    FROM coord_team_snapshots
                    WHERE team_id = ?1
                    ORDER BY created_at DESC
                    "#,
                )
                .map_err(db_err)?
                .query_map(params![tid], read_meta)
                .map_err(db_err)?
                .map(|r| r.map_err(db_err))
                .collect(),
            None => conn
                .prepare_cached(
                    r#"
                    SELECT id, team_id, tag, created_at, size_bytes
                    FROM coord_team_snapshots
                    ORDER BY created_at DESC
                    "#,
                )
                .map_err(db_err)?
                .query_map([], read_meta)
                .map_err(db_err)?
                .map(|r| r.map_err(db_err))
                .collect(),
        };
        rows
    }

    /// Load the full snapshot payload by id.
    pub async fn get(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<(SnapshotMeta, TeamSnapshotPayload)>> {
        let conn = self.conn.lock().await;
        let row: Option<(SnapshotMeta, String)> = conn
            .prepare_cached(
                r#"
                SELECT id, team_id, tag, created_at, size_bytes, payload
                FROM coord_team_snapshots WHERE id = ?1
                "#,
            )
            .map_err(db_err)?
            .query_row(params![snapshot_id], |r| {
                Ok((
                    SnapshotMeta {
                        id: r.get(0)?,
                        team_id: r.get(1)?,
                        tag: r.get(2)?,
                        created_at: r.get(3)?,
                        size_bytes: r.get(4)?,
                    },
                    r.get::<_, String>(5)?,
                ))
            })
            .optional()
            .map_err(db_err)?;

        let Some((meta, body)) = row else {
            return Ok(None);
        };
        let payload: TeamSnapshotPayload =
            serde_json::from_str(&body).map_err(|e| AlephError::ConfigError {
                message: format!("snapshot deserialize failed: {e}"),
                suggestion: None,
            })?;
        Ok(Some((meta, payload)))
    }

    /// Delete a snapshot by id. Idempotent — returns Ok even if not present.
    pub async fn delete(&self, snapshot_id: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let affected = conn
            .execute(
                "DELETE FROM coord_team_snapshots WHERE id = ?1",
                params![snapshot_id],
            )
            .map_err(db_err)?;
        Ok(affected > 0)
    }

    /// Hard-delete all snapshots for a team. Returns rows deleted.
    pub async fn delete_team_snapshots(&self, team_id: &str) -> Result<usize> {
        let conn = self.conn.lock().await;
        let n = conn
            .execute(
                "DELETE FROM coord_team_snapshots WHERE team_id = ?1",
                params![team_id],
            )
            .map_err(db_err)?;
        Ok(n)
    }
}

fn read_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotMeta> {
    Ok(SnapshotMeta {
        id: row.get(0)?,
        team_id: row.get(1)?,
        tag: row.get(2)?,
        created_at: row.get(3)?,
        size_bytes: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::snapshots::TeamSnapshotPayload;
    use crate::teams::types::{Team, TeamStatus};

    fn minimal_payload(team_id: &str) -> TeamSnapshotPayload {
        TeamSnapshotPayload {
            team: Team {
                id: team_id.into(),
                name: "test".into(),
                description: String::new(),
                leader_id: "leader".into(),
                status: TeamStatus::Active,
                created_at: 0,
                disbanded_at: None,
                protocol: None,
                owner_user_id: None,
            },
            members: vec![],
            tasks: vec![],
            note: String::new(),
        }
    }

    #[tokio::test]
    async fn delete_team_snapshots_removes_team_rows() {
        let store = SqliteSnapshotStore::new_in_memory().await;
        // insert inserts a new row each call (not upsert), so two different
        // team_ids → 2 rows total; delete_team_snapshots("team-A") hits 1 row.
        store
            .insert("team-A", "v1", &minimal_payload("team-A"))
            .await
            .unwrap();
        store
            .insert("team-B", "v1", &minimal_payload("team-B"))
            .await
            .unwrap();
        let n = store.delete_team_snapshots("team-A").await.unwrap();
        assert_eq!(n, 1);
        // team-B row is untouched.
        let remaining = store.list(None).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].team_id, "team-B");
    }
}
