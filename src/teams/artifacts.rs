//! Task artifact types and SQLite-backed storage.
//!
//! Artifacts are rich outputs produced by agents during task execution —
//! reports, code snippets, reviews, discoveries, etc.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::AlephError;

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
    pub fn as_str(&self) -> &str {
        match self {
            Self::Report => "report",
            Self::Code => "code",
            Self::Plan => "plan",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Reconstruct from a stored string value.
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
    /// Arbitrary structured metadata.
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Input for creating a new artifact (no id or timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewArtifact {
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
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

fn read_artifact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskArtifact> {
    let artifact_type_str: String = row.get(3)?;
    let metadata_str: String = row.get(6)?;
    let created_at_str: String = row.get(7)?;

    Ok(TaskArtifact {
        id: row.get(0)?,
        task_id: row.get(1)?,
        agent_id: row.get(2)?,
        artifact_type: ArtifactType::from_stored(&artifact_type_str),
        title: row.get(4)?,
        content: row.get(5)?,
        metadata: serde_json::from_str(&metadata_str).unwrap_or_else(|_| default_metadata()),
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

// ---------------------------------------------------------------------------
// ArtifactStore trait
// ---------------------------------------------------------------------------

/// Async persistence interface for task artifacts.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Create and persist a new artifact, returning the full record.
    async fn create_artifact(&self, input: NewArtifact) -> crate::error::Result<TaskArtifact>;

    /// Fetch a single artifact by its ID. Returns `None` if not found.
    async fn get_artifact(&self, id: &str) -> crate::error::Result<Option<TaskArtifact>>;

    /// Return all artifacts belonging to a task, ordered by creation time.
    async fn get_artifacts_for_task(
        &self,
        task_id: &str,
    ) -> crate::error::Result<Vec<TaskArtifact>>;
}

// ---------------------------------------------------------------------------
// SqliteArtifactStore
// ---------------------------------------------------------------------------

pub struct SqliteArtifactStore {
    conn: Arc<Mutex<Connection>>,
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
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = Self::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    /// Run schema migration — creates the `task_artifacts` table and index.
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
                metadata      TEXT NOT NULL DEFAULT '{}',
                created_at    TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_task_artifacts_task_id
                ON task_artifacts(task_id);
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
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let metadata_str =
            serde_json::to_string(&input.metadata).unwrap_or_else(|_| "{}".to_string());

        conn.execute(
            r#"
            INSERT INTO task_artifacts (id, task_id, agent_id, artifact_type, title, content, metadata, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                id,
                input.task_id,
                input.agent_id,
                input.artifact_type.as_str(),
                input.title,
                input.content,
                metadata_str,
                now_str,
            ],
        )
        .map_err(db_err)?;

        Ok(TaskArtifact {
            id,
            task_id: input.task_id,
            agent_id: input.agent_id,
            artifact_type: input.artifact_type,
            title: input.title,
            content: input.content,
            metadata: input.metadata,
            created_at: now,
        })
    }

    async fn get_artifact(&self, id: &str) -> crate::error::Result<Option<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, agent_id, artifact_type, title, content, metadata, created_at \
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
                "SELECT id, task_id, agent_id, artifact_type, title, content, metadata, created_at \
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
        assert!(!artifact.id.is_empty());

        // Fetch by ID
        let fetched = store.get_artifact(&artifact.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, artifact.id);
        assert_eq!(fetched.title, "Status Report");
        assert_eq!(fetched.artifact_type, ArtifactType::Report);
        assert_eq!(fetched.metadata["priority"], "high");

        // Fetch by task_id
        let task_artifacts = store.get_artifacts_for_task("task-1").await.unwrap();
        assert_eq!(task_artifacts.len(), 1);
        assert_eq!(task_artifacts[0].id, artifact.id);

        // Non-existent returns None / empty
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
                metadata: serde_json::Value::Object(serde_json::Map::new()),
            })
            .await
            .unwrap();

        assert_eq!(
            artifact.artifact_type,
            ArtifactType::Custom("analysis".into())
        );

        // Roundtrip through SQLite
        let fetched = store.get_artifact(&artifact.id).await.unwrap().unwrap();
        assert_eq!(
            fetched.artifact_type,
            ArtifactType::Custom("analysis".into())
        );
        assert_eq!(fetched.title, "Deep Analysis");
    }
}
