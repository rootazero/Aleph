# Teams Phase 2: Kanban + Task Dependencies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Kanban board support with task status tracking and automatic dependency unblocking to the teams module.

**Architecture:** Extend `TaskArtifact` with `status`, `blocked_by`, `assignee`, `priority` fields. Create `KanbanBoard` trait with `SqliteKanbanBoard` implementation. Build `KanbanAutoUnblocker` EventHandler that listens for `TeamPlanResolved`/`TeamArtifactSubmitted` events and automatically unblocks dependent tasks.

**Tech Stack:** Rust, tokio, rusqlite, serde_json, async-trait

**Prerequisite:** Phase 1 (EventBus integration) must be complete.

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/teams/artifacts.rs` | Modify | Add TaskStatus, extend TaskArtifact |
| `src/teams/messages/types.rs` | Modify | Add Task* MessageType variants |
| `src/teams/kanban/mod.rs` | Create | KanbanBoard trait + SqliteKanbanBoard |
| `src/teams/kanban/unblocker.rs` | Create | KanbanAutoUnblocker EventHandler |
| `src/teams/mod.rs` | Modify | Export Kanban types |
| `src/teams/plans.rs` | Modify | Integrate with Kanban status changes |
| `tests/teams_kanban_test.rs` | Create | Integration tests for Kanban flow |

---

### Task 1: Extend TaskArtifact with Status Fields

**Files:**
- Modify: `src/teams/artifacts.rs`
- Test: `src/teams/artifacts.rs` (existing test module)

- [ ] **Step 1: Define TaskStatus enum**

Add after `ArtifactType`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Failed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Completed => "completed",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Failed => "failed",
        }
    }
    
    pub fn from_stored(s: &str) -> Self {
        match s {
            "pending" => TaskStatus::Pending,
            "in_progress" => TaskStatus::InProgress,
            "completed" => TaskStatus::Completed,
            "blocked" => TaskStatus::Blocked,
            "failed" => TaskStatus::Failed,
            _ => TaskStatus::Pending,
        }
    }
    
    pub fn can_transition_to(&self, new_status: &TaskStatus) -> bool {
        match (self, new_status) {
            (TaskStatus::Pending, TaskStatus::InProgress) => true,
            (TaskStatus::Pending, TaskStatus::Blocked) => true,
            (TaskStatus::InProgress, TaskStatus::Completed) => true,
            (TaskStatus::InProgress, TaskStatus::Failed) => true,
            (TaskStatus::Blocked, TaskStatus::Pending) => true, // Unblocked
            (TaskStatus::Failed, TaskStatus::Pending) => true,   // Retry
            (TaskStatus::Completed, _) => false,                  // Terminal
            (a, b) if a == b => true,                             // No-op
            _ => false,
        }
    }
}
```

- [ ] **Step 2: Extend TaskArtifact struct**

```rust
pub struct TaskArtifact {
    pub id: String,
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    pub status: TaskStatus,              // NEW
    pub blocked_by: Vec<String>,         // NEW: artifact IDs that block this task
    pub assignee: Option<String>,        // NEW
    pub priority: i32,                   // NEW: lower = higher priority
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,  // NEW
    pub completed_at: Option<DateTime<Utc>>, // NEW
}
```

- [ ] **Step 3: Update NewArtifact**

```rust
pub struct NewArtifact {
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub status: TaskStatus,              // NEW: defaults to Pending
    #[serde(default)]
    pub blocked_by: Vec<String>,         // NEW
    #[serde(default)]
    pub assignee: Option<String>,        // NEW
    #[serde(default = "default_priority")]
    pub priority: i32,                   // NEW
}

fn default_priority() -> i32 { 0 }
```

- [ ] **Step 4: Update database schema in migrate()**

```rust
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
        "#,
    )
    .map_err(db_err)?;
    Ok(())
}
```

- [ ] **Step 5: Update row reader**

```rust
fn read_artifact_row(row: &rusqlite::Row<'>>) -> rusqlite::Result<TaskArtifact> {
    let artifact_type_str: String = row.get(3)?;
    let metadata_str: String = row.get(6)?;
    let status_str: String = row.get(8)?;
    let blocked_by_str: String = row.get(9)?;
    let assignee: Option<String> = row.get(10)?;
    let priority: i32 = row.get(11)?;
    let created_at_str: String = row.get(12)?;
    let started_at_str: Option<String> = row.get(13)?;
    let completed_at_str: Option<String> = row.get(14)?;

    Ok(TaskArtifact {
        id: row.get(0)?,
        task_id: row.get(1)?,
        agent_id: row.get(2)?,
        artifact_type: ArtifactType::from_stored(&artifact_type_str),
        title: row.get(4)?,
        content: row.get(5)?,
        metadata: serde_json::from_str(&metadata_str)
            .unwrap_or_else(|_| default_metadata()),
        status: TaskStatus::from_stored(&status_str),
        blocked_by: serde_json::from_str(&blocked_by_str)
            .unwrap_or_default(),
        assignee,
        priority,
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        started_at: started_at_str.and_then(|s| 
            DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
        completed_at: completed_at_str.and_then(|s| 
            DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
    })
}
```

- [ ] **Step 6: Update create_artifact**

```rust
async fn create_artifact(&self, 
    input: NewArtifact
) -> crate::error::Result<TaskArtifact> {
    let conn = self.conn.lock().await;
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    
    let metadata_str = serde_json::to_string(&input.metadata)
        .unwrap_or_else(|_| "{}".to_string());
    let blocked_by_str = serde_json::to_string(&input.blocked_by)
        .unwrap_or_else(|_| "[]".to_string());
    
    conn.execute(
        r#"
        INSERT INTO task_artifacts 
        (id, task_id, agent_id, artifact_type, title, content, 
         status, blocked_by, assignee, priority, metadata, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            id, input.task_id, input.agent_id, 
            input.artifact_type.as_str(), input.title, input.content,
            input.status.as_str(), blocked_by_str, 
            input.assignee, input.priority, metadata_str, 
            now.to_rfc3339(),
        ],
    ).map_err(db_err)?;
    
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
```

- [ ] **Step 7: Add status update method**

```rust
pub async fn update_status(
    &self,
    artifact_id: &str,
    new_status: TaskStatus,
) -> crate::error::Result<TaskArtifact> {
    let conn = self.conn.lock().await;
    let now = Utc::now();
    
    let (started_at, completed_at) = match new_status {
        TaskStatus::InProgress => (Some(now.to_rfc3339()), None),
        TaskStatus::Completed => (None, Some(now.to_rfc3339())),
        _ => (None, None),
    };
    
    conn.execute(
        "UPDATE task_artifacts SET status = ?1, started_at = ?2, completed_at = ?3 WHERE id = ?4",
        params![new_status.as_str(), started_at, completed_at, artifact_id],
    ).map_err(db_err)?;
    
    drop(conn);
    self.get_artifact(artifact_id).await
        .and_then(|opt| opt.ok_or_else(|| db_err("artifact not found after update")))
}
```

- [ ] **Step 8: Write test for status transitions**

```rust
#[tokio::test]
async fn test_artifact_status_transitions() {
    let store = SqliteArtifactStore::new_in_memory().await;
    
    let artifact = store.create_artifact(NewArtifact {
        task_id: "task-1".into(),
        agent_id: "agent-1".into(),
        artifact_type: ArtifactType::Plan,
        title: "Plan".into(),
        content: "Details".into(),
        metadata: json!({}),
        status: TaskStatus::Pending,
        blocked_by: vec![],
        assignee: Some("agent-2".into()),
        priority: 1,
    }).await.unwrap();
    
    assert_eq!(artifact.status, TaskStatus::Pending);
    assert!(artifact.started_at.is_none());
    
    // Transition to InProgress
    let updated = store.update_status(&artifact.id, TaskStatus::InProgress).await.unwrap();
    assert_eq!(updated.status, TaskStatus::InProgress);
    assert!(updated.started_at.is_some());
    
    // Transition to Completed
    let completed = store.update_status(&artifact.id, TaskStatus::Completed).await.unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert!(completed.completed_at.is_some());
}
```

- [ ] **Step 9: Run tests**

```bash
cargo test -p alephcore teams::artifacts --lib
```
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add src/teams/artifacts.rs
git commit -m "teams(artifacts): add TaskStatus and status fields to TaskArtifact"
```

---

### Task 2: Create KanbanBoard Implementation

**Files:**
- Create: `src/teams/kanban/mod.rs`
- Test: `src/teams/kanban/mod.rs` (inline test module)

- [ ] **Step 1: Create KanbanBoard trait and types**

```rust
//! Kanban board for team task management.

use async_trait::async_trait;
use rusqlite::{params, Connection};
use tokio::sync::Mutex;
use chrono::Utc;

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, TaskArtifact, TaskStatus};

/// Kanban column grouping
pub struct KanbanColumns {
    pub pending: Vec<TaskArtifact>,
    pub in_progress: Vec<TaskArtifact>,
    pub completed: Vec<TaskArtifact>,
    pub blocked: Vec<TaskArtifact>,
    pub failed: Vec<TaskArtifact>,
}

/// Kanban board operations
#[async_trait]
pub trait KanbanBoard: Send + Sync {
    /// Get all tasks for a team grouped by status
    async fn get_board(&self, 
        team_id: &str
    ) -> Result<KanbanColumns>;
    
    /// Move task to new status
    async fn move_task(
        &self, 
        artifact_id: &str, 
        new_status: TaskStatus
    ) -> Result<TaskArtifact>;
    
    /// Complete task and return unblocked tasks
    async fn complete_task(
        &self, 
        artifact_id: &str
    ) -> Result<Vec<TaskArtifact>>;
    
    /// Add dependency
    async fn add_dependency(
        &self, 
        artifact_id: &str, 
        depends_on: &str
    ) -> Result<()>;
    
    /// Get tasks assigned to agent
    async fn get_agent_tasks(
        &self, 
        agent_id: &str
    ) -> Result<Vec<TaskArtifact>>;
}
```

- [ ] **Step 2: Implement SqliteKanbanBoard**

```rust
pub struct SqliteKanbanBoard {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteKanbanBoard {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }
    
    #[cfg(test)]
    pub async fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let board = Self::new(conn);
        board.migrate().await.expect("migrate");
        board
    }
    
    pub async fn migrate(&self
) -> Result<()> {
        // Schema handled by ArtifactStore; this is for any kanban-specific tables
        Ok(())
    }
}

#[async_trait]
impl KanbanBoard for SqliteKanbanBoard {
    async fn get_board(&self, 
        team_id: &str
    ) -> Result<KanbanColumns> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT * FROM task_artifacts WHERE task_id = ?1 ORDER BY priority ASC, created_at DESC"
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban: {e}"),
            suggestion: None,
        })?;
        
        let artifacts: Vec<TaskArtifact> = stmt
            .query_map([team_id], |row| {
                // ... row mapping ...
                Ok(TaskArtifact { /* ... */ })
            })
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("Kanban: {e}"),
                suggestion: None,
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("Kanban: {e}"),
                suggestion: None,
            })?;
        
        Ok(KanbanColumns {
            pending: artifacts.iter().filter(|a| a.status == TaskStatus::Pending).cloned().collect(),
            in_progress: artifacts.iter().filter(|a| a.status == TaskStatus::InProgress).cloned().collect(),
            completed: artifacts.iter().filter(|a| a.status == TaskStatus::Completed).cloned().collect(),
            blocked: artifacts.iter().filter(|a| a.status == TaskStatus::Blocked).cloned().collect(),
            failed: artifacts.iter().filter(|a| a.status == TaskStatus::Failed).cloned().collect(),
        })
    }
    
    async fn move_task(
        &self, 
        artifact_id: &str, 
        new_status: TaskStatus
    ) -> Result<TaskArtifact> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        
        let (started, completed) = match new_status {
            TaskStatus::InProgress => (Some(&now as &dyn rusqlite::types::ToSql), None),
            TaskStatus::Completed => (None, Some(&now as &dyn rusqlite::types::ToSql)),
            _ => (None, None),
        };
        
        conn.execute(
            "UPDATE task_artifacts SET status = ?1, started_at = ?2, completed_at = ?3 WHERE id = ?4",
            params![new_status.as_str(), started, completed, artifact_id],
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban move: {e}"),
            suggestion: None,
        })?;
        
        // Fetch updated artifact
        let mut stmt = conn.prepare(
            "SELECT * FROM task_artifacts WHERE id = ?1"
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban fetch: {e}"),
            suggestion: None,
        })?;
        
        let artifact = stmt.query_row([artifact_id], |row| {
            Ok(TaskArtifact { /* ... */ })
        }).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban fetch: {e}"),
            suggestion: None,
        })?;
        
        Ok(artifact)
    }
    
    async fn complete_task(
        &self, 
        artifact_id: &str
    ) -> Result<Vec<TaskArtifact>> {
        // 1. Mark as completed
        self.move_task(artifact_id, TaskStatus::Completed).await?;
        
        // 2. Find blocked tasks that depend on this artifact
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, blocked_by FROM task_artifacts WHERE blocked_by LIKE ?1"
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban unblock: {e}"),
            suggestion: None,
        })?;
        
        let blocked_ids: Vec<String> = stmt
            .query_map([format!("%{}%", artifact_id)], |row| {
                let id: String = row.get(0)?;
                let blocked_by_str: String = row.get(1)?;
                let blocked_by: Vec<String> = serde_json::from_str(&blocked_by_str).unwrap_or_default();
                if blocked_by.contains(&artifact_id.to_string()) {
                    Ok(Some(id))
                } else {
                    Ok(None)
                }
            })
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("Kanban unblock: {e}"),
                suggestion: None,
            })?
            .filter_map(|r| r.unwrap_or(None))
            .collect();
        
        drop(conn);
        
        // 3. Check if all dependencies are now completed
        let mut unblocked = vec![];
        for blocked_id in blocked_ids {
            if self.all_dependencies_completed(&blocked_id).await? {
                let task = self.move_task(&blocked_id, TaskStatus::Pending).await?;
                unblocked.push(task);
            }
        }
        
        Ok(unblocked)
    }
    
    async fn add_dependency(
        &self, 
        artifact_id: &str, 
        depends_on: &str
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        
        // Get current blocked_by
        let blocked_by_str: String = conn.query_row(
            "SELECT blocked_by FROM task_artifacts WHERE id = ?1",
            [artifact_id],
            |row| row.get(0),
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban dep: {e}"),
            suggestion: None,
        })?;
        
        let mut blocked_by: Vec<String> = serde_json::from_str(&blocked_by_str).unwrap_or_default();
        if !blocked_by.contains(&depends_on.to_string()) {
            blocked_by.push(depends_on.to_string());
        }
        
        let new_blocked_by = serde_json::to_string(&blocked_by).unwrap_or_else(|_| "[]".to_string());
        
        // Update blocked_by and set status to Blocked
        conn.execute(
            "UPDATE task_artifacts SET blocked_by = ?1, status = 'blocked' WHERE id = ?2",
            params![new_blocked_by, artifact_id],
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban dep: {e}"),
            suggestion: None,
        })?;
        
        Ok(())
    }
    
    async fn get_agent_tasks(
        &self, 
        agent_id: &str
    ) -> Result<Vec<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT * FROM task_artifacts WHERE assignee = ?1 OR agent_id = ?1 ORDER BY priority ASC"
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban agent: {e}"),
            suggestion: None,
        })?;
        
        let tasks = stmt
            .query_map([agent_id], |row| {
                Ok(TaskArtifact { /* ... */ })
            })
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("Kanban agent: {e}"),
                suggestion: None,
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("Kanban agent: {e}"),
                suggestion: None,
            })?;
        
        Ok(tasks)
    }
}

impl SqliteKanbanBoard {
    async fn all_dependencies_completed(
        &self, 
        artifact_id: &str
    ) -> Result<bool> {
        let conn = self.conn.lock().await;
        
        let blocked_by_str: String = conn.query_row(
            "SELECT blocked_by FROM task_artifacts WHERE id = ?1",
            [artifact_id],
            |row| row.get(0),
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban check: {e}"),
            suggestion: None,
        })?;
        
        let blocked_by: Vec<String> = serde_json::from_str(&blocked_by_str).unwrap_or_default();
        if blocked_by.is_empty() {
            return Ok(true);
        }
        
        // Check if all dependencies are completed
        let placeholders = blocked_by.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT COUNT(*) FROM task_artifacts WHERE id IN ({}) AND status != 'completed'",
            placeholders
        );
        
        let params: Vec<&dyn rusqlite::types::ToSql> = 
            blocked_by.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
        
        let incomplete_count: i64 = conn.query_row(
            &sql,
            params.as_slice(),
            |row| row.get(0),
        ).map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Kanban check: {e}"),
            suggestion: None,
        })?;
        
        Ok(incomplete_count == 0)
    }
}
```

- [ ] **Step 3: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_kanban_board_grouping() {
        let board = SqliteKanbanBoard::new_in_memory().await;
        // Create test artifacts via direct SQL or artifact store
        // ... setup ...
        
        let columns = board.get_board("team-1").await.unwrap();
        assert!(!columns.pending.is_empty() || !columns.completed.is_empty());
    }
    
    #[tokio::test]
    async fn test_complete_unblocks_dependencies() {
        let board = SqliteKanbanBoard::new_in_memory().await;
        
        // Setup: task B depends on task A
        // Complete task A
        // Verify task B is unblocked (status = Pending)
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore teams::kanban --lib
```

- [ ] **Step 5: Commit**

```bash
git add src/teams/kanban/mod.rs
git commit -m "teams(kanban): add KanbanBoard trait and SqliteKanbanBoard"
```

---

### Task 3: Create KanbanAutoUnblocker EventHandler

**Files:**
- Create: `src/teams/kanban/unblocker.rs`
- Modify: `src/teams/messages/types.rs`
- Test: `src/teams/kanban/unblocker.rs` (inline test module)

- [ ] **Step 1: Add new MessageType variants**

```rust
pub enum MessageType {
    // ... existing ...
    TaskAssigned,
    TaskStatusChanged,
    TaskUnblocked,
    DependencyAdded,
}
```

- [ ] **Step 2: Implement KanbanAutoUnblocker**

```rust
use crate::event::handler::{EventHandler, EventContext, HandlerError};
use crate::event::types::{AlephEvent, EventType, TeamPlanResolvedEvent, TeamArtifactSubmittedEvent};
use crate::teams::kanban::KanbanBoard;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::{MessageType, Recipient, RecipientRole};

pub struct KanbanAutoUnblocker {
    kanban: Arc<dyn KanbanBoard>,
    msg_router: Arc<MessageRouter>,
}

impl KanbanAutoUnblocker {
    pub fn new(
        kanban: Arc<dyn KanbanBoard>,
        msg_router: Arc<MessageRouter>,
    ) -> Self {
        Self { kanban, msg_router }
    }
}

#[async_trait]
impl EventHandler for KanbanAutoUnblocker {
    fn name(&self) -> &'static str {
        "KanbanAutoUnblocker"
    }
    
    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::TeamPlanResolved,
            EventType::TeamArtifactSubmitted,
        ]
    }
    
    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        let artifact_id = match event {
            AlephEvent::TeamPlanResolved(e) if e.approved => &e.artifact_id,
            AlephEvent::TeamArtifactSubmitted(e) => &e.artifact_id,
            _ => return Ok(vec![]),
        };
        
        let unblocked = self.kanban.complete_task(artifact_id).await
            .map_err(|e| HandlerError::Internal(e.to_string()))?;
        
        for task in unblocked {
            // Send notification to assignee
            let target = task.assignee.unwrap_or_else(|| task.agent_id.clone());
            
            self.msg_router.send(SendRequest {
                team_id: task.team_id.clone(),
                from_agent: "system".to_string(),
                to: vec![target],
                cc: vec![],
                msg_type: MessageType::TaskUnblocked,
                subject: format!("Task unblocked: {}", task.title),
                content: format!(
                    "Task '{}' is now unblocked and ready to start.\n\nPriority: {}",
                    task.title, task.priority
                ),
                reply_to: None,
                attachments: vec![task.id.clone()],
            }).await.ok();
            
            // Publish event
            ctx.bus.publish(AlephEvent::TeamTaskUnblocked(
                crate::event::types::TeamTaskUnblockedEvent {
                    team_id: task.team_id,
                    task_id: task.id,
                    unblocked_by: artifact_id.clone(),
                }
            )).await;
        }
        
        Ok(vec![])
    }
}
```

- [ ] **Step 3: Write test**

```rust
#[tokio::test]
async fn test_auto_unblocker_unblocks_tasks() {
    // Setup: Create kanban board with dependency
    // Publish TeamPlanResolved event
    // Verify blocked task status changed to Pending
    // Verify notification message was sent
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore teams::kanban::unblocker --lib
```

- [ ] **Step 5: Commit**

```bash
git add src/teams/kanban/unblocker.rs src/teams/messages/types.rs
git commit -m "teams(kanban): add KanbanAutoUnblocker EventHandler"
```

---

### Task 4: Integrate Kanban with PlanManager

**Files:**
- Modify: `src/teams/plans.rs`
- Modify: `src/teams/mod.rs`

- [ ] **Step 1: Update PlanManager to use Kanban**

```rust
pub struct PlanManager {
    msg_router: Arc<MessageRouter>,
    artifact_store: Arc<dyn ArtifactStore>,
    event_store: Arc<dyn EventLogStore>,
    kanban: Option<Arc<dyn KanbanBoard>>, // NEW
    bus: Option<EventBus>,
}

impl PlanManager {
    pub fn with_kanban(mut self, kanban: Arc<dyn KanbanBoard>) -> Self {
        self.kanban = Some(kanban);
        self
    }
    
    pub async fn approve_plan(...) -> Result<TeamMessage> {
        // ... existing approval logic ...
        
        // Update kanban status
        if let Some(ref kanban) = self.kanban {
            kanban.move_task(
                &submission.artifact.id, 
                TaskStatus::InProgress
            ).await.ok();
        }
        
        // Publish event
        if let Some(ref bus) = self.bus {
            bus.publish(AlephEvent::TeamPlanResolved(...)).await;
        }
        
        Ok(msg)
    }
}
```

- [ ] **Step 2: Update exports**

```rust
pub use kanban::{KanbanBoard, SqliteKanbanBoard, KanbanColumns, TaskStatus};
pub use kanban::unblocker::KanbanAutoUnblocker;
```

- [ ] **Step 3: Run integration tests**

```bash
cargo test -p alephcore teams::integration_tests --lib
cargo test -p alephcore teams::kanban --lib
```

- [ ] **Step 4: Commit**

```bash
git add src/teams/plans.rs src/teams/mod.rs src/teams/kanban/
git commit -m "teams: integrate Kanban with PlanManager"
```

---

## Self-Review Checklist

- [ ] Spec coverage: Kanban board, status transitions, dependency unblocking all have tasks
- [ ] Placeholder scan: No TBD or vague descriptions
- [ ] Type consistency: TaskStatus, KanbanColumns match spec
- [ ] Test coverage: Board grouping, unblocking, integration tests
