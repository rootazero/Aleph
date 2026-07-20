//! Task artifact types and SQLite-backed storage.
//!
//! Artifacts are rich outputs produced by agents during task execution —
//! reports, code snippets, reviews, discoveries, etc.

use crate::sync_primitives::Arc;
use std::any::Any;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::AlephError;

/// Maximum bytes for a single artifact's content body. Artifacts cover
/// plan markdown, shell snippets, extracted doc sections, and reviews —
/// none of which have a legitimate need for multi-MiB payloads. 1 MiB is
/// well above any real artifact and well below anything a model would read.
pub(super) const MAX_ARTIFACT_CONTENT_LEN: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// ArtifactType
// ---------------------------------------------------------------------------

/// The kind of artifact produced by an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Report,
    Code,
    Plan,
    Custom(String),
}

impl ArtifactType {
    /// Canonical string representation for storage.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Report => "report",
            Self::Code => "code",
            Self::Plan => "plan",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Reconstruct from a stored string value.
    #[must_use]
    pub fn from_stored(s: &str) -> Self {
        match s {
            "report" => Self::Report,
            "code" => Self::Code,
            "plan" => Self::Plan,
            other => Self::Custom(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// TaskArtifact
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// TaskStatus
// ---------------------------------------------------------------------------

/// Workflow status for a task artifact.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
    Failed,
}

impl TaskStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn from_stored(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "blocked" => Self::Blocked,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }

    /// Whether self can transition to `new_status`.
    #[must_use]
    pub fn can_transition_to(&self, new_status: &Self) -> bool {
        match (self, new_status) {
            (a, b) if a == b => true,
            (Self::Pending, Self::InProgress) => true,
            (Self::Pending, Self::Blocked) => true,
            (Self::Pending, Self::Completed) => true,
            (Self::InProgress, Self::Completed) => true,
            (Self::InProgress, Self::Failed) => true,
            (Self::Blocked, Self::Pending) => true,
            (Self::Failed, Self::Pending) => true,
            (Self::Completed, _) => false,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// TaskArtifact
// ---------------------------------------------------------------------------

/// A rich output produced by an agent while executing a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub id: String,
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    /// Markdown content body.
    pub content: String,
    /// Workflow status.
    pub status: TaskStatus,
    /// Artifact IDs that must complete before this task can proceed.
    pub blocked_by: Vec<String>,
    /// Assigned agent for this task.
    pub assignee: Option<String>,
    /// Lower value = higher priority.
    pub priority: i32,
    /// Arbitrary structured metadata.
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    /// When the task moved to `InProgress`.
    pub started_at: Option<DateTime<Utc>>,
    /// When the task reached a terminal state.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Input for creating a new artifact (no id or timestamps).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewArtifact {
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
}

const fn default_priority() -> i32 {
    0
}

fn default_metadata() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("ArtifactStore: {e}"),
        suggestion: None,
    }
}

pub(crate) fn read_artifact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskArtifact> {
    let artifact_type_str: String = row.get(3)?;
    let status_str: String = row.get(6)?;
    let blocked_by_str: String = row.get(7)?;
    let assignee: Option<String> = row.get(8)?;
    let priority: i32 = row.get(9)?;
    let metadata_str: String = row.get(10)?;
    let created_at_str: String = row.get(11)?;
    let started_at_str: Option<String> = row.get(12)?;
    let completed_at_str: Option<String> = row.get(13)?;

    let blocked_by = serde_json::from_str(&blocked_by_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid blocked_by JSON: {e}"),
            )),
        )
    })?;

    let metadata = serde_json::from_str(&metadata_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            10,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid metadata JSON: {e}"),
            )),
        )
    })?;

    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let started_at = match started_at_str {
        Some(s) => Some(
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        12,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
        ),
        None => None,
    };

    let completed_at = match completed_at_str {
        Some(s) => Some(
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        13,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
        ),
        None => None,
    };

    Ok(TaskArtifact {
        id: row.get(0)?,
        task_id: row.get(1)?,
        agent_id: row.get(2)?,
        artifact_type: ArtifactType::from_stored(&artifact_type_str),
        title: row.get(4)?,
        content: row.get(5)?,
        status: TaskStatus::from_stored(&status_str),
        blocked_by,
        assignee,
        priority,
        metadata,
        created_at,
        started_at,
        completed_at,
    })
}

// ---------------------------------------------------------------------------
// ArtifactStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for task artifacts.
#[async_trait]
pub trait ArtifactStore: Send + Sync + Any {
    /// Create and persist a new artifact, returning the full record.
    async fn create_artifact(&self, input: NewArtifact) -> crate::error::Result<TaskArtifact>;

    /// Fetch a single artifact by its ID. Returns `None` if not found.
    async fn get_artifact(&self, id: &str) -> crate::error::Result<Option<TaskArtifact>>;

    /// Return all artifacts belonging to a task, ordered by creation time.
    async fn get_artifacts_for_task(
        &self,
        task_id: &str,
    ) -> crate::error::Result<Vec<TaskArtifact>>;

    /// Hard-delete all artifacts (and their dependency rows) belonging to the given tasks.
    /// Returns artifact rows deleted.
    async fn delete_artifacts_for_tasks(&self, task_ids: &[String]) -> crate::error::Result<usize>;
}

// ---------------------------------------------------------------------------
// SqliteArtifactStore
// ---------------------------------------------------------------------------

pub struct SqliteArtifactStore {
    pub(crate) conn: Arc<Mutex<Connection>>,
}

impl SqliteArtifactStore {
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

    /// Run schema migration — creates the `task_artifacts` table, indexes,
    /// and the `task_artifact_dependencies` junction table.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS task_artifacts (
                id            TEXT PRIMARY KEY,
                task_id       TEXT NOT NULL,
                agent_id      TEXT NOT NULL,
                artifact_type TEXT NOT NULL,
                title         TEXT NOT NULL,
                content       TEXT NOT NULL DEFAULT '',
                status        TEXT NOT NULL DEFAULT 'pending',
                blocked_by    TEXT NOT NULL DEFAULT '[]',
                assignee      TEXT,
                priority      INTEGER NOT NULL DEFAULT 0,
                metadata      TEXT NOT NULL DEFAULT '{}',
                created_at    TEXT NOT NULL,
                started_at    TEXT,
                completed_at  TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_task_artifacts_task_id
                ON task_artifacts(task_id);
            CREATE INDEX IF NOT EXISTS idx_task_artifacts_status
                ON task_artifacts(task_id, status);
            CREATE INDEX IF NOT EXISTS idx_task_artifacts_assignee
                ON task_artifacts(assignee);

            -- Junction table for explicit dependency tracking (replaces LIKE-based JSON matching)
            CREATE TABLE IF NOT EXISTS task_artifact_dependencies (
                artifact_id    TEXT NOT NULL,
                depends_on    TEXT NOT NULL,
                PRIMARY KEY (artifact_id, depends_on),
                FOREIGN KEY (artifact_id) REFERENCES task_artifacts(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_tad_depends_on
                ON task_artifact_dependencies(depends_on);
            "#,
        )
        .map_err(db_err)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ArtifactStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ArtifactStore for SqliteArtifactStore {
    async fn create_artifact(&self, input: NewArtifact) -> crate::error::Result<TaskArtifact> {
        // Per-artifact content cap. Artifact bodies can be plan markdown,
        // shell snippets, or extracted doc sections — none of which have a
        // legitimate need for multi-MiB payloads. Without a cap, a single
        // create_artifact with a multi-GB content grows SQLite and is fully
        // materialised into memory on every read for JSON parsing.
        if input.content.len() > MAX_ARTIFACT_CONTENT_LEN {
            return Err(crate::error::AlephError::config(format!(
                "artifact content exceeds {} byte cap (got {})",
                MAX_ARTIFACT_CONTENT_LEN,
                input.content.len()
            )));
        }
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let metadata_str =
            serde_json::to_string(&input.metadata).unwrap_or_else(|_| "{}".to_string());
        let blocked_by_str =
            serde_json::to_string(&input.blocked_by).unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            r#"
            INSERT INTO task_artifacts (id, task_id, agent_id, artifact_type, title, content, status, blocked_by, assignee, priority, metadata, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                id,
                input.task_id,
                input.agent_id,
                input.artifact_type.as_str(),
                input.title,
                input.content,
                input.status.as_str(),
                blocked_by_str,
                input.assignee,
                input.priority,
                metadata_str,
                now_str,
            ],
        )
        .map_err(db_err)?;

        // Mirror blocked_by into the junction table so complete_task can
        // resolve dependents without parsing JSON. kanban::add_dependency
        // populates this for incremental edits; create_artifact must do the
        // same for the bulk-insert path or unblocking is silently broken.
        for dep in &input.blocked_by {
            conn.execute(
                "INSERT OR IGNORE INTO task_artifact_dependencies (artifact_id, depends_on) VALUES (?1, ?2)",
                params![id, dep],
            )
            .map_err(db_err)?;
        }

        Ok(TaskArtifact {
            id,
            task_id: input.task_id,
            agent_id: input.agent_id,
            artifact_type: input.artifact_type,
            title: input.title,
            content: input.content,
            status: input.status,
            blocked_by: input.blocked_by,
            assignee: input.assignee,
            priority: input.priority,
            metadata: input.metadata,
            created_at: now,
            started_at: None,
            completed_at: None,
        })
    }

    async fn get_artifact(&self, id: &str) -> crate::error::Result<Option<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, agent_id, artifact_type, title, content, status, blocked_by, assignee, priority, metadata, created_at, started_at, completed_at \
                 FROM task_artifacts WHERE id = ?1",
            )
            .map_err(db_err)?;

        stmt.query_row(params![id], read_artifact_row)
            .optional()
            .map_err(db_err)
    }

    async fn get_artifacts_for_task(
        &self,
        task_id: &str,
    ) -> crate::error::Result<Vec<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, agent_id, artifact_type, title, content, status, blocked_by, assignee, priority, metadata, created_at, started_at, completed_at \
                 FROM task_artifacts WHERE task_id = ?1 ORDER BY created_at ASC",
            )
            .map_err(db_err)?;

        let artifacts = stmt
            .query_map(params![task_id], read_artifact_row)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;

        Ok(artifacts)
    }

    async fn delete_artifacts_for_tasks(&self, task_ids: &[String]) -> crate::error::Result<usize> {
        if task_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().await;
        let placeholders = task_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let params: Vec<&dyn rusqlite::types::ToSql> = task_ids
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        // Delete dependency rows for artifacts belonging to the given tasks first.
        // (The FK has ON DELETE CASCADE, but pragma foreign_keys may not be set;
        // deleting explicitly is always safe.)
        let sql_deps = {
            let mut s = String::from(
                "DELETE FROM task_artifact_dependencies WHERE artifact_id IN \
                 (SELECT id FROM task_artifacts WHERE task_id IN (",
            );
            s.push_str(&placeholders);
            s.push_str("))");
            s
        };
        conn.execute(&sql_deps, params.as_slice()).map_err(db_err)?;
        let sql_art = {
            let mut s = String::from("DELETE FROM task_artifacts WHERE task_id IN (");
            s.push_str(&placeholders);
            s.push(')');
            s
        };
        let n = conn.execute(&sql_art, params.as_slice()).map_err(db_err)?;
        Ok(n)
    }
}

impl SqliteArtifactStore {
    pub async fn update_status(
        &self,
        artifact_id: &str,
        new_status: TaskStatus,
    ) -> crate::error::Result<TaskArtifact> {
        let conn = self.conn.lock().await;
        let now = Utc::now();

        let (started_at, completed_at) = match new_status {
            TaskStatus::InProgress => (Some(now.to_rfc3339()), None),
            TaskStatus::Completed | TaskStatus::Failed => {
                // Preserve existing started_at — do not overwrite it on completion/failure.
                let existing: Option<String> = conn
                    .query_row(
                        "SELECT started_at FROM task_artifacts WHERE id = ?1",
                        params![artifact_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db_err)?
                    .flatten();
                (existing, Some(now.to_rfc3339()))
            }
            _ => (None, None),
        };

        conn.execute(
            "UPDATE task_artifacts SET status = ?1, started_at = ?2, completed_at = ?3 WHERE id = ?4",
            params![new_status.as_str(), started_at, completed_at, artifact_id],
        )
        .map_err(db_err)?;

        drop(conn);
        self.get_artifact(artifact_id)
            .await
            .and_then(|opt| opt.ok_or_else(|| db_err("artifact not found after update")))
    }

    /// Check whether all dependencies of a task are completed.
    ///
    /// `blocked_by` is the JSON array stored in the `blocked_by` column.
    /// Returns `true` if the array is empty or if all artifact IDs in it
    /// have `status == 'completed'`.
    async fn all_dependencies_completed(
        &self,
        blocked_by: &[String],
    ) -> crate::error::Result<bool> {
        if blocked_by.is_empty() {
            return Ok(true);
        }

        let conn = self.conn.lock().await;
        let placeholders = blocked_by.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let params_vec: Vec<&dyn rusqlite::types::ToSql> = blocked_by
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        // A non-existent dependency must not satisfy the block — require that
        // every referenced artifact exists and is completed.
        let existing_count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM task_artifacts WHERE id IN ({placeholders})"),
                params_vec.as_slice(),
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if existing_count != blocked_by.len() as i64 {
            return Ok(false);
        }

        let sql = format!(
            "SELECT COUNT(*) FROM task_artifacts WHERE id IN ({placeholders}) AND status != 'completed'"
        );
        let incomplete_count: i64 = conn
            .query_row(&sql, params_vec.as_slice(), |row| row.get(0))
            .map_err(db_err)?;
        Ok(incomplete_count == 0)
    }

    pub async fn complete_task(
        &self,
        artifact_id: &str,
    ) -> crate::error::Result<Vec<TaskArtifact>> {
        self.update_status(artifact_id, TaskStatus::Completed)
            .await?;

        let blocked_ids: Vec<String> = {
            let conn = self.conn.lock().await;
            let mut stmt = conn
                .prepare_cached(
                    "SELECT artifact_id FROM task_artifact_dependencies WHERE depends_on = ?1",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![artifact_id], |row| row.get(0))
                .map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<String>>>()
                .map_err(db_err)?
        };

        let mut unblocked = Vec::new();
        for blocked_id in blocked_ids {
            // Look up this dependent's own dependencies, then check whether
            // they are *all* completed. The previous implementation passed
            // `blocked_id` itself to `all_dependencies_completed`, which
            // checks artifact status — i.e. it asked "is the blocked
            // artifact completed?" instead of "are its dependencies?". The
            // answer was always false for an item still in Blocked state,
            // so unblocking never fired.
            let deps: Vec<String> = {
                let conn = self.conn.lock().await;
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT depends_on FROM task_artifact_dependencies WHERE artifact_id = ?1",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(params![&blocked_id], |row| row.get(0))
                    .map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<String>>>()
                    .map_err(db_err)?
            };

            if self.all_dependencies_completed(&deps).await? {
                let task = self.update_status(&blocked_id, TaskStatus::Pending).await?;
                unblocked.push(task);
            }
        }

        Ok(unblocked)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_read_artifact() {
        let store = SqliteArtifactStore::new_in_memory().await;

        let artifact = store
            .create_artifact(NewArtifact {
                task_id: "task-1".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Report,
                title: "Status Report".into(),
                content: "# Summary\n\nAll good.".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({"priority": "high"}),
            })
            .await
            .unwrap();

        assert_eq!(artifact.task_id, "task-1");
        assert_eq!(artifact.agent_id, "agent-1");
        assert_eq!(artifact.artifact_type, ArtifactType::Report);
        assert_eq!(artifact.title, "Status Report");
        assert_eq!(artifact.content, "# Summary\n\nAll good.");
        assert_eq!(artifact.metadata["priority"], "high");
        assert_eq!(artifact.status, TaskStatus::Pending);
        assert!(artifact.started_at.is_none());
        assert!(artifact.completed_at.is_none());
        assert!(!artifact.id.is_empty());

        let fetched = store.get_artifact(&artifact.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, artifact.id);
        assert_eq!(fetched.title, "Status Report");
        assert_eq!(fetched.artifact_type, ArtifactType::Report);
        assert_eq!(fetched.metadata["priority"], "high");
        assert_eq!(fetched.status, TaskStatus::Pending);

        let task_artifacts = store.get_artifacts_for_task("task-1").await.unwrap();
        assert_eq!(task_artifacts.len(), 1);
        assert_eq!(task_artifacts[0].id, artifact.id);

        assert!(store.get_artifact("no-such-id").await.unwrap().is_none());
        assert!(store
            .get_artifacts_for_task("no-such-task")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_custom_artifact_type() {
        let store = SqliteArtifactStore::new_in_memory().await;

        let artifact = store
            .create_artifact(NewArtifact {
                task_id: "task-2".into(),
                agent_id: "agent-2".into(),
                artifact_type: ArtifactType::Custom("analysis".into()),
                title: "Deep Analysis".into(),
                content: "Detailed findings.".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        assert_eq!(
            artifact.artifact_type,
            ArtifactType::Custom("analysis".into())
        );

        let fetched = store.get_artifact(&artifact.id).await.unwrap().unwrap();
        assert_eq!(
            fetched.artifact_type,
            ArtifactType::Custom("analysis".into())
        );
        assert_eq!(fetched.title, "Deep Analysis");
    }

    #[tokio::test]
    async fn test_status_transitions() {
        let store = SqliteArtifactStore::new_in_memory().await;

        let artifact = store
            .create_artifact(NewArtifact {
                task_id: "task-3".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Plan,
                title: "Plan".into(),
                content: "Details".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: Some("agent-2".into()),
                priority: 1,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        assert_eq!(artifact.status, TaskStatus::Pending);
        assert!(artifact.started_at.is_none());

        let updated = store
            .update_status(&artifact.id, TaskStatus::InProgress)
            .await
            .unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert!(updated.started_at.is_some());

        let completed = store
            .update_status(&artifact.id, TaskStatus::Completed)
            .await
            .unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
        assert!(completed.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_task_status_can_transition() {
        assert!(TaskStatus::Pending.can_transition_to(&TaskStatus::InProgress));
        assert!(TaskStatus::Pending.can_transition_to(&TaskStatus::Blocked));
        assert!(TaskStatus::InProgress.can_transition_to(&TaskStatus::Completed));
        assert!(TaskStatus::Blocked.can_transition_to(&TaskStatus::Pending));
        assert!(TaskStatus::Failed.can_transition_to(&TaskStatus::Pending));
        assert!(!TaskStatus::Completed.can_transition_to(&TaskStatus::Pending));
        assert!(TaskStatus::Pending.can_transition_to(&TaskStatus::Pending));
    }

    #[tokio::test]
    async fn test_all_dependencies_completed_empty() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let artifact = store
            .create_artifact(NewArtifact {
                task_id: "t".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "T".into(),
                content: "C".into(),
                status: TaskStatus::Blocked,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(store.all_dependencies_completed(&[]).await.unwrap());
        assert!(store
            .all_dependencies_completed(&artifact.blocked_by)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_all_dependencies_completed_with_deps() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let dep = store
            .create_artifact(NewArtifact {
                task_id: "t1".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "Dep".into(),
                content: "C".into(),
                status: TaskStatus::Completed,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let artifact = store
            .create_artifact(NewArtifact {
                task_id: "t2".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "Task".into(),
                content: "C".into(),
                status: TaskStatus::Blocked,
                blocked_by: vec![dep.id.clone()],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(store
            .all_dependencies_completed(&artifact.blocked_by)
            .await
            .unwrap());
        store
            .update_status(&dep.id, TaskStatus::Pending)
            .await
            .unwrap();
        assert!(!store
            .all_dependencies_completed(&artifact.blocked_by)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_complete_task_no_dependents() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let artifact = store
            .create_artifact(NewArtifact {
                task_id: "t".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "T".into(),
                content: "C".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let unblocked = store.complete_task(&artifact.id).await.unwrap();
        assert!(unblocked.is_empty());
        let fetched = store.get_artifact(&artifact.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn test_complete_task_unblocks_dependent() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let dep = store
            .create_artifact(NewArtifact {
                task_id: "t1".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "Dep".into(),
                content: "C".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let blocked = store
            .create_artifact(NewArtifact {
                task_id: "t2".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "Blocked".into(),
                content: "C".into(),
                status: TaskStatus::Blocked,
                blocked_by: vec![dep.id.clone()],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let unblocked = store.complete_task(&dep.id).await.unwrap();
        assert_eq!(unblocked.len(), 1);
        assert_eq!(unblocked[0].id, blocked.id);
        let fetched = store.get_artifact(&blocked.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_complete_task_does_not_unblock_partially_blocked() {
        let store = SqliteArtifactStore::new_in_memory().await;
        let dep1 = store
            .create_artifact(NewArtifact {
                task_id: "t1".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "Dep1".into(),
                content: "C".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let dep2 = store
            .create_artifact(NewArtifact {
                task_id: "t2".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "Dep2".into(),
                content: "C".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let blocked = store
            .create_artifact(NewArtifact {
                task_id: "t3".into(),
                agent_id: "a".into(),
                artifact_type: ArtifactType::Custom("task".into()),
                title: "Blocked".into(),
                content: "C".into(),
                status: TaskStatus::Blocked,
                blocked_by: vec![dep1.id.clone(), dep2.id.clone()],
                assignee: None,
                priority: 0,
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let unblocked = store.complete_task(&dep1.id).await.unwrap();
        assert!(unblocked.is_empty());
        let fetched = store.get_artifact(&blocked.id).await.unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Blocked);
        store
            .update_status(&dep1.id, TaskStatus::Completed)
            .await
            .unwrap();
        let unblocked = store.complete_task(&dep2.id).await.unwrap();
        assert_eq!(unblocked.len(), 1);
        assert_eq!(unblocked[0].id, blocked.id);
    }

    #[tokio::test]
    async fn delete_artifacts_for_tasks_removes_only_listed() {
        let store = SqliteArtifactStore::new_in_memory().await;

        let a = store
            .create_artifact(NewArtifact {
                task_id: "task-1".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Report,
                title: "Artifact 1".into(),
                content: "content".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        store
            .create_artifact(NewArtifact {
                task_id: "task-2".into(),
                agent_id: "agent-1".into(),
                artifact_type: ArtifactType::Report,
                title: "Artifact 2".into(),
                content: "content".into(),
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        let n = store
            .delete_artifacts_for_tasks(&["task-1".to_string()])
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert!(store.get_artifact(&a.id).await.unwrap().is_none());
        assert_eq!(
            store.get_artifacts_for_task("task-2").await.unwrap().len(),
            1
        );
    }
}
