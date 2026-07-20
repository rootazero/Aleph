# Task Coordination System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add task DAG coordination, team management, and leader-worker prompt templates to Aleph's swarm module.

**Architecture:** Extend `src/agents/swarm/` with a `tasks/` submodule containing data models, SQLite store, DAG queries, and template parser. Expose 8 new builtin tools (`task_create/update/list/wait`, `team_create/launch/list/disband`). Integrate with Event Bus for unlock notifications and Context Injector for prompt injection.

**Tech Stack:** Rust, SQLite (rusqlite), tokio (select! for task_wait), serde/serde_json, schemars (JSON Schema for tool args)

**Spec:** `docs/superpowers/specs/2026-03-21-task-coordination-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|----------------|
| `src/agents/swarm/tasks/mod.rs` | Data models (`CoordTask`, `CoordTaskStatus`, `Team`, etc.), `CoordTaskStore` trait, type aliases |
| `src/agents/swarm/tasks/store.rs` | `SqliteCoordTaskStore` — all SQLite CRUD, migration, derived Blocked status |
| `src/agents/swarm/tasks/dag.rs` | Cycle detection BFS, `get_newly_unblocked` query helper |
| `src/agents/swarm/tasks/template.rs` | Template parser (Markdown + YAML frontmatter), variable substitution, key→TaskId resolution |
| `src/builtin_tools/task_manage/mod.rs` | Module exports for task tools |
| `src/builtin_tools/task_manage/create.rs` | `TaskCreateTool` |
| `src/builtin_tools/task_manage/update.rs` | `TaskUpdateTool` (triggers DAG unlock + Event Bus) |
| `src/builtin_tools/task_manage/list.rs` | `TaskListTool` |
| `src/builtin_tools/task_manage/wait.rs` | `TaskWaitTool` (select! on Event Bus + timeout) |
| `src/builtin_tools/team_manage/mod.rs` | Module exports for team tools |
| `src/builtin_tools/team_manage/create.rs` | `TeamCreateTool` |
| `src/builtin_tools/team_manage/launch.rs` | `TeamLaunchTool` (template → agents + tasks, rollback) |
| `src/builtin_tools/team_manage/list.rs` | `TeamListTool` |
| `src/builtin_tools/team_manage/disband.rs` | `TeamDisbandTool` |

### Modified files

| File | Changes |
|------|---------|
| `src/agents/swarm/mod.rs` | Add `pub mod tasks;` |
| `src/agents/swarm/events.rs` | +4 `ImportantEvent` variants |
| `src/agents/swarm/coordinator.rs` | Add `CoordTaskStore` to `SwarmCoordinator`, pass to `ContextInjector` |
| `src/agents/swarm/context_injector.rs` | Add `Option<Arc<dyn CoordTaskStore>>` field, `inject_task_context()` method |
| `src/builtin_tools/mod.rs` | Add `pub mod task_manage; pub mod team_manage;` |
| `src/executor/builtin_registry/config.rs` | Add `coord_task_store: Option<Arc<dyn CoordTaskStore>>` to `BuiltinToolConfig` |
| `src/executor/builtin_registry/definitions.rs` | +8 entries in `BUILTIN_TOOL_DEFINITIONS`, +8 arms in `create_tool_boxed` |
| `src/executor/builtin_registry/groups.rs` | +1 `ToolGroup` entry for `task_coordination` |
| `src/executor/builtin_registry/builder.rs` | Instantiate 8 tools from config, register metadata |
| `src/executor/builtin_registry/registry.rs` | +8 fields, +8 `execute_tool` match arms |

---

## Task 1: Data Models + CoordTaskStore Trait

**Files:**
- Create: `src/agents/swarm/tasks/mod.rs`
- Modify: `src/agents/swarm/mod.rs`

- [ ] **Step 1: Create `tasks/mod.rs` with all data types**

```rust
// src/agents/swarm/tasks/mod.rs
pub mod dag;
pub mod store;
pub mod template;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::AlephError;
type Result<T> = std::result::Result<T, AlephError>;

// ---- ID newtypes ----

pub type CoordTaskId = String;
pub type TeamId = String;
pub type AgentId = String;

// ---- CoordTask ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordTask {
    pub id: CoordTaskId,
    pub team_id: Option<TeamId>,
    pub subject: String,
    pub description: String,
    pub status: CoordTaskStatus,
    pub owner: Option<AgentId>,
    pub priority: Priority,
    pub result: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Original dependencies (immutable after creation, for display/audit)
    pub dependencies: Vec<CoordTaskId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordTaskStatus {
    Pending,
    Blocked,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl CoordTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Blocked => "blocked",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "blocked" => Some(Self::Blocked),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for Priority {
    fn default() -> Self { Self::Normal }
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

// ---- Team ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader: AgentId,
    pub members: Vec<TeamMember>,
    pub status: TeamStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub agent_id: AgentId,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    Active,
    Completed,
    Disbanded,
}

impl TeamStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Disbanded => "disbanded",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "completed" => Some(Self::Completed),
            "disbanded" => Some(Self::Disbanded),
            _ => None,
        }
    }
}

// ---- Input/Filter types ----

#[derive(Debug, Clone)]
pub struct NewCoordTask {
    pub team_id: Option<TeamId>,
    pub subject: String,
    pub description: String,
    pub owner: Option<AgentId>,
    pub priority: Priority,
    pub blocked_by: Vec<CoordTaskId>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct CoordTaskUpdate {
    pub status: Option<CoordTaskStatus>,
    pub owner: Option<AgentId>,
    pub result: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct CoordTaskFilter {
    pub team_id: Option<TeamId>,
    pub status: Option<CoordTaskStatus>,
    pub owner: Option<AgentId>,
}

#[derive(Debug, Clone)]
pub struct NewTeam {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader: AgentId,
}

#[derive(Debug, Clone, Default)]
pub struct TeamUpdate {
    pub status: Option<TeamStatus>,
}

#[derive(Debug, Clone, Default)]
pub struct TeamFilter {}

// ---- Trait ----

#[async_trait::async_trait]
pub trait CoordTaskStore: Send + Sync {
    async fn create_task(&self, task: NewCoordTask) -> Result<CoordTask>;
    async fn get_task(&self, id: &str) -> Result<Option<CoordTask>>;
    async fn update_task(&self, id: &str, update: CoordTaskUpdate) -> Result<CoordTask>;
    async fn list_tasks(&self, filter: CoordTaskFilter) -> Result<Vec<CoordTask>>;

    async fn get_dependencies(&self, id: &str) -> Result<Vec<CoordTaskId>>;
    async fn get_dependents(&self, id: &str) -> Result<Vec<CoordTaskId>>;
    async fn get_newly_unblocked(&self, completed_id: &str) -> Result<Vec<CoordTask>>;

    async fn create_team(&self, team: NewTeam) -> Result<Team>;
    async fn get_team(&self, id: &str) -> Result<Option<Team>>;
    async fn update_team(&self, id: &str, update: TeamUpdate) -> Result<Team>;
    async fn list_teams(&self, filter: TeamFilter) -> Result<Vec<Team>>;
    async fn add_member(&self, team_id: &str, member: TeamMember) -> Result<()>;
    async fn remove_member(&self, team_id: &str, agent_id: &str) -> Result<()>;

    /// Find all teams where agent_id is leader or member
    async fn get_agent_teams(&self, agent_id: &str) -> Result<Vec<Team>>;
}
```

- [ ] **Step 2: Add `pub mod tasks;` to swarm/mod.rs**

Add `pub mod tasks;` to `src/agents/swarm/mod.rs`.

- [ ] **Step 3: Create stub files for store, dag, template**

Create empty stub files so the module compiles:
- `src/agents/swarm/tasks/store.rs` — `// TODO: SqliteCoordTaskStore`
- `src/agents/swarm/tasks/dag.rs` — `// TODO: cycle detection`
- `src/agents/swarm/tasks/template.rs` — `// TODO: template parser`

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: Compiles (or only pre-existing warnings)

- [ ] **Step 5: Commit**

```bash
git add src/agents/swarm/tasks/ src/agents/swarm/mod.rs
git commit -m "swarm: add CoordTask data models and CoordTaskStore trait"
```

---

## Task 2: SQLite Store Implementation

**Files:**
- Create: `src/agents/swarm/tasks/store.rs`

- [ ] **Step 1: Implement `SqliteCoordTaskStore` with migration**

```rust
// src/agents/swarm/tasks/store.rs
use std::sync::Arc;
use rusqlite::Connection;
use tokio::sync::Mutex;
use chrono::Utc;
use uuid::Uuid;

use super::*;
use crate::error::AlephError;

type Result<T> = std::result::Result<T, AlephError>;

pub struct SqliteCoordTaskStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteCoordTaskStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self> {
        let store = Self { conn };
        // Run migration synchronously during construction
        // (called from async context via block_in_place or at startup)
        store
    }

    pub async fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS coord_teams (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                leader TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS coord_tasks (
                id TEXT PRIMARY KEY,
                team_id TEXT,
                subject TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                owner TEXT,
                priority TEXT NOT NULL DEFAULT 'normal',
                result TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                FOREIGN KEY (team_id) REFERENCES coord_teams(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS coord_task_dependencies (
                task_id TEXT NOT NULL,
                depends_on TEXT NOT NULL,
                PRIMARY KEY (task_id, depends_on),
                FOREIGN KEY (task_id) REFERENCES coord_tasks(id) ON DELETE CASCADE,
                FOREIGN KEY (depends_on) REFERENCES coord_tasks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS coord_team_members (
                team_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT '',
                joined_at TEXT NOT NULL,
                PRIMARY KEY (team_id, agent_id),
                FOREIGN KEY (team_id) REFERENCES coord_teams(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_coord_tasks_team_status ON coord_tasks(team_id, status);
            CREATE INDEX IF NOT EXISTS idx_coord_tasks_owner ON coord_tasks(owner);"
        )?;
        Ok(())
    }

    /// Derive effective status: stored 'pending' with unresolved deps → Blocked
    fn derive_status(stored_status: &str, has_unresolved_deps: bool) -> CoordTaskStatus {
        if stored_status == "pending" && has_unresolved_deps {
            CoordTaskStatus::Blocked
        } else {
            CoordTaskStatus::from_str(stored_status).unwrap_or(CoordTaskStatus::Pending)
        }
    }
}
```

Implement all trait methods. Key queries:

**`create_task`**: INSERT into coord_tasks + INSERT into coord_task_dependencies for each blocked_by. If blocked_by is non-empty, stored status = 'pending' (Blocked is derived).

**`list_tasks`**: LEFT JOIN with a subquery to check for unresolved deps:
```sql
SELECT t.*,
  EXISTS(
    SELECT 1 FROM coord_task_dependencies d
    JOIN coord_tasks dep ON dep.id = d.depends_on
    WHERE d.task_id = t.id AND dep.status != 'completed'
  ) AS has_unresolved_deps
FROM coord_tasks t
WHERE (?1 IS NULL OR t.team_id = ?1)
  AND (?2 IS NULL OR t.owner = ?2)
```
Then apply `derive_status()` and filter by requested status if any.

**`get_newly_unblocked`**: The spec's SQL query from the DAG section.

**`get_agent_teams`**: Query coord_teams JOIN coord_team_members WHERE agent_id = ? OR leader = ?

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

- [ ] **Step 3: Write unit tests**

Test in the same file or a `#[cfg(test)]` module:
- `test_create_and_get_task` — create task, get by id, verify fields
- `test_list_tasks_with_filter` — create tasks with different owners/teams, filter
- `test_derived_blocked_status` — create task with blocked_by, verify it shows as Blocked; complete dependency, verify it shows as Pending
- `test_get_newly_unblocked` — create A→B→C chain, complete A, verify B unblocked; complete B, verify C unblocked
- `test_create_and_list_teams` — CRUD cycle for teams and members

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib swarm::tasks::store 2>&1 | tail -20`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add src/agents/swarm/tasks/store.rs
git commit -m "swarm: implement SqliteCoordTaskStore with derived Blocked status"
```

---

## Task 3: DAG Cycle Detection

**Files:**
- Create: `src/agents/swarm/tasks/dag.rs`

- [ ] **Step 1: Implement BFS cycle detection**

```rust
// src/agents/swarm/tasks/dag.rs
use std::collections::{HashSet, VecDeque};
use super::CoordTaskStore;
use crate::error::AlephError;

type Result<T> = std::result::Result<T, AlephError>;

/// Check if adding edges `new_task_id ← blocked_by[..]` would create a cycle.
/// Returns Err if cycle detected.
pub async fn check_no_cycle(
    store: &dyn CoordTaskStore,
    new_task_id: &str,
    blocked_by: &[String],
) -> Result<()> {
    if blocked_by.is_empty() {
        return Ok(());
    }

    // BFS from each blocked_by task, following reverse edges (dependents).
    // If we reach new_task_id, there's a cycle.
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    for dep_id in blocked_by {
        queue.push_back(dep_id.clone());
    }

    while let Some(current) = queue.pop_front() {
        if current == new_task_id {
            return Err(AlephError::validation(format!(
                "Adding dependencies would create a cycle involving task '{}'",
                new_task_id
            )));
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        // Get what `current` depends on (its own blocked_by)
        let deps = store.get_dependencies(&current).await?;
        for dep in deps {
            queue.push_back(dep);
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Write tests**

```rust
#[cfg(test)]
mod tests {
    // test_no_cycle_simple — A→B, no cycle
    // test_cycle_detected — A→B→C, try to add C→A, detect cycle
    // test_self_dependency — A→A, detect cycle
    // test_empty_blocked_by — always Ok
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib swarm::tasks::dag 2>&1 | tail -20`
Expected: All pass

- [ ] **Step 4: Integrate cycle check into `SqliteCoordTaskStore::create_task`**

In `store.rs`, call `dag::check_no_cycle(self, &new_id, &task.blocked_by).await?` before the INSERT.

- [ ] **Step 5: Verify + commit**

```bash
cargo test -p alephcore --lib swarm::tasks 2>&1 | tail -20
git add src/agents/swarm/tasks/dag.rs src/agents/swarm/tasks/store.rs
git commit -m "swarm: add DAG cycle detection for task dependencies"
```

---

## Task 4: Event Bus Integration

**Files:**
- Modify: `src/agents/swarm/events.rs`

- [ ] **Step 1: Add 4 new ImportantEvent variants**

Add to the existing `ImportantEvent` enum (match existing convention with timestamp field):

```rust
TaskCompleted {
    task_id: String,
    agent_id: String,
    timestamp: u64,
},
TaskUnblocked {
    task_id: String,
    unblocked_by: String,
    timestamp: u64,
},
TaskFailed {
    task_id: String,
    reason: String,
    timestamp: u64,
},
AllTasksCompleted {
    team_id: String,
    timestamp: u64,
},
```

- [ ] **Step 2: Update any `match` arms on `ImportantEvent`**

Search for exhaustive matches on `ImportantEvent` and add arms for the new variants (likely in `context_injector.rs` and `rules.rs`).

Run: `cargo check -p alephcore 2>&1 | head -30` — fix any match exhaustiveness errors.

- [ ] **Step 3: Commit**

```bash
git add src/agents/swarm/events.rs src/agents/swarm/context_injector.rs src/agents/swarm/rules.rs
git commit -m "swarm: add task coordination events to ImportantEvent"
```

---

## Task 5: Template Parser

**Files:**
- Create: `src/agents/swarm/tasks/template.rs`

- [ ] **Step 1: Define template data structures**

```rust
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct TeamTemplate {
    pub name: String,
    pub description: String,
    pub leader: String,
    pub members: Vec<TemplateMember>,
    pub tasks: Vec<TemplateTask>,
    #[serde(default)]
    pub variables: Vec<TemplateVariable>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateMember {
    pub name: String,
    pub role: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateTask {
    pub key: String,
    pub subject: String,
    pub owner: String,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub description: String,
}

pub struct ParsedTemplate {
    pub frontmatter: TeamTemplate,
    pub body: String,
}
```

- [ ] **Step 2: Implement parsing + variable substitution**

```rust
pub fn parse_template(content: &str) -> Result<ParsedTemplate> {
    // Split on "---" fences, parse YAML frontmatter, keep body
}

pub fn substitute_variables(
    template: &str,
    variables: &std::collections::HashMap<String, String>,
) -> String {
    // Replace {key} with value for each variable
}
```

- [ ] **Step 3: Implement template file discovery**

```rust
pub fn find_template(name: &str) -> Result<String> {
    // Search order: ~/.aleph/templates/{name}.md → built-in
    // Plugin templates use namespace: "plugin:name"
}
```

- [ ] **Step 4: Write tests**

- `test_parse_template` — parse the example template from spec, verify all fields
- `test_substitute_variables` — verify {goal}, {agent_name} substitution
- `test_key_resolution` — verify blocked_by keys map correctly

- [ ] **Step 5: Run tests + commit**

```bash
cargo test -p alephcore --lib swarm::tasks::template 2>&1 | tail -20
git add src/agents/swarm/tasks/template.rs
git commit -m "swarm: add team template parser with key-based dependency resolution"
```

---

## Task 6: Task Builtin Tools (task_create, task_update, task_list, task_wait)

**Files:**
- Create: `src/builtin_tools/task_manage/mod.rs`
- Create: `src/builtin_tools/task_manage/create.rs`
- Create: `src/builtin_tools/task_manage/update.rs`
- Create: `src/builtin_tools/task_manage/list.rs`
- Create: `src/builtin_tools/task_manage/wait.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Create `task_manage/mod.rs`**

```rust
pub mod create;
pub mod update;
pub mod list;
pub mod wait;

pub use create::TaskCreateTool;
pub use update::TaskUpdateTool;
pub use list::TaskListTool;
pub use wait::TaskWaitTool;
```

Add `pub mod task_manage;` to `src/builtin_tools/mod.rs`.

- [ ] **Step 2: Implement `TaskCreateTool`**

Follow the `AgentCreateTool` pattern:
- Struct holds `Arc<dyn CoordTaskStore>`
- Args: `subject, description?, team_id?, owner?, blocked_by: Vec<String>, priority?, metadata?`
- Calls `store.create_task(NewCoordTask { ... })`
- Returns created task summary

- [ ] **Step 3: Implement `TaskUpdateTool`**

- Struct holds `Arc<dyn CoordTaskStore>` + `Arc<AgentMessageBus>` (for event publishing)
- Args: `task_id, status?, owner?, result?, metadata?`
- Calls `store.update_task()`
- **If status == Completed**: call `store.get_newly_unblocked()`, publish `TaskCompleted` + `TaskUnblocked` events to bus
- **If status == Failed**: publish `TaskFailed` event
- Check if all team tasks completed → publish `AllTasksCompleted`

- [ ] **Step 4: Implement `TaskListTool`**

- Struct holds `Arc<dyn CoordTaskStore>`
- Args: `team_id?, status?, owner?`
- Calls `store.list_tasks(filter)`
- Format output as readable task board with status, owner, dependencies

- [ ] **Step 5: Implement `TaskWaitTool`**

- Struct holds `Arc<dyn CoordTaskStore>` + `Arc<AgentMessageBus>`
- Args: `task_ids: Option<Vec<String>>, team_id: Option<String>, timeout_seconds: Option<u64>`
- Implementation: `select!` loop per spec (check → wait for event → recheck → timeout)
- On timeout: return current status (not error)

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/task_manage/ src/builtin_tools/mod.rs
git commit -m "tools: add task_create, task_update, task_list, task_wait builtin tools"
```

---

## Task 7: Team Builtin Tools (team_create, team_launch, team_list, team_disband)

**Files:**
- Create: `src/builtin_tools/team_manage/mod.rs`
- Create: `src/builtin_tools/team_manage/create.rs`
- Create: `src/builtin_tools/team_manage/launch.rs`
- Create: `src/builtin_tools/team_manage/list.rs`
- Create: `src/builtin_tools/team_manage/disband.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Create `team_manage/mod.rs`**

```rust
pub mod create;
pub mod launch;
pub mod list;
pub mod disband;

pub use create::TeamCreateTool;
pub use launch::TeamLaunchTool;
pub use list::TeamListTool;
pub use disband::TeamDisbandTool;
```

Add `pub mod team_manage;` to `src/builtin_tools/mod.rs`.

- [ ] **Step 2: Implement `TeamCreateTool`**

- Struct holds `Arc<dyn CoordTaskStore>`
- Args: `name, description?, leader`
- ID = slugify(name)
- Calls `store.create_team(NewTeam { ... })`

- [ ] **Step 3: Implement `TeamLaunchTool`**

This is the most complex tool. Struct holds: `Arc<dyn CoordTaskStore>`, `Arc<AgentRegistry>`, `Arc<AgentEnvStore>`, `Option<Arc<AgentManager>>`, `Arc<AgentMessageBus>`.

- Args: `template: String, name?: String, variables?: HashMap<String, String>`
- Steps:
  1. `template::find_template(&args.template)` → content
  2. `template::parse_template(&content)` → ParsedTemplate
  3. Substitute variables (merge built-in + user-provided)
  4. `store.create_team()` → Team
  5. For each member: create agent via `AgentCreateTool` logic (reuse internals, not the tool itself). On failure → rollback (delete created agents + team)
  6. `store.add_member()` for each
  7. Create all tasks in order, resolving key→CoordTaskId mapping. Use a `HashMap<String, CoordTaskId>` for key resolution.
  8. Publish TeamCreated event
  9. Return team overview

- [ ] **Step 4: Implement `TeamListTool`**

- Struct holds `Arc<dyn CoordTaskStore>`
- No args
- Calls `store.list_teams(TeamFilter {})`
- Format: team name, status, member count, task progress (X/Y completed)

- [ ] **Step 5: Implement `TeamDisbandTool`**

- Struct holds `Arc<dyn CoordTaskStore>`, `Arc<AgentRegistry>`, `Arc<AgentEnvStore>`, `Option<Arc<AgentManager>>`
- Args: `team_id: String, archive: Option<bool>`
- Steps:
  1. Cancel all non-completed tasks: `store.list_tasks(team_id)` → for each pending/in_progress/blocked, `store.update_task(status=Cancelled)`
  2. `store.update_team(status=Disbanded)`
  3. If `archive == true`: for each member, call agent_delete logic (archive workspace)

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/team_manage/ src/builtin_tools/mod.rs
git commit -m "tools: add team_create, team_launch, team_list, team_disband builtin tools"
```

---

## Task 8: Registry Integration

**Files:**
- Modify: `src/executor/builtin_registry/config.rs`
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/groups.rs`
- Modify: `src/executor/builtin_registry/builder.rs`
- Modify: `src/executor/builtin_registry/registry.rs`

- [ ] **Step 1: Add `coord_task_store` to `BuiltinToolConfig`**

In `config.rs`, add:
```rust
pub coord_task_store: Option<Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>>,
```

- [ ] **Step 2: Add 8 tool definitions to `BUILTIN_TOOL_DEFINITIONS`**

In `definitions.rs`, add entries:
```rust
BuiltinToolDefinition { name: "task_create", description: "Create a coordination task...", requires_config: true },
BuiltinToolDefinition { name: "task_update", description: "Update task status...", requires_config: true },
BuiltinToolDefinition { name: "task_list", description: "List coordination tasks...", requires_config: true },
BuiltinToolDefinition { name: "task_wait", description: "Wait for tasks to complete...", requires_config: true },
BuiltinToolDefinition { name: "team_create", description: "Create a coordination team...", requires_config: true },
BuiltinToolDefinition { name: "team_launch", description: "Launch a team from template...", requires_config: true },
BuiltinToolDefinition { name: "team_list", description: "List all teams...", requires_config: true },
BuiltinToolDefinition { name: "team_disband", description: "Disband a team...", requires_config: true },
```

Add match arms in `create_tool_boxed` returning `None` (dynamic creation only, like agent tools).

- [ ] **Step 3: Add tool group to `TOOL_GROUPS`**

In `groups.rs`:
```rust
ToolGroup {
    id: "task_coordination",
    name: "任务协调",
    tools: &[
        "task_create", "task_update", "task_list", "task_wait",
        "team_create", "team_launch", "team_list", "team_disband",
    ],
},
```

- [ ] **Step 4: Add tool fields and instantiation in builder.rs**

Follow the agent_manage pattern: conditionally create tools if `coord_task_store` is present in config. Register metadata (UnifiedTool with parameters_schema).

- [ ] **Step 5: Add `execute_tool` match arms in registry.rs**

Add 8 match arms in the `execute_tool` method, following the existing pattern:
```rust
"task_create" => Box::pin(async move {
    let tool = self.task_create_tool.as_ref().ok_or_else(|| ...)?;
    tool.call_json(arguments).await
}),
```

- [ ] **Step 6: Verify all tests pass**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All pass including `test_all_builtin_tools_have_a_group`

- [ ] **Step 7: Commit**

```bash
git add src/executor/builtin_registry/
git commit -m "registry: wire 8 task coordination tools into builtin registry"
```

---

## Task 9: Context Injector Integration

**Files:**
- Modify: `src/agents/swarm/context_injector.rs`
- Modify: `src/agents/swarm/coordinator.rs`

- [ ] **Step 1: Add `CoordTaskStore` to `ContextInjector`**

Add field:
```rust
task_store: Option<Arc<dyn CoordTaskStore>>,
```

Update constructor to accept optional store. Add builder method:
```rust
pub fn with_task_store(mut self, store: Arc<dyn CoordTaskStore>) -> Self {
    self.task_store = Some(store);
    self
}
```

- [ ] **Step 2: Implement `inject_task_context`**

```rust
pub async fn inject_task_context(&self, agent_id: &str) -> Option<String> {
    let store = self.task_store.as_ref()?;
    let teams = store.get_agent_teams(agent_id).await.ok()?;
    if teams.is_empty() { return None; }

    let mut sections = Vec::new();
    for team in &teams {
        let is_leader = team.leader == agent_id;
        let tasks = store.list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        }).await.ok()?;

        if is_leader {
            sections.push(format_leader_prompt(team, &tasks));
        } else {
            let my_tasks: Vec<_> = tasks.iter()
                .filter(|t| t.owner.as_deref() == Some(agent_id))
                .collect();
            sections.push(format_member_prompt(team, agent_id, &my_tasks));
        }
    }
    Some(sections.join("\n\n"))
}
```

- [ ] **Step 3: Wire into `inject_swarm_state`**

In the existing `inject_swarm_state` method, append task context after swarm events:
```rust
if let Some(task_ctx) = self.inject_task_context(agent_id).await {
    output.push_str("\n\n");
    output.push_str(&task_ctx);
}
```

- [ ] **Step 4: Update `SwarmCoordinator` to pass store**

In `coordinator.rs`, add `CoordTaskStore` field. Update constructor and `start()` to pass store to injector:
```rust
pub async fn with_task_store(mut self, store: Arc<dyn CoordTaskStore>) -> Self {
    self.task_store = Some(Arc::clone(&store));
    self.injector = Arc::new(
        ContextInjector::new(Arc::clone(&self.bus))
            .with_task_store(store)
    );
    self
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

- [ ] **Step 6: Commit**

```bash
git add src/agents/swarm/context_injector.rs src/agents/swarm/coordinator.rs
git commit -m "swarm: inject task coordination context into agent prompts"
```

---

## Task 10: Server Startup Wiring

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs` (or wherever SwarmCoordinator is initialized)

- [ ] **Step 1: Find where SwarmCoordinator is created at startup**

Search for `SwarmCoordinator::new` or `SwarmCoordinator::with_config` in the server startup code.

- [ ] **Step 2: Create `SqliteCoordTaskStore` at startup and wire it**

At server startup:
1. Create or open SQLite connection for coord tasks (can share the existing state DB or use a separate file like `~/.aleph/data/coord.db`)
2. `let coord_store = Arc::new(SqliteCoordTaskStore::new(conn)?);`
3. `coord_store.migrate().await?;`
4. Pass to `SwarmCoordinator::with_task_store(coord_store.clone())`
5. Pass to `BuiltinToolConfig { coord_task_store: Some(coord_store), ... }`

- [ ] **Step 3: Verify server starts**

Run: `cargo run --bin aleph-server -- start 2>&1 | head -20` (then Ctrl+C)
Expected: Server starts without errors.

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph-server/
git commit -m "server: wire SqliteCoordTaskStore into startup"
```

---

## Task 11: Built-in Team Template

**Files:**
- Create: `~/.aleph/templates/code-review-team.md` (or `core/assets/templates/`)

- [ ] **Step 1: Create the example template from the spec**

Write the code-review-team template from the spec to `~/.aleph/templates/code-review-team.md`.

- [ ] **Step 2: Commit**

```bash
git add -f core/assets/templates/code-review-team.md 2>/dev/null || true
git commit -m "templates: add built-in code-review-team template"
```

---

## Task 12: Integration Test

**Files:**
- This is a manual verification task

- [ ] **Step 1: Full compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -10`

- [ ] **Step 2: All unit tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All pass (including existing tests + new swarm::tasks tests)

- [ ] **Step 3: Clippy**

Run: `cargo clippy -p alephcore 2>&1 | tail -20`
Fix any warnings in new code.

- [ ] **Step 4: Final commit if any clippy fixes**

```bash
git add -A && git commit -m "chore: fix clippy warnings in task coordination"
```
