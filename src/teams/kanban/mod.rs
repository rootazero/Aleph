pub mod unblocker;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::TaskStatus;
#[cfg(test)]
use crate::teams::artifacts::SqliteArtifactStore;
use crate::teams::artifacts::TaskArtifact;

fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("Kanban: {e}"),
        suggestion: None,
    }
}

fn read_artifact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskArtifact> {
    let artifact_type_str: String = row.get(3)?;
    let status_str: String = row.get(6)?;
    let blocked_by_str: String = row.get(7)?;
    let assignee: Option<String> = row.get(8)?;
    let priority: i32 = row.get(9)?;
    let metadata_str: String = row.get(10)?;
    let created_at_str: String = row.get(11)?;
    let started_at_str: Option<String> = row.get(12)?;
    let completed_at_str: Option<String> = row.get(13)?;

    let default_metadata =
        serde_json::Value::Object(serde_json::Map::new());

    Ok(TaskArtifact {
        id: row.get(0)?,
        task_id: row.get(1)?,
        agent_id: row.get(2)?,
        artifact_type: crate::teams::artifacts::ArtifactType::from_stored(&artifact_type_str),
        title: row.get(4)?,
        content: row.get(5)?,
        status: TaskStatus::from_stored(&status_str),
        blocked_by: serde_json::from_str(&blocked_by_str).unwrap_or_default(),
        assignee,
        priority,
        metadata: serde_json::from_str(&metadata_str)
            .unwrap_or_else(|_| default_metadata.clone()),
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        started_at: started_at_str
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
        completed_at: completed_at_str
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KanbanColumns {
    pub pending: Vec<TaskArtifact>,
    pub in_progress: Vec<TaskArtifact>,
    pub completed: Vec<TaskArtifact>,
    pub blocked: Vec<TaskArtifact>,
    pub failed: Vec<TaskArtifact>,
}

impl KanbanColumns {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
            && self.in_progress.is_empty()
            && self.completed.is_empty()
            && self.blocked.is_empty()
            && self.failed.is_empty()
    }

    pub fn total(&self) -> usize {
        self.pending.len()
            + self.in_progress.len()
            + self.completed.len()
            + self.blocked.len()
            + self.failed.len()
    }
}

#[async_trait]
pub trait KanbanBoard: Send + Sync {
    async fn get_board(&self, team_id: &str) -> Result<KanbanColumns>;

    async fn move_task(
        &self,
        artifact_id: &str,
        new_status: TaskStatus,
    ) -> Result<TaskArtifact>;

    async fn complete_task(&self, artifact_id: &str) -> Result<Vec<TaskArtifact>>;

    async fn add_dependency(&self, artifact_id: &str, depends_on: &str) -> Result<()>;

    async fn get_agent_tasks(&self, agent_id: &str) -> Result<Vec<TaskArtifact>>;
}

pub struct SqliteKanbanBoard {
    conn: Arc<tokio::sync::Mutex<rusqlite::Connection>>,
}

impl SqliteKanbanBoard {
    pub fn new(conn: Arc<tokio::sync::Mutex<rusqlite::Connection>>) -> Self {
        Self { conn }
    }

    #[cfg(test)]
    pub async fn from_artifact_store(store: &SqliteArtifactStore) -> Self {
        Self { conn: store.conn.clone() }
    }

    async fn get_artifact_by_id(&self, artifact_id: &str) -> Result<Option<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, agent_id, artifact_type, title, content, status, blocked_by, assignee, priority, metadata, created_at, started_at, completed_at \
                 FROM task_artifacts WHERE id = ?1",
            )
            .map_err(db_err)?;

        stmt.query_row(params![artifact_id], read_artifact_row)
            .optional()
            .map_err(db_err)
    }

}

#[async_trait]
impl KanbanBoard for SqliteKanbanBoard {
    async fn get_board(&self, team_id: &str) -> Result<KanbanColumns> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, agent_id, artifact_type, title, content, status, blocked_by, assignee, priority, metadata, created_at, started_at, completed_at \
                 FROM task_artifacts WHERE task_id = ?1 ORDER BY priority ASC, created_at DESC",
            )
            .map_err(db_err)?;

        let artifacts: Vec<TaskArtifact> = stmt
            .query_map(params![team_id], read_artifact_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        let mut columns = KanbanColumns::default();
        for artifact in artifacts {
            match artifact.status {
                TaskStatus::Pending => columns.pending.push(artifact),
                TaskStatus::InProgress => columns.in_progress.push(artifact),
                TaskStatus::Completed => columns.completed.push(artifact),
                TaskStatus::Blocked => columns.blocked.push(artifact),
                TaskStatus::Failed => columns.failed.push(artifact),
            }
        }

        Ok(columns)
    }

    async fn move_task(
        &self,
        artifact_id: &str,
        new_status: TaskStatus,
    ) -> Result<TaskArtifact> {
        let conn = self.conn.lock().await;
        let now = Utc::now();

        let (started_at, completed_at) = match new_status {
            TaskStatus::InProgress => (Some(now.to_rfc3339()), None),
            TaskStatus::Completed | TaskStatus::Failed => (None, Some(now.to_rfc3339())),
            _ => (None, None),
        };

        conn.execute(
            "UPDATE task_artifacts SET status = ?1, started_at = ?2, completed_at = ?3 WHERE id = ?4",
            params![new_status.as_str(), started_at, completed_at, artifact_id],
        )
        .map_err(db_err)?;

        drop(conn);
        self.get_artifact_by_id(artifact_id)
            .await?
            .ok_or_else(|| db_err("artifact not found after move"))
    }

    async fn complete_task(&self, artifact_id: &str) -> Result<Vec<TaskArtifact>> {
        self.move_task(artifact_id, TaskStatus::Completed).await?;

        let blocked_ids: Vec<String> = {
            let conn = self.conn.lock().await;
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, task_id, agent_id, artifact_type, title, content, status, blocked_by, assignee, priority, metadata, created_at, started_at, completed_at \
                     FROM task_artifacts WHERE blocked_by LIKE ?1",
                )
                .map_err(db_err)?;

            let pattern = format!("%{}%", artifact_id);
            let rows = match stmt.query_map(params![pattern], |row| {
                let id: String = row.get(0)?;
                let blocked_by_str: String = row.get(7)?;
                let blocked_by: Vec<String> =
                    serde_json::from_str(&blocked_by_str).unwrap_or_default();
                if blocked_by.contains(&artifact_id.to_string()) {
                    Ok(Some(id))
                } else {
                    Ok(None)
                }
            }) {
                Ok(r) => r,
                Err(e) => return Err(db_err(e)),
            };
            let mut ids = Vec::new();
            for row in rows {
                match row {
                    Ok(Some(id)) => ids.push(id),
                    Ok(None) => {}
                    Err(e) => return Err(db_err(e)),
                }
            }
            ids
        };

        let mut unblocked = vec![];
        for blocked_id in blocked_ids {
            // Inline all_dependencies_completed: check if this artifact has no remaining incomplete deps
            let should_unblock = {
                let conn = self.conn.lock().await;
                let blocked_by_str: String = conn
                    .query_row(
                        "SELECT blocked_by FROM task_artifacts WHERE id = ?1",
                        [&blocked_id],
                        |row| row.get(0),
                    )
                    .map_err(db_err)?;
                let blocked_by: Vec<String> =
                    serde_json::from_str(&blocked_by_str).unwrap_or_default();
                if blocked_by.is_empty() {
                    true
                } else {
                    let placeholders = blocked_by.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                    let sql = format!(
                        "SELECT COUNT(*) FROM task_artifacts WHERE id IN ({}) AND status != 'completed'",
                        placeholders
                    );
                    let params_vec: Vec<&dyn rusqlite::types::ToSql> =
                        blocked_by.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
                    let incomplete_count: i64 = conn
                        .query_row(&sql, params_vec.as_slice(), |row| row.get(0))
                        .map_err(db_err)?;
                    incomplete_count == 0
                }
            };
            if should_unblock {
                let task = self.move_task(&blocked_id, TaskStatus::Pending).await?;
                unblocked.push(task);
            }
        }

        Ok(unblocked)
    }

    async fn add_dependency(&self, artifact_id: &str, depends_on: &str) -> Result<()> {
        let conn = self.conn.lock().await;

        let blocked_by_str: String = conn
            .query_row(
                "SELECT blocked_by FROM task_artifacts WHERE id = ?1",
                [artifact_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let mut blocked_by: Vec<String> =
            serde_json::from_str(&blocked_by_str).unwrap_or_default();
        if !blocked_by.contains(&depends_on.to_string()) {
            blocked_by.push(depends_on.to_string());
        }

        let new_blocked_by =
            serde_json::to_string(&blocked_by).unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            "UPDATE task_artifacts SET blocked_by = ?1, status = 'blocked' WHERE id = ?2",
            params![new_blocked_by, artifact_id],
        )
        .map_err(db_err)?;

        Ok(())
    }

    async fn get_agent_tasks(&self, agent_id: &str) -> Result<Vec<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, agent_id, artifact_type, title, content, status, blocked_by, assignee, priority, metadata, created_at, started_at, completed_at \
                 FROM task_artifacts WHERE assignee = ?1 OR agent_id = ?1 ORDER BY priority ASC",
            )
            .map_err(db_err)?;

        let tasks = stmt
            .query_map(params![agent_id], read_artifact_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact, SqliteArtifactStore};

    #[tokio::test]
    async fn test_kanban_board_grouping() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let board = SqliteKanbanBoard::from_artifact_store(&store).await;

        store
            .create_artifact(NewArtifact {
                task_id: "team-1".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Plan,
                title: "Plan A".into(),
                content: "Plan content".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: Some("agent-2".into()),
                priority: 1,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        store
            .create_artifact(NewArtifact {
                task_id: "team-1".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Report,
                title: "Report B".into(),
                content: "Report content".into(),
                status: TaskStatus::Completed,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        let columns = board.get_board("team-1").await.unwrap();
        assert_eq!(columns.pending.len(), 1);
        assert_eq!(columns.completed.len(), 1);
        assert_eq!(columns.total(), 2);
    }

    #[tokio::test]
    async fn test_complete_unblocks_dependencies() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let board = SqliteKanbanBoard::from_artifact_store(&store).await;

        let dep = store
            .create_artifact(NewArtifact {
                task_id: "team-2".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Report,
                title: "Dependency".into(),
                content: "Must complete first".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        let blocked = store
            .create_artifact(NewArtifact {
                task_id: "team-2".into(),
                agent_id: "agent-2".into(),
                artifact_type: ArtifactType::Plan,
                title: "Blocked Plan".into(),
                content: "Waiting on dependency".into(),
                status: TaskStatus::Blocked,
                blocked_by: vec![dep.id.clone()],
                assignee: Some("agent-2".into()),
                priority: 2,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        assert_eq!(blocked.status, TaskStatus::Blocked);

        let unblocked = board.complete_task(&dep.id).await.unwrap();
        assert_eq!(unblocked.len(), 1);
        assert_eq!(unblocked[0].id, blocked.id);
        assert_eq!(unblocked[0].status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_add_dependency_sets_blocked() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let board = SqliteKanbanBoard::from_artifact_store(&store).await;

        let task = store
            .create_artifact(NewArtifact {
                task_id: "team-3".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Plan,
                title: "Original".into(),
                content: "Original task".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        let dep = store
            .create_artifact(NewArtifact {
                task_id: "team-3".into(),
                agent_id: "agent-2".into(),
                artifact_type: ArtifactType::Report,
                title: "New dependency".into(),
                content: "New dependency".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        board.add_dependency(&task.id, &dep.id).await.unwrap();

        let updated = store.get_artifact(&task.id).await.unwrap().unwrap();
        assert_eq!(updated.status, TaskStatus::Blocked);
        assert!(updated.blocked_by.contains(&dep.id));
    }

    #[tokio::test]
    async fn test_get_agent_tasks() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let board = SqliteKanbanBoard::from_artifact_store(&store).await;

        store
            .create_artifact(NewArtifact {
                task_id: "team-4".into(),
                agent_id: "agent-x".into(),
                artifact_type: ArtifactType::Report,
                title: "Task 1".into(),
                content: "Content".into(),
                status: TaskStatus::InProgress,
                blocked_by: vec![],
                assignee: Some("agent-y".into()),
                priority: 1,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        store
            .create_artifact(NewArtifact {
                task_id: "team-4".into(),
                agent_id: "agent-y".into(),
                artifact_type: ArtifactType::Plan,
                title: "Task 2".into(),
                content: "Content".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: Some("agent-y".into()),
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        let tasks = board.get_agent_tasks("agent-y").await.unwrap();
        assert_eq!(tasks.len(), 2);
    }
}
