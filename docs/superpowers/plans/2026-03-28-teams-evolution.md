# Teams Evolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Evolve Aleph's teams module into a three-layer communication architecture (TaskCoordinator + MessageRouter + CollaborativeSession) with Explorer/Critic role mechanisms.

**Architecture:** Four phases: (1) Unify task systems and add artifacts, (2) Build SQLite-backed message inbox with to/cc routing and threads, (3) Add collaborative sessions with escalation suggestions, (4) Add role types and review_score tool with configurable validation. Each phase is independently testable and deployable.

**Tech Stack:** Rust, rusqlite (SQLite), serde/schemars (JSON Schema), chrono (timestamps), uuid (IDs), tokio (async)

**Spec:** `docs/superpowers/specs/2026-03-28-teams-evolution-design.md`

**Note on SQLite threading:** All new stores wrap `rusqlite::Connection` in `tokio::sync::Mutex`, same pattern as `SqliteCoordTaskStore` and `SqliteTeamStore`.

**Note on tool registration:** New tools follow the existing pattern: (1) add `BuiltinToolDefinition` in `definitions.rs`, (2) add fields in `registry.rs`, (3) instantiate in `builder.rs`, (4) add to tool category in `groups.rs`, (5) add match arm in `execute_tool()` in `registry.rs` (see existing team tools at ~line 568 for the pattern — each tool needs `"tool_name" => self.tool_field.as_ref()?.call_json(args).await`).

**Note on SQLite connections:** All new stores can share the same `Connection` as `SqliteTeamStore` (same database file, separate tables). Pass the existing `Arc<Mutex<Connection>>` from `SqliteTeamStore` to avoid proliferating database connections. The `migrate()` method of each store creates its own tables independently.

**Note on test placement:** Tests go inline as `#[cfg(test)] mod tests { ... }` at the bottom of each source file, following the existing codebase pattern. Do NOT create separate `tests/` directories.

---

## File Structure

### Phase 1 — Layer 1: Task Unification + Artifacts

**New Files:**
- `src/teams/artifacts.rs` — TaskArtifact, ArtifactType types + SQLite storage
- `src/teams/events.rs` — TeamEvent, TeamEventType types + SQLite event log store
- `src/builtin_tools/team/task_submit.rs` — task_submit tool
- `src/builtin_tools/team/task_read_artifact.rs` — task_read_artifact tool

**Modified Files:**
- `src/teams/types.rs` — Remove TeamTask, TeamTaskStatus (retired)
- `src/teams/store.rs` — Remove task methods from TeamStore trait + SqliteTeamStore; add artifact + event store traits
- `src/teams/mod.rs` — Add `pub mod artifacts; pub mod events;`
- `src/builtin_tools/team/delegate.rs` — Migrate from TeamStore tasks to CoordTaskStore; auto-persist artifacts
- `src/builtin_tools/team/status.rs` — Read tasks from CoordTaskStore instead of TeamStore
- `src/builtin_tools/team/mod.rs` — Export new tools
- `src/executor/builtin_registry/definitions.rs` — Add task_submit, task_read_artifact definitions
- `src/executor/builtin_registry/registry.rs` — Add tool fields
- `src/executor/builtin_registry/builder.rs` — Instantiate new tools
- `src/executor/builtin_registry/groups.rs` — Add to "team" category

### Phase 2 — Layer 2: MessageRouter

**New Files:**
- `src/teams/messages/mod.rs` — Re-exports
- `src/teams/messages/types.rs` — TeamMessage, Recipient, RecipientRole, MessageType
- `src/teams/messages/store.rs` — MessageStore trait + SqliteMessageStore (3 tables)
- `src/teams/messages/router.rs` — MessageRouter: send, TTL computation, escalation check
- `src/teams/messages/inbox.rs` — Inbox: read, thread, expire
- `src/teams/context.rs` — InboxContext for ContextInjector integration
- `src/builtin_tools/team/message_send.rs` — message_send tool
- `src/builtin_tools/team/inbox_read.rs` — inbox_read tool (inbox + thread mode)
- `src/builtin_tools/team/team_digest.rs` — team_digest tool

**Modified Files:**
- `src/teams/mod.rs` — Add `pub mod messages; pub mod context;`
- `src/agents/swarm/context_injector.rs` — Add inbox awareness (InboxContext)
- `src/builtin_tools/team/mod.rs` — Export new tools
- `src/executor/builtin_registry/definitions.rs` — Add message_send, inbox_read, team_digest
- `src/executor/builtin_registry/registry.rs` — Add tool fields
- `src/executor/builtin_registry/builder.rs` — Instantiate new tools
- `src/executor/builtin_registry/groups.rs` — Add to "team" category

### Phase 3 — Layer 3: CollaborativeSession

**New Files:**
- `src/teams/sessions/mod.rs` — Re-exports
- `src/teams/sessions/types.rs` — CollaborativeSession, SessionTurn, SessionOutcome, SessionTrigger, SessionStatus, EscalationRule
- `src/teams/sessions/store.rs` — SessionStore trait + SqliteSessionStore
- `src/teams/sessions/coordinator.rs` — Session lifecycle: create, add turn, conclude, cancel
- `src/builtin_tools/team/session_collaborate.rs` — session_collaborate tool
- `src/builtin_tools/team/session_turn.rs` — session_turn tool (respond/conclude modes)
- `src/builtin_tools/team/session_read.rs` — session_read tool

**Modified Files:**
- `src/teams/mod.rs` — Add `pub mod sessions;`
- `src/teams/messages/router.rs` — Add escalation check after message delivery
- `src/builtin_tools/team/mod.rs` — Export new tools
- `src/executor/builtin_registry/definitions.rs` — Add 3 session tools
- `src/executor/builtin_registry/registry.rs` — Add tool fields
- `src/executor/builtin_registry/builder.rs` — Instantiate new tools
- `src/executor/builtin_registry/groups.rs` — Add to "team" category

### Phase 4 — Role Mechanism

**New Files:**
- `src/teams/roles/mod.rs` — Re-exports
- `src/teams/roles/types.rs` — AgentRole, TeamRoleConfig, Severity
- `src/teams/roles/review.rs` — ReviewScore, DimensionScore, Challenge types + validation logic
- `src/builtin_tools/team/review_score.rs` — review_score tool with configurable validation

**Modified Files:**
- `src/teams/mod.rs` — Add `pub mod roles;`
- `src/teams/types.rs` — Add role config to TeamMember or Team
- `src/builtin_tools/team/mod.rs` — Export new tool
- `src/executor/builtin_registry/definitions.rs` — Add review_score
- `src/executor/builtin_registry/registry.rs` — Add tool field
- `src/executor/builtin_registry/builder.rs` — Instantiate new tool
- `src/executor/builtin_registry/groups.rs` — Add to "team" category

---

## Task 1: Retire TeamTask — Unify on CoordTask

**Files:**
- Modify: `src/teams/types.rs`
- Modify: `src/teams/store.rs`
- Modify: `src/builtin_tools/team/delegate.rs`
- Modify: `src/builtin_tools/team/status.rs`

- [ ] **Step 0: Verify NewCoordTask has team_id field**

Check `src/agents/swarm/tasks/mod.rs` — confirm `NewCoordTask` struct has `pub team_id: Option<String>`. If not present, add it and update `SqliteCoordTaskStore::create_task()` in `src/agents/swarm/tasks/store.rs` to persist it.

- [ ] **Step 1: Write test verifying CoordTaskStore is used for team delegation**

Add to `#[cfg(test)] mod tests` at the bottom of `src/teams/store.rs`:

```rust
#[tokio::test]
async fn test_team_delegate_uses_coord_task_store() {
    // Verify that team_delegate creates a CoordTask, not a TeamTask
    let coord_store = SqliteCoordTaskStore::new_in_memory().await.unwrap();
    let task = coord_store.create_task(NewCoordTask {
        team_id: Some("team-1".to_string()),
        subject: "test task".to_string(),
        description: "test".to_string(),
        owner: Some("agent-1".to_string()),
        priority: Priority::Normal,
        metadata: serde_json::Value::Null,
        dependencies: vec![],
    }).await.unwrap();

    assert_eq!(task.team_id, Some("team-1".to_string()));
    assert_eq!(task.status, CoordTaskStatus::Pending);
}
```

- [ ] **Step 2: Run test to verify it passes (CoordTaskStore already supports team_id)**

Run: `cargo test -p alephcore --lib test_team_delegate_uses_coord_task_store`

- [ ] **Step 3: Remove TeamTask, TeamTaskStatus, NewTeamTask from types.rs**

In `src/teams/types.rs`, remove:
- `TeamTaskStatus` enum and its impls
- `TeamTask` struct
- `NewTeamTask` struct

Keep: `Team`, `TeamMember`, `TeamSummary`, `NewTeam`, `NewTeamMember`, `TeamId`, `TeamStatus`.

- [ ] **Step 4: Remove task methods from TeamStore trait and SqliteTeamStore**

In `src/teams/store.rs`:
- Remove `create_task`, `update_task_status`, `get_tasks` from `TeamStore` trait
- Remove their implementations in `SqliteTeamStore`
- Keep the `team_tasks` table in SQLite migration for backward compatibility (data migration not needed — old tasks are historical records)

- [ ] **Step 5: Update TeamDelegateTool to use CoordTaskStore**

In `src/builtin_tools/team/delegate.rs`:
- Add `coord_store: Arc<dyn CoordTaskStore>` field to `TeamDelegateTool`
- Replace `self.store.create_task(NewTeamTask{..})` with `self.coord_store.create_task(NewCoordTask{..})`
- Replace `self.store.update_task_status(...)` with `self.coord_store.update_task(id, CoordTaskUpdate{..})`
- Update constructor `new()` and `with_context()` to accept `coord_store`

- [ ] **Step 6: Update TeamStatusTool to read from CoordTaskStore**

In `src/builtin_tools/team/status.rs`:
- Add `coord_store: Arc<dyn CoordTaskStore>` field
- Replace `self.store.get_tasks(team_id)` with `self.coord_store.list_tasks(CoordTaskFilter { team_id: Some(team_id), .. })`
- Map `CoordTask` fields to the existing `TaskInfo` output type

- [ ] **Step 7: Update builder.rs to pass CoordTaskStore to team tools**

In `src/executor/builtin_registry/builder.rs`:
- Pass `coord_task_store` to `TeamDelegateTool::new()` and `TeamStatusTool::new()`

- [ ] **Step 8: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: All existing team tests pass with CoordTask backend.

- [ ] **Step 9: Commit**

```bash
git add -A && git commit -m "teams: unify task system — retire TeamTask in favor of CoordTask"
```

---

## Task 2: Task Artifact System — Types + Storage

**Files:**
- Create: `src/teams/artifacts.rs`
- Modify: `src/teams/mod.rs`

- [ ] **Step 1: Write tests for artifact storage**

Add to `src/teams/artifacts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_read_artifact() {
        let store = SqliteArtifactStore::new_in_memory().await.unwrap();
        let artifact = store.create_artifact(NewArtifact {
            task_id: "task-1".to_string(),
            agent_id: "agent-1".to_string(),
            artifact_type: ArtifactType::Report,
            title: "Test Report".to_string(),
            content: "# Results\nAll good.".to_string(),
            metadata: serde_json::json!({"score": 8}),
        }).await.unwrap();

        assert_eq!(artifact.task_id, "task-1");
        assert_eq!(artifact.artifact_type, ArtifactType::Report);

        let artifacts = store.get_artifacts_for_task("task-1").await.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].title, "Test Report");
    }

    #[tokio::test]
    async fn test_custom_artifact_type() {
        let store = SqliteArtifactStore::new_in_memory().await.unwrap();
        let artifact = store.create_artifact(NewArtifact {
            task_id: "task-2".to_string(),
            agent_id: "agent-1".to_string(),
            artifact_type: ArtifactType::Custom("analysis".to_string()),
            title: "Custom Analysis".to_string(),
            content: "content".to_string(),
            metadata: serde_json::Value::Null,
        }).await.unwrap();

        assert!(matches!(artifact.artifact_type, ArtifactType::Custom(ref s) if s == "analysis"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib test_create_and_read_artifact`
Expected: FAIL — module doesn't exist yet.

- [ ] **Step 3: Implement artifact types and store**

Create `src/teams/artifacts.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;
use rusqlite::Connection;

// --- Types ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Report,
    Code,
    Review,
    Discovery,
    Challenge,
    Custom(String),
}

impl ArtifactType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Report => "report",
            Self::Code => "code",
            Self::Review => "review",
            Self::Discovery => "discovery",
            Self::Challenge => "challenge",
            Self::Custom(s) => s.as_str(),
        }
    }

    pub fn from_stored(s: &str) -> Self {
        match s {
            "report" => Self::Report,
            "code" => Self::Code,
            "review" => Self::Review,
            "discovery" => Self::Discovery,
            "challenge" => Self::Challenge,
            other => Self::Custom(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub id: String,
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub struct NewArtifact {
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    pub metadata: serde_json::Value,
}

// --- Store trait ---

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn create_artifact(&self, input: NewArtifact) -> crate::error::Result<TaskArtifact>;
    async fn get_artifact(&self, id: &str) -> crate::error::Result<Option<TaskArtifact>>;
    async fn get_artifacts_for_task(&self, task_id: &str) -> crate::error::Result<Vec<TaskArtifact>>;
}

// --- SQLite implementation ---

pub struct SqliteArtifactStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteArtifactStore {
    pub async fn new(conn: Connection) -> crate::error::Result<Self> {
        let store = Self { conn: Arc::new(Mutex::new(conn)) };
        store.migrate().await?;
        Ok(store)
    }

    #[cfg(test)]
    pub async fn new_in_memory() -> crate::error::Result<Self> {
        Self::new(Connection::open_in_memory()?).await
    }

    async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS task_artifacts (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                artifact_type TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_task_artifacts_task ON task_artifacts(task_id);
        ")?;
        Ok(())
    }
}

#[async_trait]
impl ArtifactStore for SqliteArtifactStore {
    async fn create_artifact(&self, input: NewArtifact) -> crate::error::Result<TaskArtifact> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO task_artifacts (id, task_id, agent_id, artifact_type, title, content, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![id, input.task_id, input.agent_id, input.artifact_type.as_str(), input.title, input.content, serde_json::to_string(&input.metadata)?, now.to_rfc3339()],
        )?;
        Ok(TaskArtifact { id, task_id: input.task_id, agent_id: input.agent_id, artifact_type: input.artifact_type, title: input.title, content: input.content, metadata: input.metadata, created_at: now })
    }

    async fn get_artifact(&self, id: &str) -> crate::error::Result<Option<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare("SELECT id, task_id, agent_id, artifact_type, title, content, metadata, created_at FROM task_artifacts WHERE id = ?1")?;
        let result = stmt.query_row(rusqlite::params![id], |row| {
            Ok(TaskArtifact {
                id: row.get(0)?,
                task_id: row.get(1)?,
                agent_id: row.get(2)?,
                artifact_type: ArtifactType::from_stored(&row.get::<_, String>(3)?),
                title: row.get(4)?,
                content: row.get(5)?,
                metadata: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?).map(|dt| dt.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
            })
        }).optional()?;
        Ok(result)
    }

    async fn get_artifacts_for_task(&self, task_id: &str) -> crate::error::Result<Vec<TaskArtifact>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare("SELECT id, task_id, agent_id, artifact_type, title, content, metadata, created_at FROM task_artifacts WHERE task_id = ?1 ORDER BY created_at ASC")?;
        let rows = stmt.query_map(rusqlite::params![task_id], |row| {
            Ok(TaskArtifact {
                id: row.get(0)?,
                task_id: row.get(1)?,
                agent_id: row.get(2)?,
                artifact_type: ArtifactType::from_stored(&row.get::<_, String>(3)?),
                title: row.get(4)?,
                content: row.get(5)?,
                metadata: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?).map(|dt| dt.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
```

- [ ] **Step 4: Add module to teams/mod.rs**

Add `pub mod artifacts;` to `src/teams/mod.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib test_create_and_read_artifact test_custom_artifact_type`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "teams: add TaskArtifact types and SQLite storage"
```

---

## Task 3: Event Log System

**Files:**
- Create: `src/teams/events.rs`
- Modify: `src/teams/mod.rs`

- [ ] **Step 1: Write tests for event log**

```rust
#[tokio::test]
async fn test_log_and_query_events() {
    let store = SqliteEventLogStore::new_in_memory().await.unwrap();
    store.log_event(NewTeamEvent {
        team_id: "team-1".to_string(),
        event_type: TeamEventType::MessageSent,
        agent_id: "agent-1".to_string(),
        payload: serde_json::json!({"msg": "hello"}),
    }).await.unwrap();

    let events = store.get_events("team-1", None, None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].event_type, TeamEventType::MessageSent));
}

#[tokio::test]
async fn test_prune_old_events() {
    let store = SqliteEventLogStore::new_in_memory().await.unwrap();
    // Create an old event by inserting with past timestamp
    store.log_event(NewTeamEvent {
        team_id: "team-1".to_string(),
        event_type: TeamEventType::TaskCreated,
        agent_id: "agent-1".to_string(),
        payload: serde_json::Value::Null,
    }).await.unwrap();

    let pruned = store.prune_events("team-1", chrono::Duration::zero()).await.unwrap();
    assert_eq!(pruned, 1);

    let events = store.get_events("team-1", None, None).await.unwrap();
    assert_eq!(events.len(), 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib test_log_and_query_events`
Expected: FAIL

- [ ] **Step 3: Implement TeamEvent types and SqliteEventLogStore**

Create `src/teams/events.rs` with:
- `TeamEventType` enum (MessageSent, MessageRead, TaskCreated, TaskCompleted, TaskFailed, ArtifactSubmitted, ReviewScoreSubmitted, SessionStarted, SessionConcluded, DigestGenerated) with `as_str()`/`from_stored()` pattern
- `TeamEvent` struct (id, team_id, event_type, agent_id, payload: Value, timestamp: DateTime<Utc>)
- `NewTeamEvent` input struct
- `EventLogStore` trait: `log_event()`, `get_events(team_id, since, until)`, `prune_events(team_id, max_age)`
- `SqliteEventLogStore` implementation with `team_events` table

- [ ] **Step 4: Add module and run tests**

Add `pub mod events;` to `src/teams/mod.rs`.

Run: `cargo test -p alephcore --lib test_log_and_query_events test_prune_old_events`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "teams: add event log system with retention policy"
```

---

## Task 4: task_submit and task_read_artifact Tools

**Files:**
- Create: `src/builtin_tools/team/task_submit.rs`
- Create: `src/builtin_tools/team/task_read_artifact.rs`
- Modify: `src/builtin_tools/team/mod.rs`
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/builder.rs`
- Modify: `src/executor/builtin_registry/groups.rs`

- [ ] **Step 1: Implement TaskSubmitTool**

Create `src/builtin_tools/team/task_submit.rs`:

```rust
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskSubmitArgs {
    /// The task ID this artifact belongs to
    pub task_id: String,
    /// Type of artifact
    pub artifact_type: ArtifactType,
    /// Short title for the artifact
    pub title: String,
    /// Full content (markdown)
    pub content: String,
    /// Optional structured metadata (scores, tags, etc.)
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct TaskSubmitOutput {
    pub artifact_id: String,
    pub task_id: String,
    pub message: String,
}

#[derive(Clone)]
pub struct TaskSubmitTool {
    store: Arc<dyn ArtifactStore>,
    pub current_agent_id: String,
}

impl TaskSubmitTool {
    pub fn new(store: Arc<dyn ArtifactStore>, current_agent_id: String) -> Self {
        Self { store, current_agent_id }
    }
}

#[async_trait]
impl AlephTool for TaskSubmitTool {
    const NAME: &'static str = "task_submit";
    const DESCRIPTION: &'static str = "Submit a structured artifact (report, code, review, discovery, challenge) as the output of a task";

    type Args = TaskSubmitArgs;
    type Output = TaskSubmitOutput;

    async fn call(&self, args: Self::Args) -> crate::error::Result<Self::Output> {
        let artifact = self.store.create_artifact(NewArtifact {
            task_id: args.task_id.clone(),
            agent_id: self.current_agent_id.clone(),
            artifact_type: args.artifact_type,
            title: args.title,
            content: args.content,
            metadata: args.metadata,
        }).await?;

        Ok(TaskSubmitOutput {
            artifact_id: artifact.id,
            task_id: args.task_id,
            message: "Artifact submitted successfully".to_string(),
        })
    }
}
```

- [ ] **Step 2: Implement TaskReadArtifactTool**

Create `src/builtin_tools/team/task_read_artifact.rs` following the same pattern. Args: `task_id: String, artifact_id: Option<String>`. If artifact_id provided, return single artifact; otherwise return all artifacts for the task.

- [ ] **Step 3: Export tools from team/mod.rs**

Add `pub mod task_submit; pub mod task_read_artifact;` and re-export the tool structs.

- [ ] **Step 4: Register in builtin registry**

In `definitions.rs`, add two `BuiltinToolDefinition` entries for `"task_submit"` and `"task_read_artifact"` with `requires_config: true`.

In `registry.rs`, add fields:
```rust
pub(crate) task_submit_tool: Option<TaskSubmitTool>,
pub(crate) task_read_artifact_tool: Option<TaskReadArtifactTool>,
```

In `builder.rs`, instantiate when `artifact_store` is available.

In `groups.rs`, add `"task_submit"`, `"task_read_artifact"` to the `"team"` category.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "teams: add task_submit and task_read_artifact tools"
```

---

## Task 5: Wire Artifact Persistence into TeamDelegateTool

**Files:**
- Modify: `src/builtin_tools/team/delegate.rs`

- [ ] **Step 1: Add artifact_store to TeamDelegateTool**

Add `artifact_store: Option<Arc<dyn ArtifactStore>>` field. In `delegate.rs`'s `call()` method, find the `DelegateStatus::Completed` arm (around line 248-337 where `last_message` is extracted from the agent session response). After the existing `coord_store.update_task()` call that marks the task as completed, add artifact persistence:

```rust
// Insert AFTER the update_task() call in the Completed branch:
if let Some(ref artifact_store) = self.artifact_store {
    let _ = artifact_store.create_artifact(NewArtifact {
        task_id: task.id.clone(),
        agent_id: args.agent_id.clone(),
        artifact_type: ArtifactType::Report,
        title: format!("Delegation result: {}", args.task),
        content: last_message.clone(),
        metadata: serde_json::Value::Null,
    }).await;
}
```

- [ ] **Step 2: Update builder to pass artifact_store**

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "teams: auto-persist delegation results as artifacts"
```

---

## Task 6: Message Types and Store

**Files:**
- Create: `src/teams/messages/mod.rs`
- Create: `src/teams/messages/types.rs`
- Create: `src/teams/messages/store.rs`
- Modify: `src/teams/mod.rs`

- [ ] **Step 1: Write tests for message store**

```rust
#[tokio::test]
async fn test_send_and_read_message() {
    let store = SqliteMessageStore::new_in_memory().await.unwrap();
    let msg = store.send_message(NewMessage {
        team_id: "team-1".to_string(),
        from_agent: "agent-a".to_string(),
        msg_type: MessageType::Message,
        subject: "Hello".to_string(),
        content: "How are you?".to_string(),
        recipients: vec![
            Recipient { agent_id: "agent-b".to_string(), role: RecipientRole::To },
            Recipient { agent_id: "agent-c".to_string(), role: RecipientRole::Cc },
        ],
        reply_to: None,
        attachments: vec![],
    }).await.unwrap();

    assert!(msg.thread_id.is_some());
    assert_eq!(msg.thread_id.as_ref().unwrap(), &msg.id); // first msg = thread root

    let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].subject, "Hello");
}

#[tokio::test]
async fn test_thread_continuation() {
    let store = SqliteMessageStore::new_in_memory().await.unwrap();
    let msg1 = store.send_message(NewMessage {
        team_id: "team-1".to_string(),
        from_agent: "agent-a".to_string(),
        msg_type: MessageType::Message,
        subject: "Topic".to_string(),
        content: "First".to_string(),
        recipients: vec![Recipient { agent_id: "agent-b".to_string(), role: RecipientRole::To }],
        reply_to: None,
        attachments: vec![],
    }).await.unwrap();

    let msg2 = store.send_message(NewMessage {
        team_id: "team-1".to_string(),
        from_agent: "agent-b".to_string(),
        msg_type: MessageType::Message,
        subject: "Re: Topic".to_string(),
        content: "Reply".to_string(),
        recipients: vec![Recipient { agent_id: "agent-a".to_string(), role: RecipientRole::To }],
        reply_to: Some(msg1.id.clone()),
        attachments: vec![],
    }).await.unwrap();

    assert_eq!(msg2.thread_id, msg1.thread_id); // same thread

    let thread = store.read_thread(&msg1.thread_id.unwrap()).await.unwrap();
    assert_eq!(thread.len(), 2);
}

#[tokio::test]
async fn test_message_expiry() {
    let store = SqliteMessageStore::new_in_memory().await.unwrap();
    // Send message with very short TTL (already expired)
    let msg = store.send_message_with_ttl(NewMessage {
        team_id: "team-1".to_string(),
        from_agent: "agent-a".to_string(),
        msg_type: MessageType::SystemNotification,
        subject: "Expired".to_string(),
        content: "old".to_string(),
        recipients: vec![Recipient { agent_id: "agent-b".to_string(), role: RecipientRole::To }],
        reply_to: None,
        attachments: vec![],
    }, chrono::Duration::zero()).await.unwrap();

    let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
    assert_eq!(inbox.len(), 0); // expired messages filtered out
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib test_send_and_read_message`
Expected: FAIL

- [ ] **Step 3: Implement message types**

Create `src/teams/messages/types.rs` with all types from spec:
- `TeamMessage`, `Recipient`, `RecipientRole` (To/Cc), `MessageType` (Message, Discovery, Challenge, ReviewRequest, ReviewResult, SystemNotification, Idle, PlanApprovalRequest, PlanApproved, PlanRejected)
- `NewMessage` input struct
- `InboxFilter` for read queries

- [ ] **Step 4: Implement SqliteMessageStore**

Create `src/teams/messages/store.rs`:
- 3 tables: `team_messages`, `message_recipients`, `message_attachments` (per spec SQL schema)
- Thread ID logic: if `reply_to` is set, look up the replied message's `thread_id`; otherwise `thread_id = id`
- TTL computation: `expires_at = created_at + ttl` where TTL depends on recipient roles (to: 2h, cc: 30m, system: 15m)
- `read_inbox()` filters by `expires_at > now` and `read_at IS NULL`
- `mark_read()` sets `read_at` on `message_recipients`
- `read_thread()` returns all messages with given `thread_id` ordered by `created_at`

- [ ] **Step 5: Wire up mod.rs**

Create `src/teams/messages/mod.rs` with re-exports.
Add `pub mod messages;` to `src/teams/mod.rs`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib test_send_and_read_message test_thread_continuation test_message_expiry`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "teams: add message types and SQLite message store with threading"
```

---

## Task 7: MessageRouter — Send, TTL, Escalation Check

**Files:**
- Create: `src/teams/messages/router.rs`
- Create: `src/teams/messages/inbox.rs`

- [ ] **Step 1: Write tests for router**

```rust
#[tokio::test]
async fn test_router_sends_and_logs_event() {
    let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await.unwrap());
    let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await.unwrap());
    let router = MessageRouter::new(msg_store.clone(), event_store.clone(), Default::default());

    router.send(SendRequest {
        team_id: "team-1".to_string(),
        from_agent: "agent-a".to_string(),
        to: vec!["agent-b".to_string()],
        cc: vec!["agent-c".to_string()],
        msg_type: MessageType::Message,
        subject: "Test".to_string(),
        content: "Hello".to_string(),
        reply_to: None,
        attachments: vec![],
    }).await.unwrap();

    // Verify message delivered
    let inbox = msg_store.read_inbox("agent-b", "team-1", None).await.unwrap();
    assert_eq!(inbox.len(), 1);

    // Verify event logged
    let events = event_store.get_events("team-1", None, None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].event_type, TeamEventType::MessageSent));
}

#[tokio::test]
async fn test_escalation_suggestion() {
    let msg_store = Arc::new(SqliteMessageStore::new_in_memory().await.unwrap());
    let event_store = Arc::new(SqliteEventLogStore::new_in_memory().await.unwrap());
    let rules = EscalationRule { thread_message_threshold: 3, review_reject_threshold: 2, enabled: true };
    let router = MessageRouter::new(msg_store.clone(), event_store.clone(), rules);

    // Send 3 messages in same thread to trigger suggestion
    let msg1 = router.send(SendRequest {
        team_id: "team-1".into(), from_agent: "agent-a".into(),
        to: vec!["agent-b".into()], cc: vec![], msg_type: MessageType::Message,
        subject: "Topic".into(), content: "First".into(), reply_to: None, attachments: vec![],
    }).await.unwrap();

    let _ = router.send(SendRequest {
        team_id: "team-1".into(), from_agent: "agent-b".into(),
        to: vec!["agent-a".into()], cc: vec![], msg_type: MessageType::Message,
        subject: "Re: Topic".into(), content: "Reply".into(),
        reply_to: Some(msg1.id.clone()), attachments: vec![],
    }).await.unwrap();

    let _ = router.send(SendRequest {
        team_id: "team-1".into(), from_agent: "agent-a".into(),
        to: vec!["agent-b".into()], cc: vec![], msg_type: MessageType::Message,
        subject: "Re: Topic".into(), content: "Third".into(),
        reply_to: Some(msg1.id.clone()), attachments: vec![],
    }).await.unwrap();

    // After 3rd message (threshold=3), check that a SystemNotification was sent to leader
    // Router needs leader_id in its config (passed at construction time)
    let leader_inbox = msg_store.read_inbox("leader-1", "team-1", None).await.unwrap();
    let notifications: Vec<_> = leader_inbox.iter()
        .filter(|m| matches!(m.msg_type, MessageType::SystemNotification))
        .collect();
    assert_eq!(notifications.len(), 1);
    assert!(notifications[0].content.contains("consider starting a collaborative session"));
}
```

- [ ] **Step 2: Implement MessageRouter**

`src/teams/messages/router.rs`:
- `MessageRouter` struct with `msg_store`, `event_store`, `escalation_rules`
- `send()` method: creates message via store, logs event, checks escalation rules
- Escalation check: count messages in thread, if >= threshold, send `SystemNotification` to team leader
- `SendRequest` input struct (higher-level than `NewMessage` — separates `to` and `cc` into separate fields)

- [ ] **Step 3: Implement Inbox helper**

`src/teams/messages/inbox.rs`:
- `Inbox` struct wrapping `msg_store`
- `read()`: read unread messages for agent (with optional filter)
- `read_thread()`: read full thread
- `mark_read()`: mark messages as read
- `get_unread_counts()`: return `(unread_to, unread_cc)` for context injection

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib test_router`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "teams: add MessageRouter with TTL, escalation suggestions, and inbox helper"
```

---

## Task 8: message_send and inbox_read Tools

**Files:**
- Create: `src/builtin_tools/team/message_send.rs`
- Create: `src/builtin_tools/team/inbox_read.rs`
- Modify: `src/builtin_tools/team/mod.rs`
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/builder.rs`
- Modify: `src/executor/builtin_registry/groups.rs`

- [ ] **Step 1: Implement MessageSendTool**

`src/builtin_tools/team/message_send.rs`:

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct MessageSendArgs {
    /// Team to send within
    pub team_id: String,
    /// Agents who must process/respond
    pub to: Vec<String>,
    /// Agents to inform (optional)
    #[serde(default)]
    pub cc: Vec<String>,
    /// Message type
    #[serde(default = "default_msg_type")]
    pub msg_type: MessageType,
    /// Short subject line
    pub subject: String,
    /// Full message content (markdown)
    pub content: String,
    /// Reply to a specific message ID (continues thread)
    pub reply_to: Option<String>,
    /// Artifact IDs to attach
    #[serde(default)]
    pub attachments: Vec<String>,
}
```

Impl `AlephTool` with `NAME = "message_send"`. The tool calls `router.send()`.

- [ ] **Step 2: Implement InboxReadTool**

`src/builtin_tools/team/inbox_read.rs`:

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct InboxReadArgs {
    /// Team to read from
    pub team_id: String,
    /// Mode: "inbox" (default) or "thread"
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Required when mode="thread" — the thread_id to read
    pub thread_id: Option<String>,
    /// Filter by message type
    pub msg_type: Option<MessageType>,
    /// Only unread messages (default: true)
    #[serde(default = "default_true")]
    pub unread_only: bool,
    /// Mark messages as read after returning (default: true)
    #[serde(default = "default_true")]
    pub mark_read: bool,
}
```

When `mode = "inbox"`: calls `inbox.read()`. When `mode = "thread"`: calls `inbox.read_thread()`.

- [ ] **Step 3: Register tools and compile**

Add to definitions, registry, builder, groups per the standard pattern.

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "teams: add message_send and inbox_read tools"
```

---

## Task 9: team_digest Tool

**Files:**
- Create: `src/builtin_tools/team/team_digest.rs`
- Modify: registration files

- [ ] **Step 1: Implement TeamDigestTool**

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct TeamDigestArgs {
    /// Team to generate digest for
    pub team_id: String,
    /// Hours to look back (default: 24)
    #[serde(default = "default_24")]
    pub hours: u64,
}

#[derive(Debug, Serialize)]
pub struct TeamDigestOutput {
    pub team_id: String,
    pub period_start: String,
    pub period_end: String,
    pub event_count: usize,
    /// Raw events summary for LLM to process
    pub events_summary: String,
    pub message: String,
}
```

The tool reads events from the event log for the specified period and returns a structured summary. The **LLM** (not the tool) generates the natural language digest from the raw events. This keeps intelligence in the prompt (R10).

If the caller is the team leader (check via `team_store.get_team(team_id)` → compare `leader_id` with `current_agent_id`), the tool also broadcasts the digest as a `SystemNotification` to all team members via cc using `router.send()`. The tool needs `team_store: Arc<dyn TeamStore>` and `router: Arc<MessageRouter>` as dependencies.

- [ ] **Step 2: Register and compile**

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "teams: add team_digest tool"
```

---

## Task 10: ContextInjector Integration

**Files:**
- Create: `src/teams/context.rs`
- Modify: `src/agents/swarm/context_injector.rs`

- [ ] **Step 1: Create InboxContext type**

`src/teams/context.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct InboxContext {
    pub unread_to: u32,
    pub unread_cc: u32,
    pub urgent_summary: String,
    // NOTE: active_sessions field will be added in Task 13 after SessionStore exists
}

impl InboxContext {
    pub fn to_injection_text(&self) -> Option<String> {
        if self.unread_to == 0 && self.unread_cc == 0 && self.active_sessions.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if self.unread_to > 0 {
            parts.push(format!("{} unread messages requiring your action", self.unread_to));
        }
        if self.unread_cc > 0 {
            parts.push(format!("{} informational messages (cc)", self.unread_cc));
        }
        if !self.urgent_summary.is_empty() {
            parts.push(self.urgent_summary.clone());
        }
        // active_sessions check will be added in Task 13
        Some(format!("[Team Inbox] {}\nUse inbox_read to view details.", parts.join("; ")))
    }
}
```

- [ ] **Step 2: Extend ContextInjector**

In `src/agents/swarm/context_injector.rs`:
- Add `inbox_provider: Option<Arc<dyn InboxContextProvider>>` field
- Define `InboxContextProvider` trait: `async fn get_inbox_context(agent_id: &str) -> InboxContext`
- In `inject_swarm_state()`, append inbox context text if available

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "teams: integrate inbox context into ContextInjector"
```

---

## Task 11: CollaborativeSession Types + Store

**Files:**
- Create: `src/teams/sessions/mod.rs`
- Create: `src/teams/sessions/types.rs`
- Create: `src/teams/sessions/store.rs`
- Modify: `src/teams/mod.rs`

- [ ] **Step 1: Write tests for session store**

```rust
#[tokio::test]
async fn test_create_session_and_add_turns() {
    let store = SqliteSessionStore::new_in_memory().await.unwrap();
    let session = store.create_session(NewSession {
        team_id: "team-1".to_string(),
        participants: vec!["agent-a".to_string(), "agent-b".to_string()],
        topic: "Design review".to_string(),
        trigger: SessionTrigger::Explicit { requested_by: "agent-a".to_string() },
        thread_id: None,
        max_rounds: 10,
    }).await.unwrap();

    assert!(matches!(session.status, SessionStatus::Active));
    assert_eq!(session.participants.len(), 2);

    store.add_turn(&session.id, SessionTurn {
        agent_id: "agent-a".to_string(),
        content: "I think we should...".to_string(),
        turn_number: 1,
        timestamp: Utc::now(),
    }).await.unwrap();

    let updated = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(updated.transcript.len(), 1);
}

#[tokio::test]
async fn test_conclude_session() {
    let store = SqliteSessionStore::new_in_memory().await.unwrap();
    let session = store.create_session(/* ... */).await.unwrap();

    store.conclude_session(&session.id, SessionOutcome {
        conclusion: "We agreed to use approach B".to_string(),
        agreed_by: vec!["agent-a".to_string(), "agent-b".to_string()],
        dissent: None,
    }).await.unwrap();

    let updated = store.get_session(&session.id).await.unwrap().unwrap();
    assert!(matches!(updated.status, SessionStatus::Concluded));
    assert!(updated.outcome.is_some());
}

#[tokio::test]
async fn test_max_rounds_enforcement() {
    let store = SqliteSessionStore::new_in_memory().await.unwrap();
    let session = store.create_session(NewSession {
        max_rounds: 2,
        // ...
    }).await.unwrap();

    store.add_turn(&session.id, /* turn 1 */).await.unwrap();
    store.add_turn(&session.id, /* turn 2 */).await.unwrap();

    let result = store.add_turn(&session.id, /* turn 3 */).await;
    assert!(result.is_err()); // exceeds max_rounds
}
```

- [ ] **Step 2: Run tests to verify they fail**

- [ ] **Step 3: Implement session types**

`src/teams/sessions/types.rs`: All types from spec — `CollaborativeSession`, `SessionTurn`, `SessionOutcome`, `SessionTrigger` (Explicit/AutoEscalation), `SessionStatus` (Active/Concluded/Deadlocked/Cancelled), `EscalationRule`, `NewSession`.

- [ ] **Step 4: Implement SqliteSessionStore**

`src/teams/sessions/store.rs`:
- Tables: `collaborative_sessions` (id, team_id, topic, trigger_json, thread_id, max_rounds, status, outcome_json, created_at), `session_participants` (session_id, agent_id), `session_turns` (session_id, agent_id, content, turn_number, timestamp)
- `create_session()`, `get_session()`, `add_turn()` (checks max_rounds), `conclude_session()`, `cancel_session()`, `list_active_sessions(team_id)`

- [ ] **Step 5: Wire up mod.rs and run tests**

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "teams: add CollaborativeSession types and SQLite store"
```

---

## Task 12: Session Coordinator Logic

**Files:**
- Create: `src/teams/sessions/coordinator.rs`

- [ ] **Step 1: Implement SessionCoordinator**

This is a thin helper (not an active process — per spec, leader orchestrates via tools):

```rust
pub struct SessionCoordinator {
    session_store: Arc<dyn SessionStore>,
    msg_router: Arc<MessageRouter>,
    event_store: Arc<dyn EventLogStore>,
    artifact_store: Arc<dyn ArtifactStore>,
}

impl SessionCoordinator {
    /// Create session and notify participants via message
    pub async fn start_session(&self, input: NewSession, leader_id: &str) -> Result<CollaborativeSession>;

    /// Add a turn (respond or conclude). If mode=conclude, sets outcome.
    /// If turn_number == max_rounds, sends "final round" notification.
    pub async fn submit_turn(&self, session_id: &str, agent_id: &str, content: &str, mode: TurnMode) -> Result<()>;

    /// Finalize session: save outcome, archive transcript as artifact
    pub async fn finalize(&self, session_id: &str, outcome: SessionOutcome) -> Result<()>;

    /// Cancel session
    pub async fn cancel(&self, session_id: &str) -> Result<()>;
}
```

- [ ] **Step 2: Run tests**

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "teams: add SessionCoordinator helper"
```

---

## Task 13: Session Tools (session_collaborate, session_turn, session_read)

**Files:**
- Create: `src/builtin_tools/team/session_collaborate.rs`
- Create: `src/builtin_tools/team/session_turn.rs`
- Create: `src/builtin_tools/team/session_read.rs`
- Modify: registration files

- [ ] **Step 1: Implement SessionCollaborateTool**

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SessionCollaborateArgs {
    pub team_id: String,
    pub participants: Vec<String>,
    pub topic: String,
    #[serde(default = "default_10")]
    pub max_rounds: u32,
    /// Optional thread_id to inherit from L2 escalation
    pub thread_id: Option<String>,
}
```

Calls `coordinator.start_session()`.

- [ ] **Step 2: Implement SessionTurnTool**

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SessionTurnArgs {
    pub session_id: String,
    pub content: String,
    /// "respond" (default) or "conclude"
    #[serde(default = "default_respond")]
    pub mode: String,
    /// Required when mode="conclude" — the proposed conclusion
    pub conclusion: Option<String>,
    /// Required when mode="conclude" — agents who agree
    pub agreed_by: Option<Vec<String>>,
    /// Optional dissent note
    pub dissent: Option<String>,
}
```

Calls `coordinator.submit_turn()`.

- [ ] **Step 3: Implement SessionReadTool**

Args: `session_id`. Returns session metadata, transcript, and outcome if concluded.

- [ ] **Step 3.5: Add active_sessions to InboxContext**

Now that `SessionStore` exists, go back to `src/teams/context.rs` and add:
- `pub active_sessions: Vec<String>` field to `InboxContext`
- Active sessions check in `to_injection_text()`
- Update `ContextInjector` to query `session_store.list_active_sessions(team_id)` when building InboxContext

- [ ] **Step 4: Register all 3 tools**

Add to definitions, registry, builder, groups. Tool names: `"session_collaborate"`, `"session_turn"`, `"session_read"`.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "teams: add session_collaborate, session_turn, session_read tools"
```

---

## Task 14: Escalation Check in MessageRouter

**Files:**
- Modify: `src/teams/messages/router.rs`

- [ ] **Step 1: Write test for escalation suggestion**

```rust
#[tokio::test]
async fn test_escalation_sends_notification_to_leader() {
    // Setup router with threshold = 3
    // Send 3 messages in same thread
    // Verify a SystemNotification was sent to leader suggesting collaborative session
}
```

- [ ] **Step 2: Implement escalation check in router.send()**

After each `send()`, count messages in the thread. If count >= `escalation_rules.thread_message_threshold` and no notification has been sent for this thread yet:
- Look up team leader from TeamStore
- Send `SystemNotification` to leader with suggestion text

- [ ] **Step 3: Run tests**

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "teams: add escalation suggestion check to MessageRouter"
```

---

## Task 15: Role Types and Configuration

**Files:**
- Create: `src/teams/roles/mod.rs`
- Create: `src/teams/roles/types.rs`
- Create: `src/teams/roles/review.rs`
- Modify: `src/teams/mod.rs`

- [ ] **Step 1: Implement role types**

`src/teams/roles/types.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Leader,
    Explorer,
    Critic,
    Worker,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRoleConfig {
    pub role: AgentRole,
    pub prompt_template: String,
    pub review_dimensions: Vec<String>,
    #[serde(default = "default_7")]
    pub min_score_threshold: u8,
    #[serde(default = "default_3")]
    pub min_challenges: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    Major,
    Minor,
}
```

- [ ] **Step 2: Implement review types and validation**

`src/teams/roles/review.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewScore {
    pub task_id: String,
    pub artifact_id: String,
    pub scores: Vec<DimensionScore>,
    pub overall_pass: bool,
    pub challenges: Vec<Challenge>,
    pub improvement_suggestions: Vec<String>,
    pub risks_if_accepted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DimensionScore {
    pub dimension: String,
    pub score: u8,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Challenge {
    pub point: String,
    pub severity: Severity,
    pub evidence: String,
}

impl ReviewScore {
    /// Validate against role config thresholds
    pub fn validate(&self, config: &TeamRoleConfig) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.challenges.len() < config.min_challenges as usize {
            errors.push(format!(
                "Minimum {} challenges required, got {}",
                config.min_challenges, self.challenges.len()
            ));
        }

        if self.overall_pass {
            for score in &self.scores {
                if score.score < config.min_score_threshold {
                    errors.push(format!(
                        "Cannot pass: dimension '{}' scored {}/{}, minimum is {}",
                        score.dimension, score.score, 10, config.min_score_threshold
                    ));
                }
            }
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}
```

- [ ] **Step 3: Write validation tests**

```rust
#[test]
fn test_review_score_rejects_insufficient_challenges() {
    let config = TeamRoleConfig {
        role: AgentRole::Critic,
        prompt_template: String::new(),
        review_dimensions: vec!["credibility".into(), "logic".into()],
        min_score_threshold: 7,
        min_challenges: 3,
    };

    let score = ReviewScore {
        task_id: "t1".into(),
        artifact_id: "a1".into(),
        scores: vec![DimensionScore { dimension: "credibility".into(), score: 8, rationale: "good".into() }],
        overall_pass: false,
        challenges: vec![Challenge { point: "one".into(), severity: Severity::Major, evidence: "ev".into() }],
        improvement_suggestions: vec![],
        risks_if_accepted: vec![],
    };

    let result = score.validate(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err()[0].contains("Minimum 3 challenges"));
}

#[test]
fn test_review_score_rejects_pass_with_low_scores() {
    let config = TeamRoleConfig { min_score_threshold: 7, min_challenges: 1, ..default_config() };

    let score = ReviewScore {
        overall_pass: true,
        scores: vec![
            DimensionScore { dimension: "credibility".into(), score: 8, rationale: "ok".into() },
            DimensionScore { dimension: "logic".into(), score: 5, rationale: "weak".into() },
        ],
        challenges: vec![Challenge { point: "p".into(), severity: Severity::Minor, evidence: "e".into() }],
        ..default_score()
    };

    let result = score.validate(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err()[0].contains("logic"));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib test_review_score`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "teams: add role types, review scoring, and validation logic"
```

---

## Task 16: review_score Tool

**Files:**
- Create: `src/builtin_tools/team/review_score.rs`
- Modify: registration files

- [ ] **Step 1: Implement ReviewScoreTool**

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReviewScoreArgs {
    pub team_id: String,
    pub task_id: String,
    pub artifact_id: String,
    pub scores: Vec<DimensionScore>,
    pub overall_pass: bool,
    pub challenges: Vec<Challenge>,
    #[serde(default)]
    pub improvement_suggestions: Vec<String>,
    #[serde(default)]
    pub risks_if_accepted: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReviewScoreOutput {
    pub accepted: bool,
    pub validation_errors: Vec<String>,
    pub review_id: String,  // artifact ID of the saved review
}
```

The tool:
1. Builds `ReviewScore` from args
2. Looks up `TeamRoleConfig` for the calling agent's role in this team
3. Calls `score.validate(&config)` — if errors, returns `accepted: false` with errors
4. If valid, saves as `TaskArtifact` (type: Review) and logs `ReviewScoreSubmitted` event
5. Sends `ReviewResult` message to relevant agents

- [ ] **Step 2: Register tool**

Add `"review_score"` to definitions, registry, builder, groups.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "teams: add review_score tool with configurable validation"
```

---

## Task 17: Disbandment Cleanup + Missing Tests

**Files:**
- Modify: `src/builtin_tools/team/disband.rs`
- Various test files

- [ ] **Step 1: Add cleanup to TeamDisbandTool**

In `src/builtin_tools/team/disband.rs`, after marking team as disbanded:
- Call `msg_store.expire_all_for_team(team_id)` to mark all pending messages as expired
- Call `session_store.cancel_all_for_team(team_id)` to cancel active collaborative sessions
- Call `event_store.prune_events(team_id, Duration::hours(24))` to clean old events

Add `msg_store`, `session_store`, `event_store` as optional fields (they may not exist in Phase 1).

- [ ] **Step 2: Add mark-as-read test**

In `src/teams/messages/store.rs` tests:
```rust
#[tokio::test]
async fn test_inbox_read_marks_as_read() {
    let store = SqliteMessageStore::new_in_memory().await.unwrap();
    // Send message to agent-b
    store.send_message(/* to agent-b */).await.unwrap();

    // First read: should see 1 unread
    let inbox = store.read_inbox("agent-b", "team-1", None).await.unwrap();
    assert_eq!(inbox.len(), 1);

    // Mark as read
    store.mark_read(&inbox[0].id, "agent-b").await.unwrap();

    // Second read (unread_only): should see 0
    let inbox2 = store.read_inbox_unread("agent-b", "team-1").await.unwrap();
    assert_eq!(inbox2.len(), 0);
}
```

- [ ] **Step 3: Add message_send tool validation test**

In `src/builtin_tools/team/message_send.rs` tests:
```rust
#[tokio::test]
async fn test_message_send_requires_team_membership() {
    // Setup tool with a team where agent-a is NOT a member
    // Call message_send as agent-a
    // Expect error: "agent not a member of this team"
}
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "teams: add disbandment cleanup and missing test coverage"
```

---

## Task 18: Update Tool Categories and Final Integration

**Files:**
- Modify: `src/executor/builtin_registry/groups.rs`

- [ ] **Step 1: Update the "team" category**

Replace the `team` category in `TOOL_CATEGORIES`:

```rust
ToolCategory {
    id: "team",
    name: "团队协调",
    tools: &[
        // Team management
        "team_create", "team_delegate", "team_status", "team_disband",
        // Task coordination
        "task_create", "task_update", "task_list", "task_wait",
        // Artifacts
        "task_submit", "task_read_artifact",
        // Messaging
        "message_send", "inbox_read", "team_digest",
        // Collaborative sessions
        "session_collaborate", "session_turn", "session_read",
        // Review
        "review_score",
    ],
},
```

- [ ] **Step 2: Run the groups test to verify all tools are covered**

Run: `cargo test -p alephcore --lib test_all_builtin_tools_have_a_group test_no_duplicate_tools_across_groups`
Expected: PASS

- [ ] **Step 3: Full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "teams: update tool categories for teams evolution"
```

---

## Task 19: End-to-End Verification

- [ ] **Step 1: Compile full project**

Run: `cargo check -p alephcore`
Expected: PASS with no warnings related to new code.

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all`
Expected: No new warnings.

- [ ] **Step 4: Final commit**

```bash
git add -A && git commit -m "teams: teams evolution complete — three-layer communication + role mechanism"
```
