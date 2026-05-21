# Team Module Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the team module from sub-agent-based to registered-agent teams with SQLite persistence, new tools, and panel UI.

**Architecture:** Replace in-memory TeamStore + sub-agent spawning with SQLite-backed team registry. Four new tools (team_create, team_delegate, team_status, team_disband) replace the old team_manage module. Panel gets agent list dropdown filter, agent Teams tab, and dashboard Teams page.

**Tech Stack:** Rust (core), rusqlite (SQLite), Leptos/WASM (panel), JSON-RPC (panel↔core)

**Spec:** `docs/superpowers/specs/2026-03-26-team-module-refactor-design.md`

**Note on i18n:** Panel code snippets use hardcoded English strings for clarity. The implementor should replace them with `t!(i18n, ...)` calls and add corresponding i18n keys to the locale files, following the existing pattern in `channels.rs`.

**Note on SQLite threading:** `SqliteTeamStore` wraps synchronous `rusqlite::Connection` in `tokio::sync::Mutex`. This is the same pattern used by the existing `SqliteCoordTaskStore`. If performance becomes an issue, consider wrapping DB calls in `tokio::task::spawn_blocking`.

---

## File Structure

### New Files
- `src/teams/mod.rs` — Module root, re-exports
- `src/teams/store.rs` — TeamStore trait + SqliteTeamStore implementation
- `src/teams/types.rs` — Team, TeamMember, TeamTask structs
- `src/builtin_tools/team/mod.rs` — New team tools module root
- `src/builtin_tools/team/create.rs` — team_create tool
- `src/builtin_tools/team/delegate.rs` — team_delegate tool
- `src/builtin_tools/team/status.rs` — team_status tool
- `src/builtin_tools/team/disband.rs` — team_disband tool
- `src/gateway/handlers/teams.rs` — RPC handlers for panel
- `interfaces/webchat/src/views/agents/teams.rs` — Agent edit Teams tab
- `interfaces/webchat/src/views/teams.rs` — Dashboard Teams page
- `interfaces/webchat/src/api/teams.rs` — Teams API client

### Modified Files
- `src/lib.rs` — Add `pub mod teams`
- `src/builtin_tools/mod.rs` — Replace `team_manage` with `team`
- `src/executor/builtin_registry/registry.rs` — Replace team tool fields
- `src/executor/builtin_registry/builder.rs` — Replace team tool registration
- `src/executor/builtin_registry/groups.rs` — Update team category tools list
- `src/agents/swarm/tasks/mod.rs` — Remove Team/TeamMember/NewTeam/TeamFilter/TeamUpdate types and team methods from CoordTaskStore trait
- `src/agents/swarm/tasks/store.rs` — Remove team SQL tables and team method implementations
- `src/agents/sub_agents/run.rs` — Remove `persona`, `keep_alive` fields
- `src/bin/aleph-server/commands/start/builder/agent_init.rs` — Init TeamStore alongside CoordTaskStore
- `src/bin/aleph-server/commands/start/builder/handlers.rs` — Register teams.* RPC handlers
- `src/gateway/handlers/mod.rs` — Add `pub mod teams` + placeholder registrations
- `interfaces/webchat/src/views/agents/mod.rs` — Add Teams tab to ALL_TABS
- `interfaces/webchat/src/views/mod.rs` — Add `pub mod teams`
- `interfaces/webchat/src/api/mod.rs` — Add `pub mod teams`
- `interfaces/webchat/src/components/agents_sidebar.rs` — Add dropdown filter
- `interfaces/webchat/src/components/dashboard_sidebar.rs` — Add Teams nav entry

### Removed Files
- `src/builtin_tools/team_manage/mod.rs`
- `src/builtin_tools/team_manage/create.rs`
- `src/builtin_tools/team_manage/launch.rs`
- `src/builtin_tools/team_manage/list.rs`
- `src/builtin_tools/team_manage/disband.rs`

---

## Task 1: SQLite Team Store — Types and Trait

**Files:**
- Create: `src/teams/types.rs`
- Create: `src/teams/mod.rs`
- Create: `src/teams/store.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/teams/types.rs`**

```rust
use serde::{Deserialize, Serialize};

pub type TeamId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: TeamStatus,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamStatus {
    Active,
    Disbanded,
}

impl TeamStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disbanded => "disbanded",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "disbanded" => Some(Self::Disbanded),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub team_id: TeamId,
    pub agent_id: String,
    pub role: String,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamTask {
    pub id: String,
    pub team_id: TeamId,
    pub agent_id: String,
    pub subject: String,
    pub status: TeamTaskStatus,
    pub result: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamTaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl TeamTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Input for creating a new team
#[derive(Debug, Clone)]
pub struct NewTeam {
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub members: Vec<NewTeamMember>,
}

#[derive(Debug, Clone)]
pub struct NewTeamMember {
    pub agent_id: String,
    pub role: String,
}

/// Input for creating a new task record
#[derive(Debug, Clone)]
pub struct NewTeamTask {
    pub team_id: TeamId,
    pub agent_id: String,
    pub subject: String,
}

/// Summary for team listing (avoids loading full details)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSummary {
    pub id: TeamId,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: TeamStatus,
    pub member_count: usize,
    pub task_count: usize,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
}
```

- [ ] **Step 2: Create `src/teams/store.rs`**

```rust
use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use rusqlite::Connection;
use uuid::Uuid;

use super::types::*;

#[async_trait]
pub trait TeamStore: Send + Sync {
    async fn create_team(&self, team: NewTeam) -> Result<Team>;
    async fn get_team(&self, id: &str) -> Result<Option<Team>>;
    async fn list_teams(&self) -> Result<Vec<TeamSummary>>;
    async fn disband_team(&self, id: &str) -> Result<()>;
    async fn delete_team(&self, id: &str) -> Result<()>;

    async fn get_members(&self, team_id: &str) -> Result<Vec<TeamMember>>;
    async fn get_agent_teams(&self, agent_id: &str) -> Result<Vec<TeamSummary>>;

    async fn create_task(&self, task: NewTeamTask) -> Result<TeamTask>;
    async fn update_task_status(&self, task_id: &str, status: TeamTaskStatus, result: Option<String>) -> Result<()>;
    async fn get_tasks(&self, team_id: &str) -> Result<Vec<TeamTask>>;
}

pub struct SqliteTeamStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTeamStore {
    pub fn new(conn: Connection) -> Self {
        Self { conn: Arc::new(Mutex::new(conn)) }
    }

    pub async fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS teams (
                id           TEXT PRIMARY KEY,
                name         TEXT NOT NULL,
                description  TEXT NOT NULL DEFAULT '',
                leader_id    TEXT NOT NULL,
                status       TEXT NOT NULL DEFAULT 'active',
                created_at   INTEGER NOT NULL,
                disbanded_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS team_members (
                team_id   TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
                agent_id  TEXT NOT NULL,
                role      TEXT NOT NULL DEFAULT '',
                joined_at INTEGER NOT NULL,
                PRIMARY KEY (team_id, agent_id)
            );

            CREATE TABLE IF NOT EXISTS team_tasks (
                id           TEXT PRIMARY KEY,
                team_id      TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
                agent_id     TEXT NOT NULL,
                subject      TEXT NOT NULL,
                status       TEXT NOT NULL DEFAULT 'pending',
                result       TEXT,
                created_at   INTEGER NOT NULL,
                completed_at INTEGER
            );
        ")?;
        Ok(())
    }
}

#[async_trait]
impl TeamStore for SqliteTeamStore {
    async fn create_team(&self, input: NewTeam) -> Result<Team> {
        let conn = self.conn.lock().await;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO teams (id, name, description, leader_id, status, created_at) VALUES (?1, ?2, ?3, ?4, 'active', ?5)",
            rusqlite::params![&id, &input.name, &input.description, &input.leader_id, now],
        )?;

        for m in &input.members {
            conn.execute(
                "INSERT INTO team_members (team_id, agent_id, role, joined_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![&id, &m.agent_id, &m.role, now],
            )?;
        }

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

    async fn get_team(&self, id: &str) -> Result<Option<Team>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, leader_id, status, created_at, disbanded_at FROM teams WHERE id = ?1"
        )?;
        let team = stmt.query_row(rusqlite::params![id], |row| {
            Ok(Team {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                leader_id: row.get(3)?,
                status: TeamStatus::from_str(row.get::<_, String>(4)?.as_str()).unwrap_or(TeamStatus::Active),
                created_at: row.get(5)?,
                disbanded_at: row.get(6)?,
            })
        }).optional()?;
        Ok(team)
    }

    async fn list_teams(&self) -> Result<Vec<TeamSummary>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT t.id, t.name, t.description, t.leader_id, t.status, t.created_at, t.disbanded_at,
                    (SELECT COUNT(*) FROM team_members WHERE team_id = t.id) as member_count,
                    (SELECT COUNT(*) FROM team_tasks WHERE team_id = t.id) as task_count
             FROM teams t ORDER BY t.created_at DESC"
        )?;
        let teams = stmt.query_map([], |row| {
            Ok(TeamSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                leader_id: row.get(3)?,
                status: TeamStatus::from_str(row.get::<_, String>(4)?.as_str()).unwrap_or(TeamStatus::Active),
                created_at: row.get(5)?,
                disbanded_at: row.get(6)?,
                member_count: row.get(7)?,
                task_count: row.get(8)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(teams)
    }

    async fn disband_team(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE teams SET status = 'disbanded', disbanded_at = ?1 WHERE id = ?2 AND status = 'active'",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    async fn delete_team(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        // Only allow deleting disbanded teams
        let status: String = conn.query_row(
            "SELECT status FROM teams WHERE id = ?1", rusqlite::params![id],
            |row| row.get(0),
        )?;
        if status != "disbanded" {
            anyhow::bail!("Can only delete disbanded teams");
        }
        conn.execute("DELETE FROM teams WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    async fn get_members(&self, team_id: &str) -> Result<Vec<TeamMember>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT team_id, agent_id, role, joined_at FROM team_members WHERE team_id = ?1"
        )?;
        let members = stmt.query_map(rusqlite::params![team_id], |row| {
            Ok(TeamMember {
                team_id: row.get(0)?,
                agent_id: row.get(1)?,
                role: row.get(2)?,
                joined_at: row.get(3)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(members)
    }

    async fn get_agent_teams(&self, agent_id: &str) -> Result<Vec<TeamSummary>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT t.id, t.name, t.description, t.leader_id, t.status, t.created_at, t.disbanded_at,
                    (SELECT COUNT(*) FROM team_members WHERE team_id = t.id) as member_count,
                    (SELECT COUNT(*) FROM team_tasks WHERE team_id = t.id) as task_count
             FROM teams t
             JOIN team_members tm ON t.id = tm.team_id
             WHERE tm.agent_id = ?1
             ORDER BY t.created_at DESC"
        )?;
        let teams = stmt.query_map(rusqlite::params![agent_id], |row| {
            Ok(TeamSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                leader_id: row.get(3)?,
                status: TeamStatus::from_str(row.get::<_, String>(4)?.as_str()).unwrap_or(TeamStatus::Active),
                created_at: row.get(5)?,
                disbanded_at: row.get(6)?,
                member_count: row.get(7)?,
                task_count: row.get(8)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(teams)
    }

    async fn create_task(&self, input: NewTeamTask) -> Result<TeamTask> {
        let conn = self.conn.lock().await;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO team_tasks (id, team_id, agent_id, subject, status, created_at) VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
            rusqlite::params![&id, &input.team_id, &input.agent_id, &input.subject, now],
        )?;

        Ok(TeamTask {
            id,
            team_id: input.team_id,
            agent_id: input.agent_id,
            subject: input.subject,
            status: TeamTaskStatus::Running,
            result: None,
            created_at: now,
            completed_at: None,
        })
    }

    async fn update_task_status(&self, task_id: &str, status: TeamTaskStatus, result: Option<String>) -> Result<()> {
        let conn = self.conn.lock().await;
        let completed_at = if matches!(status, TeamTaskStatus::Completed | TeamTaskStatus::Failed) {
            Some(chrono::Utc::now().timestamp())
        } else {
            None
        };
        conn.execute(
            "UPDATE team_tasks SET status = ?1, result = ?2, completed_at = ?3 WHERE id = ?4",
            rusqlite::params![status.as_str(), result, completed_at, task_id],
        )?;
        Ok(())
    }

    async fn get_tasks(&self, team_id: &str) -> Result<Vec<TeamTask>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, team_id, agent_id, subject, status, result, created_at, completed_at
             FROM team_tasks WHERE team_id = ?1 ORDER BY created_at DESC"
        )?;
        let tasks = stmt.query_map(rusqlite::params![team_id], |row| {
            Ok(TeamTask {
                id: row.get(0)?,
                team_id: row.get(1)?,
                agent_id: row.get(2)?,
                subject: row.get(3)?,
                status: TeamTaskStatus::from_str(row.get::<_, String>(4)?.as_str()).unwrap_or(TeamTaskStatus::Pending),
                result: row.get(5)?,
                created_at: row.get(6)?,
                completed_at: row.get(7)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }
}
```

- [ ] **Step 3: Create `src/teams/mod.rs`**

```rust
pub mod types;
pub mod store;

pub use types::*;
pub use store::{TeamStore, SqliteTeamStore};
```

- [ ] **Step 4: Add `pub mod teams` to `src/lib.rs`**

Find where other top-level modules are declared and add `pub mod teams;`.

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (new module compiles, no references to it yet)

- [ ] **Step 6: Commit**

```bash
git add src/teams/
git commit -m "teams: add SQLite team store with types and trait"
```

---

## Task 2: Remove Old Team Module and Clean Up

**Files:**
- Delete: `src/builtin_tools/team_manage/` (all files)
- Modify: `src/builtin_tools/mod.rs` — Remove `pub mod team_manage` and re-exports
- Modify: `src/agents/swarm/tasks/mod.rs` — Remove Team types, keep CoordTask types
- Modify: `src/agents/swarm/tasks/store.rs` — Remove team SQL tables and implementations
- Modify: `src/executor/builtin_registry/registry.rs` — Remove old team tool fields
- Modify: `src/executor/builtin_registry/builder.rs` — Remove old team tool instantiation
- Modify: `src/executor/builtin_registry/groups.rs` — Remove old team tools from category
- Modify: `src/agents/sub_agents/run.rs` — Remove `persona`, `keep_alive` fields
- Modify: `src/gateway/handlers/mod.rs` — Remove team tool placeholder registrations (if any)

**Important:** This task will temporarily break compilation because the old tools are removed before new ones are added. That's expected — Task 3 will wire the new tools.

- [ ] **Step 1: Delete old team_manage module**

```bash
rm -rf src/builtin_tools/team_manage/
```

- [ ] **Step 2: Update `src/builtin_tools/mod.rs`**

Remove the line `pub mod team_manage;` and the re-export line `pub use team_manage::{TeamCreateTool, TeamDisbandTool, TeamLaunchTool, TeamListTool};`.

- [ ] **Step 3: Remove Team types from `src/agents/swarm/tasks/mod.rs`**

Remove these structs/types (keep CoordTask, CoordTaskStatus, CoordTaskFilter, CoordTaskUpdate, NewCoordTask and all task methods on the trait):
- `Team` struct
- `TeamMember` struct
- `TeamStatus` enum
- `NewTeam` struct
- `TeamFilter` struct
- `TeamUpdate` struct

Remove team methods from `CoordTaskStore` trait:
- `create_team`
- `get_team`
- `update_team`
- `list_teams`
- `add_member`
- `remove_member`
- `get_agent_teams`

- [ ] **Step 4: Remove team SQL and implementations from `src/agents/swarm/tasks/store.rs`**

Remove from the migration SQL:
- `CREATE TABLE coord_teams` — remove entirely
- `CREATE TABLE coord_team_members` — remove entirely

Remove team method implementations from `impl CoordTaskStore for SqliteCoordTaskStore`.

Update the `coord_tasks` table: remove `FOREIGN KEY (team_id) REFERENCES coord_teams(id)` since `coord_teams` no longer exists. The `team_id` column stays as nullable text (tasks can still optionally belong to a coordination context).

- [ ] **Step 5: Clean up sub-agent `SubAgentRun`**

In `src/agents/sub_agents/run.rs`:
- Remove the `persona: Option<String>` field from `SubAgentRun` (approximately line 256)
- Remove the `keep_alive: bool` field (approximately line 260)
- Remove the `with_persona()` builder method
- Remove the `with_keep_alive()` builder method

Search for all references to these fields/methods and update call sites:
```bash
# Find all references
grep -rn "\.persona\b\|with_persona\|\.keep_alive\|with_keep_alive" src/
```

- [ ] **Step 6: Remove old team tools from builtin registry**

In `src/executor/builtin_registry/registry.rs`:
- Remove fields: `team_create_tool`, `team_launch_tool`, `team_list_tool`, `team_disband_tool`

In `src/executor/builtin_registry/builder.rs`:
- Remove the team tool instantiation block (lines ~321-395 that reference TeamCreateTool, TeamLaunchTool, etc.)
- Keep the task tool instantiation (task_create, task_update, task_list, task_wait)

In `src/executor/builtin_registry/groups.rs`:
- In the "team" ToolCategory (lines 83-89), remove `"team_create", "team_launch", "team_list", "team_disband"` from the tools list. Keep task tools if they're in the same category, or restructure as needed.

- [ ] **Step 7: Fix all compilation errors**

Run: `cargo check -p alephcore 2>&1 | head -80`

Fix each error by removing references to deleted types/tools. Common patterns:
- Remove imports of deleted types
- Remove match arms for deleted tools
- Remove references to deleted registry fields

Iterate until: `cargo check -p alephcore` PASSES

- [ ] **Step 8: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (some existing team tests may need removal)

- [ ] **Step 9: Commit**

```bash
git add src/builtin_tools/ src/agents/ src/executor/
git commit -m "teams: remove old team_manage module and clean up sub-agent fields"
```

---

## Task 3: New Team Tools — team_create and team_disband

**Files:**
- Create: `src/builtin_tools/team/mod.rs`
- Create: `src/builtin_tools/team/create.rs`
- Create: `src/builtin_tools/team/disband.rs`
- Modify: `src/builtin_tools/mod.rs` — Add `pub mod team`

- [ ] **Step 1: Create `src/builtin_tools/team/create.rs`**

Follow the `AlephTool` pattern from the existing codebase. The tool needs access to `TeamStore` (for persistence) and `AgentManager` (for creating new agents on the fly).

```rust
use std::sync::Arc;
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::AlephTool;
use crate::teams::{TeamStore, NewTeam, NewTeamMember, Team};
use crate::AgentManager;
use crate::config::types::agents_def::{AgentDefinition, AgentIdentity};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TeamCreateArgs {
    /// Team name
    pub name: String,
    /// Team description
    #[serde(default)]
    pub description: String,
    /// Team members — each specifies an existing agent_id OR creates a new agent
    pub members: Vec<MemberSpec>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemberSpec {
    /// Existing agent ID (mutually exclusive with `create`)
    pub agent_id: Option<String>,
    /// New agent definition to register (mutually exclusive with `agent_id`)
    pub create: Option<CreateAgentSpec>,
    /// Role description for this member
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateAgentSpec {
    /// Unique agent ID
    pub id: String,
    /// Display name
    pub name: Option<String>,
    /// Model override
    pub model: Option<String>,
    /// Profile reference
    pub profile: Option<String>,
    /// Agent identity
    pub identity: Option<AgentIdentity>,
}

#[derive(Debug, Serialize)]
pub struct TeamCreateOutput {
    pub team_id: String,
    pub name: String,
    pub leader_id: String,
    pub members: Vec<MemberOutput>,
}

#[derive(Debug, Serialize)]
pub struct MemberOutput {
    pub agent_id: String,
    pub role: String,
    pub newly_created: bool,
}

/// **Must derive Clone** — `AlephTool` requires `Clone + Send + Sync + 'static`.
#[derive(Clone)]
pub struct TeamCreateTool {
    store: Arc<dyn TeamStore>,
    agent_manager: Arc<AgentManager>,
    current_agent_id: String,
}

impl TeamCreateTool {
    pub fn new(
        store: Arc<dyn TeamStore>,
        agent_manager: Arc<AgentManager>,
        current_agent_id: String,
    ) -> Self {
        Self { store, agent_manager, current_agent_id }
    }
}

#[async_trait::async_trait]
impl AlephTool for TeamCreateTool {
    const NAME: &'static str = "team_create";
    const DESCRIPTION: &'static str = "Create a new team of registered agents. Members can reference existing agents or create new ones on the fly.";
    type Args = TeamCreateArgs;
    type Output = TeamCreateOutput;

    async fn call(&self, args: Self::Args, _ctx: &ToolContext) -> Result<Self::Output> {
        let mut member_outputs = Vec::new();

        for spec in &args.members {
            let (agent_id, newly_created) = if let Some(ref id) = spec.agent_id {
                // Verify existing agent
                // Use agent_manager to check agent exists — if not, return error
                (id.clone(), false)
            } else if let Some(ref create) = spec.create {
                // Create new agent via agent_manager.create()
                let def = AgentDefinition {
                    id: create.id.clone(),
                    name: create.name.clone(),
                    model: create.model.clone(),
                    profile: create.profile.clone(),
                    identity: create.identity.clone(),
                    ..Default::default()
                };
                self.agent_manager.create(def)?;
                (create.id.clone(), true)
            } else {
                anyhow::bail!("Each member must specify either agent_id or create");
            };

            member_outputs.push(MemberOutput {
                agent_id: agent_id.clone(),
                role: spec.role.clone(),
                newly_created,
            });
        }

        let new_team = NewTeam {
            name: args.name.clone(),
            description: args.description,
            leader_id: self.current_agent_id.clone(),
            members: member_outputs.iter().map(|m| NewTeamMember {
                agent_id: m.agent_id.clone(),
                role: m.role.clone(),
            }).collect(),
        };

        let team = self.store.create_team(new_team).await?;

        Ok(TeamCreateOutput {
            team_id: team.id,
            name: team.name,
            leader_id: team.leader_id,
            members: member_outputs,
        })
    }
}
```

The `execute()` method should:
1. Iterate members: for `agent_id` specs, verify agent exists via `agent_manager`
2. For `create` specs, call `agent_manager.create(AgentDefinition { ... })` to register the new agent
3. Build `NewTeam` with resolved agent_ids
4. Call `store.create_team(new_team)`
5. Return `TeamCreateOutput`

- [ ] **Step 2: Create `src/builtin_tools/team/disband.rs`**

```rust
use std::sync::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::AlephTool;
use crate::teams::TeamStore;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TeamDisbandArgs {
    /// Team ID to disband
    pub team_id: String,
}

#[derive(Debug, Serialize)]
pub struct TeamDisbandOutput {
    pub success: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct TeamDisbandTool {
    store: Arc<dyn TeamStore>,
}

impl TeamDisbandTool {
    pub fn new(store: Arc<dyn TeamStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl AlephTool for TeamDisbandTool {
    const NAME: &'static str = "team_disband";
    const DESCRIPTION: &'static str = "Disband an active team. Member agents are preserved but no longer part of this team.";
    type Args = TeamDisbandArgs;
    type Output = TeamDisbandOutput;

    async fn call(&self, args: Self::Args, _ctx: &ToolContext) -> Result<Self::Output> {
        self.store.disband_team(&args.team_id).await?;
        Ok(TeamDisbandOutput {
            success: true,
            message: format!("Team {} disbanded. Member agents are preserved.", args.team_id),
        })
    }
}
```

- [ ] **Step 3: Create `src/builtin_tools/team/mod.rs`**

```rust
mod create;
mod disband;

pub use create::TeamCreateTool;
pub use disband::TeamDisbandTool;
```

- [ ] **Step 4: Register in `src/builtin_tools/mod.rs`**

Add `pub mod team;` and `pub use team::{TeamCreateTool, TeamDisbandTool};`

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/team/
git commit -m "teams: add team_create and team_disband tools"
```

---

## Task 4: New Team Tools — team_delegate and team_status

**Files:**
- Create: `src/builtin_tools/team/delegate.rs`
- Create: `src/builtin_tools/team/status.rs`
- Modify: `src/builtin_tools/team/mod.rs` — Add new exports

- [ ] **Step 1: Create `src/builtin_tools/team/delegate.rs`**

This is the most complex tool. It needs to:
1. Validate membership
2. Create a `TeamTask` record
3. Launch a new agent session (using `ExecutionEngine` or the same mechanism channels use)
4. Wait for completion with timeout
5. Update task record with result

```rust
use std::sync::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::AlephTool;
use crate::teams::{TeamStore, NewTeamTask, TeamTaskStatus};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TeamDelegateArgs {
    /// Team ID
    pub team_id: String,
    /// Target member agent ID
    pub agent_id: String,
    /// Task description/instruction to send to the member
    pub task: String,
    /// Timeout in seconds (default: 300 = 5 minutes)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 { 300 }

#[derive(Debug, Serialize)]
pub struct TeamDelegateOutput {
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    pub result: Option<String>,
}

/// **Key dependency:** Needs `ExecutionEngine` (or equivalent) to run agent sessions.
/// Check `src/gateway/execution_engine/mod.rs` for the `RunRequest` struct.
/// Look at how Telegram/Slack channel handlers submit messages — they create a `RunRequest`
/// with `session_key`, `input`, `run_id` and submit via the engine's run method.
///
/// **Constructor pattern:** The implementor should grep for `RunRequest` usage across
/// channel handlers to find the exact submit+await pattern. The tool needs whatever
/// handle/Arc the channels use to submit run requests.
#[derive(Clone)]
pub struct TeamDelegateTool {
    store: Arc<dyn TeamStore>,
    // Add the execution dependency here — likely Arc<ExecutionEngine> or
    // Arc<dyn RunSubmitter> or whatever interface channels use to submit runs.
    // Example: execution_engine: Arc<ExecutionEngine>,
}

#[async_trait::async_trait]
impl AlephTool for TeamDelegateTool {
    const NAME: &'static str = "team_delegate";
    const DESCRIPTION: &'static str = "Delegate a task to a team member. Launches an independent session for the target agent, sends the task, and returns the result.";
    type Args = TeamDelegateArgs;
    type Output = TeamDelegateOutput;

    async fn call(&self, args: Self::Args, _ctx: &ToolContext) -> Result<Self::Output> {
        // 1. Verify membership
        let members = self.store.get_members(&args.team_id).await?;
        if !members.iter().any(|m| m.agent_id == args.agent_id) {
            anyhow::bail!("Agent {} is not a member of team {}", args.agent_id, args.team_id);
        }

        // 2. Create task record
        let task = self.store.create_task(NewTeamTask {
            team_id: args.team_id.clone(),
            agent_id: args.agent_id.clone(),
            subject: args.task.clone(),
        }).await?;

        // 3. Create and submit RunRequest
        // Pattern from channel handlers:
        //   let run_req = RunRequest {
        //       run_id: uuid::Uuid::new_v4().to_string(),
        //       input: args.task.clone(),
        //       session_key: SessionKey::new(&args.agent_id, /* session_id */),
        //       timeout_secs: Some(args.timeout_secs),
        //       metadata: Default::default(),
        //       attachments: vec![],
        //       pending_media: Default::default(),
        //   };
        //   let result = self.execution_engine.run(run_req).await;

        // 4. Apply timeout wrapper
        let timeout_duration = std::time::Duration::from_secs(args.timeout_secs);
        // let result = tokio::time::timeout(timeout_duration, engine_future).await;

        // 5. Update task record based on result
        // match result {
        //     Ok(Ok(response)) => {
        //         let result_text = response.final_message(); // extract final assistant text
        //         self.store.update_task_status(&task.id, TeamTaskStatus::Completed, Some(result_text.clone())).await?;
        //         Ok(TeamDelegateOutput { task_id: task.id, agent_id: args.agent_id, status: "completed".into(), result: Some(result_text) })
        //     }
        //     Ok(Err(e)) => {
        //         self.store.update_task_status(&task.id, TeamTaskStatus::Failed, Some(e.to_string())).await?;
        //         Ok(TeamDelegateOutput { task_id: task.id, agent_id: args.agent_id, status: "failed".into(), result: Some(e.to_string()) })
        //     }
        //     Err(_timeout) => {
        //         self.store.update_task_status(&task.id, TeamTaskStatus::Failed, Some("Timeout".into())).await?;
        //         Ok(TeamDelegateOutput { task_id: task.id, agent_id: args.agent_id, status: "failed".into(), result: Some("Task timed out".into()) })
        //     }
        // }

        // NOTE TO IMPLEMENTOR: The commented code above shows the exact flow.
        // Fill in the actual ExecutionEngine API calls based on how channel handlers work.
        // Key files to study:
        //   - src/gateway/execution_engine/mod.rs (RunRequest struct, run method)
        //   - Any channel handler that creates RunRequest (grep for "RunRequest" in channels/)
        //   - The session_key format for agent sessions
        todo!("Wire execution engine — see comments above for the exact pattern")
    }
}
```

**Implementation guidance for `team_delegate`:**

The `team_delegate` tool is the most complex piece. The implementor should:

1. **Find the execution submission pattern**: `grep -rn "RunRequest" src/` to find how channels (Telegram, Slack) create and submit run requests
2. **Identify the session key format**: `SessionKey::new()` or `SessionKey::Agent()` — check how agent sessions are keyed
3. **Find the response type**: What does the engine return? Look for the response struct that contains the assistant's final message
4. **Add the execution dependency**: Whatever `Arc<...>` the channels hold to submit runs, the `TeamDelegateTool` needs the same
5. **Wire in builder.rs**: Pass the execution handle when constructing `TeamDelegateTool` in the builtin registry builder

- [ ] **Step 2: Create `src/builtin_tools/team/status.rs`**

```rust
use std::sync::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::AlephTool;
use crate::teams::TeamStore;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TeamStatusArgs {
    /// Team ID
    pub team_id: String,
}

#[derive(Debug, Serialize)]
pub struct TeamStatusOutput {
    pub team_id: String,
    pub name: String,
    pub status: String,
    pub leader_id: String,
    pub members: Vec<MemberInfo>,
    pub tasks: Vec<TaskInfo>,
}

#[derive(Debug, Serialize)]
pub struct MemberInfo {
    pub agent_id: String,
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct TaskInfo {
    pub id: String,
    pub agent_id: String,
    pub subject: String,
    pub status: String,
    pub result: Option<String>,
}

#[derive(Clone)]
pub struct TeamStatusTool {
    store: Arc<dyn TeamStore>,
}

impl TeamStatusTool {
    pub fn new(store: Arc<dyn TeamStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl AlephTool for TeamStatusTool {
    const NAME: &'static str = "team_status";
    const DESCRIPTION: &'static str = "Check the status of a team, including members and task history.";
    type Args = TeamStatusArgs;
    type Output = TeamStatusOutput;

    async fn call(&self, args: Self::Args, _ctx: &ToolContext) -> Result<Self::Output> {
        let team = self.store.get_team(&args.team_id).await?
            .ok_or_else(|| anyhow::anyhow!("Team not found: {}", args.team_id))?;
        let members = self.store.get_members(&args.team_id).await?;
        let tasks = self.store.get_tasks(&args.team_id).await?;

        Ok(TeamStatusOutput {
            team_id: team.id,
            name: team.name,
            status: team.status.as_str().to_string(),
            leader_id: team.leader_id,
            members: members.into_iter().map(|m| MemberInfo {
                agent_id: m.agent_id,
                role: m.role,
            }).collect(),
            tasks: tasks.into_iter().map(|t| TaskInfo {
                id: t.id,
                agent_id: t.agent_id,
                subject: t.subject,
                status: t.status.as_str().to_string(),
                result: t.result,
            }).collect(),
        })
    }
}
```

- [ ] **Step 3: Update `src/builtin_tools/team/mod.rs`**

Add:
```rust
mod delegate;
mod status;

pub use delegate::TeamDelegateTool;
pub use status::TeamStatusTool;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/team/
git commit -m "teams: add team_delegate and team_status tools"
```

---

## Task 5: Wire New Tools into Builtin Registry

**Files:**
- Modify: `src/executor/builtin_registry/registry.rs` — Add new team tool fields
- Modify: `src/executor/builtin_registry/builder.rs` — Instantiate and register new tools
- Modify: `src/executor/builtin_registry/groups.rs` — Update tool category
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` — Initialize SqliteTeamStore

- [ ] **Step 1: Add TeamStore initialization in `agent_init.rs`**

Follow the existing `coord_store` initialization pattern (lines 88-127 in `agent_init.rs`):

```rust
// After coord_store initialization:
let team_store: Option<Arc<dyn alephcore::teams::TeamStore>> = {
    use alephcore::teams::SqliteTeamStore;

    let db_path = get_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("aleph_data"))
        .join("teams.db");

    match rusqlite::Connection::open(&db_path) {
        Ok(conn) => {
            let store = Arc::new(SqliteTeamStore::new(conn));
            let store_clone = Arc::clone(&store);
            match tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(store_clone.migrate())
            }) {
                Ok(()) => Some(store as Arc<dyn alephcore::teams::TeamStore>),
                Err(e) => { eprintln!("Team store migration failed: {}", e); None }
            }
        }
        Err(e) => { eprintln!("Failed to open teams.db: {}", e); None }
    }
};
```

Pass `team_store` to the builtin registry builder config.

- [ ] **Step 2: Add tool fields to registry.rs**

Replace the old team tool fields with:
```rust
pub(crate) team_create_tool: Option<crate::builtin_tools::team::TeamCreateTool>,
pub(crate) team_delegate_tool: Option<crate::builtin_tools::team::TeamDelegateTool>,
pub(crate) team_status_tool: Option<crate::builtin_tools::team::TeamStatusTool>,
pub(crate) team_disband_tool: Option<crate::builtin_tools::team::TeamDisbandTool>,
```

- [ ] **Step 3: Add tool instantiation in builder.rs**

In the builder's build method, instantiate the new tools when TeamStore is available:
```rust
if let Some(ref store) = config.team_store {
    let team_create = TeamCreateTool::new(
        Arc::clone(store),
        Arc::clone(&config.agent_manager),
        agent_id.clone(),
    );
    let team_delegate = TeamDelegateTool::new(Arc::clone(store), /* execution deps */);
    let team_status = TeamStatusTool::new(Arc::clone(store));
    let team_disband = TeamDisbandTool::new(Arc::clone(store));

    // Register parameter schemas
    let defs = vec![
        team_create.definition(),
        team_delegate.definition(),
        team_status.definition(),
        team_disband.definition(),
    ];
    for td in &defs {
        let mut ut = UnifiedTool::new(
            format!("builtin:{}", td.name), &td.name, &td.description, ToolSource::Builtin,
        );
        ut = ut.with_parameters_schema(td.parameters.clone());
        tools.insert(td.name.clone(), ut);
    }
}
```

- [ ] **Step 4: Update tool category in groups.rs**

```rust
ToolCategory {
    id: "team",
    name: "团队协调",
    tools: &[
        "team_create", "team_delegate", "team_status", "team_disband",
        "task_create", "task_update", "task_list", "task_wait",
    ],
},
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore && cargo check --bin aleph-server`
Expected: PASS

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/executor/ src/bin/
git commit -m "teams: wire new team tools into builtin registry"
```

---

## Task 6: RPC Handlers for Panel

**Files:**
- Create: `src/gateway/handlers/teams.rs`
- Modify: `src/gateway/handlers/mod.rs` — Add `pub mod teams` and placeholders
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs` — Wire real handlers

- [ ] **Step 1: Create `src/gateway/handlers/teams.rs`**

Follow the pattern in `src/gateway/handlers/agents.rs`:

```rust
use std::sync::Arc;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;
use crate::teams::TeamStore;

pub async fn handle_list(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    match store.list_teams().await {
        Ok(teams) => JsonRpcResponse::success(request.id, json!({ "teams": teams })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to list teams: {}", e)),
    }
}

pub async fn handle_get(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params { team_id: String }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.get_team(&params.team_id).await {
        Ok(Some(team)) => {
            let members = store.get_members(&params.team_id).await.unwrap_or_default();
            let tasks = store.get_tasks(&params.team_id).await.unwrap_or_default();
            JsonRpcResponse::success(request.id, json!({
                "team": team,
                "members": members,
                "tasks": tasks,
            }))
        }
        Ok(None) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Team not found".to_string()),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to get team: {}", e)),
    }
}

pub async fn handle_disband(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params { team_id: String }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.disband_team(&params.team_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to disband team: {}", e)),
    }
}

pub async fn handle_delete(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params { team_id: String }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.delete_team(&params.team_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{}", e)),
    }
}

pub async fn handle_agent_teams(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params { agent_id: String }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.get_agent_teams(&params.agent_id).await {
        Ok(teams) => JsonRpcResponse::success(request.id, json!({ "teams": teams })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to get agent teams: {}", e)),
    }
}
```

- [ ] **Step 2: Add module and placeholders in `src/gateway/handlers/mod.rs`**

Add `pub mod teams;` to the module declarations.

Add placeholder registrations in the `register_defaults()` function (following the existing pattern):

```rust
registry.register("teams.list", |req| async move {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, "teams.list requires TeamStore — wire in Gateway startup".to_string())
});
registry.register("teams.get", |req| async move {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, "teams.get requires TeamStore — wire in Gateway startup".to_string())
});
registry.register("teams.disband", |req| async move {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, "teams.disband requires TeamStore — wire in Gateway startup".to_string())
});
registry.register("teams.delete", |req| async move {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, "teams.delete requires TeamStore — wire in Gateway startup".to_string())
});
registry.register("agents.teams", |req| async move {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, "agents.teams requires TeamStore — wire in Gateway startup".to_string())
});
```

- [ ] **Step 3: Wire real handlers in `handlers.rs` (bin crate)**

Add a new function in `src/bin/aleph-server/commands/start/builder/handlers.rs`:

```rust
pub(in crate::commands::start) fn register_teams_handlers(
    server: &mut GatewayServer,
    store: &Arc<dyn alephcore::teams::TeamStore>,
) {
    use alephcore::gateway::handlers::teams;

    register_handler!(server, "teams.list", teams::handle_list, store);
    register_handler!(server, "teams.get", teams::handle_get, store);
    register_handler!(server, "teams.disband", teams::handle_disband, store);
    register_handler!(server, "teams.delete", teams::handle_delete, store);
    register_handler!(server, "agents.teams", teams::handle_agent_teams, store);
}
```

Call this function from the main server setup (where other `register_*_handlers` are called), passing the `team_store` Arc.

- [ ] **Step 4: Verify compilation**

Run: `cargo check --bin aleph-server`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/ src/bin/
git commit -m "teams: add RPC handlers for panel (teams.list/get/disband/delete, agents.teams)"
```

---

## Task 7: Panel — Teams API Client

**Files:**
- Create: `interfaces/webchat/src/api/teams.rs`
- Modify: `interfaces/webchat/src/api/mod.rs` — Add `pub mod teams`

- [ ] **Step 1: Create `interfaces/webchat/src/api/teams.rs`**

Follow the pattern in `interfaces/webchat/src/api/agents.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::context::DashboardState;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: String,
    pub member_count: usize,
    pub task_count: usize,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamMember {
    pub team_id: String,
    pub agent_id: String,
    pub role: String,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamTask {
    pub id: String,
    pub team_id: String,
    pub agent_id: String,
    pub subject: String,
    pub status: String,
    pub result: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamDetail {
    pub team: TeamSummary,
    pub members: Vec<TeamMember>,
    pub tasks: Vec<TeamTask>,
}

pub struct TeamsApi;

impl TeamsApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<TeamSummary>, String> {
        let result = state.rpc_call("teams.list", serde_json::Value::Null).await?;
        let teams: Vec<TeamSummary> = serde_json::from_value(
            result.get("teams").cloned().unwrap_or_default()
        ).map_err(|e| e.to_string())?;
        Ok(teams)
    }

    pub async fn get(state: &DashboardState, team_id: &str) -> Result<TeamDetail, String> {
        let result = state.rpc_call("teams.get", json!({ "team_id": team_id })).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn disband(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state.rpc_call("teams.disband", json!({ "team_id": team_id })).await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state.rpc_call("teams.delete", json!({ "team_id": team_id })).await?;
        Ok(())
    }

    pub async fn agent_teams(state: &DashboardState, agent_id: &str) -> Result<Vec<TeamSummary>, String> {
        let result = state.rpc_call("agents.teams", json!({ "agent_id": agent_id })).await?;
        let teams: Vec<TeamSummary> = serde_json::from_value(
            result.get("teams").cloned().unwrap_or_default()
        ).map_err(|e| e.to_string())?;
        Ok(teams)
    }
}
```

- [ ] **Step 2: Add to `interfaces/webchat/src/api/mod.rs`**

Add `pub mod teams;`

- [ ] **Step 3: Verify panel compilation**

Check the webchat build process (likely `trunk build` or `wasm-pack`). Run whatever build command is standard for the panel.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/api/teams.rs interfaces/webchat/src/api/mod.rs
git commit -m "panel: add teams API client"
```

---

## Task 8: Panel — Agent Edit Teams Tab

**Files:**
- Create: `interfaces/webchat/src/views/agents/teams.rs`
- Modify: `interfaces/webchat/src/views/agents/mod.rs` — Add Teams tab to enum and tab list

- [ ] **Step 1: Create `interfaces/webchat/src/views/agents/teams.rs`**

Follow the pattern in `channels.rs`:

```rust
use leptos::prelude::*;
use crate::context::DashboardState;
use crate::api::teams::TeamsApi;
use crate::i18n::*;

#[component]
pub fn TeamsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let teams = RwSignal::new(Vec::new());
    let is_loading = RwSignal::new(true);

    let agent_id_clone = agent_id.clone();
    Effect::new(move || {
        if !state.is_connected.get() { return; }
        let id = agent_id_clone.clone();
        let dash = state;
        spawn_local(async move {
            match TeamsApi::agent_teams(&dash, &id).await {
                Ok(result) => teams.set(result),
                Err(e) => web_sys::console::error_1(&format!("Failed to load teams: {e}").into()),
            }
            is_loading.set(false);
        });
    });

    view! {
        <div class="space-y-4">
            <p class="text-sm text-text-secondary">
                "Teams this agent belongs to. Managed through conversations."
            </p>

            {move || {
                if is_loading.get() {
                    return view! { <div class="text-text-secondary">"Loading..."</div> }.into_any();
                }

                let team_list = teams.get();
                if team_list.is_empty() {
                    return view! {
                        <div class="p-4 bg-surface-secondary rounded-lg text-center text-sm text-text-tertiary">
                            "To add this agent to a team, ask any agent to create a team with this agent as a member."
                        </div>
                    }.into_any();
                }

                view! {
                    <div class="space-y-3">
                        {team_list.into_iter().map(|team| {
                            let status_class = if team.status == "active" {
                                "text-emerald-400 bg-emerald-400/10 border-emerald-400/20"
                            } else {
                                "text-text-tertiary bg-surface-secondary border-border"
                            };
                            view! {
                                <div class="border border-border rounded-lg p-4">
                                    <div class="flex items-center gap-2 mb-2">
                                        <span class="text-sm font-medium text-text-primary">{&team.name}</span>
                                        <span class=format!("text-xs px-2 py-0.5 rounded-full border {}", status_class)>
                                            {&team.status}
                                        </span>
                                    </div>
                                    <div class="flex gap-4 text-xs text-text-tertiary">
                                        <span>"Leader: " {&team.leader_id}</span>
                                        <span>"Members: " {team.member_count}</span>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
```

- [ ] **Step 2: Add Teams tab to `views/agents/mod.rs`**

Add `Teams` variant to the `AgentTab` enum:
```rust
enum AgentTab {
    Overview,
    Behavior,
    Files,
    Skills,
    Tools,
    Channels,
    Teams,  // ← new, last position
}
```

Add to `ALL_TABS`:
```rust
const ALL_TABS: [AgentTab; 7] = [
    AgentTab::Overview,
    AgentTab::Behavior,
    AgentTab::Files,
    AgentTab::Skills,
    AgentTab::Tools,
    AgentTab::Channels,
    AgentTab::Teams,
];
```

Add the tab rendering in the match statement that renders tab content:
```rust
AgentTab::Teams => view! { <teams::TeamsTab agent_id=agent_id.clone() /> }.into_any(),
```

Add `mod teams;` at the top.

- [ ] **Step 3: Verify panel compilation**

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/agents/
git commit -m "panel: add Teams tab to agent edit page (read-only)"
```

---

## Task 9: Panel — Dashboard Teams Page

**Files:**
- Create: `interfaces/webchat/src/views/teams.rs`
- Modify: `interfaces/webchat/src/views/mod.rs` — Add `pub mod teams`
- Modify: `interfaces/webchat/src/components/dashboard_sidebar.rs` — Add Teams nav entry

- [ ] **Step 1: Create `interfaces/webchat/src/views/teams.rs`**

Dashboard teams page with collapsible cards:

```rust
use leptos::prelude::*;
use crate::context::DashboardState;
use crate::api::teams::{TeamsApi, TeamSummary, TeamDetail};
use crate::i18n::*;

#[component]
pub fn TeamsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let teams = RwSignal::new(Vec::<TeamSummary>::new());
    let is_loading = RwSignal::new(true);
    let expanded_id = RwSignal::new(Option::<String>::None);
    let expanded_detail = RwSignal::new(Option::<TeamDetail>::None);

    let reload = move || {
        let dash = state;
        spawn_local(async move {
            match TeamsApi::list(&dash).await {
                Ok(result) => teams.set(result),
                Err(e) => web_sys::console::error_1(&format!("Failed to load teams: {e}").into()),
            }
            is_loading.set(false);
        });
    };

    Effect::new(move || {
        if state.is_connected.get() { reload(); }
    });

    let toggle_expand = move |team_id: String| {
        if expanded_id.get().as_deref() == Some(&team_id) {
            expanded_id.set(None);
            expanded_detail.set(None);
        } else {
            expanded_id.set(Some(team_id.clone()));
            let dash = state;
            spawn_local(async move {
                if let Ok(detail) = TeamsApi::get(&dash, &team_id).await {
                    expanded_detail.set(Some(detail));
                }
            });
        }
    };

    let handle_disband = move |team_id: String| {
        let dash = state;
        spawn_local(async move {
            if TeamsApi::disband(&dash, &team_id).await.is_ok() {
                reload();
            }
        });
    };

    let handle_delete = move |team_id: String| {
        let dash = state;
        spawn_local(async move {
            if TeamsApi::delete(&dash, &team_id).await.is_ok() {
                reload();
            }
        });
    };

    // Render: header with stats, then team cards (collapsed by default, expandable)
    // Active teams get Disband button, disbanded teams get Delete button
    // Expanded card shows: leader, members with role badges, recent tasks
    // Follow the mockup from the brainstorming session
    view! {
        <div class="p-6 max-w-4xl">
            <div class="flex justify-between items-center mb-6">
                <div>
                    <h1 class="text-lg font-semibold text-text-primary">"Teams"</h1>
                    <p class="text-xs text-text-tertiary mt-1">
                        "Manage agent teams created through conversations"
                    </p>
                </div>
                {move || {
                    let list = teams.get();
                    let active = list.iter().filter(|t| t.status == "active").count();
                    let disbanded = list.iter().filter(|t| t.status == "disbanded").count();
                    view! {
                        <span class="text-xs text-text-tertiary">
                            {format!("{} active · {} disbanded", active, disbanded)}
                        </span>
                    }
                }}
            </div>

            {move || {
                if is_loading.get() {
                    return view! { <div class="text-text-secondary">"Loading..."</div> }.into_any();
                }

                let list = teams.get();
                if list.is_empty() {
                    return view! {
                        <div class="text-center text-text-tertiary py-12">
                            "No teams yet. Create a team through conversation."
                        </div>
                    }.into_any();
                }

                view! {
                    <div class="space-y-2">
                        {list.into_iter().map(|team| {
                            let id = team.id.clone();
                            let is_expanded = move || expanded_id.get().as_deref() == Some(&id);
                            let is_active = team.status == "active";
                            let opacity = if is_active { "" } else { "opacity-60" };

                            view! {
                                <div class=format!("border border-border rounded-lg overflow-hidden {}", opacity)>
                                    // Collapsed header — always visible
                                    <div
                                        class="px-4 py-3 bg-surface-secondary flex justify-between items-center cursor-pointer"
                                        on:click=move |_| toggle_expand(team.id.clone())
                                    >
                                        <div class="flex items-center gap-3">
                                            <span class="text-xs text-text-tertiary">
                                                {move || if is_expanded() { "▼" } else { "▶" }}
                                            </span>
                                            <span class="text-sm text-text-primary">{&team.name}</span>
                                            <span class=format!("text-xs px-2 py-0.5 rounded-full border {}",
                                                if is_active { "text-emerald-400 bg-emerald-400/10 border-emerald-400/20" }
                                                else { "text-text-tertiary bg-surface-secondary border-border" }
                                            )>{&team.status}</span>
                                            <span class="text-xs text-text-tertiary">
                                                {format!("{} members", team.member_count)}
                                            </span>
                                        </div>
                                        // Disband/Delete button
                                        {if is_active {
                                            let tid = team.id.clone();
                                            view! {
                                                <button
                                                    class="text-xs text-red-400 border border-red-400/20 px-3 py-1 rounded hover:bg-red-400/10"
                                                    on:click=move |e| {
                                                        e.stop_propagation();
                                                        handle_disband(tid.clone());
                                                    }
                                                >"Disband"</button>
                                            }.into_any()
                                        } else {
                                            let tid = team.id.clone();
                                            view! {
                                                <button
                                                    class="text-xs text-text-tertiary border border-border px-3 py-1 rounded hover:bg-surface-secondary"
                                                    on:click=move |e| {
                                                        e.stop_propagation();
                                                        handle_delete(tid.clone());
                                                    }
                                                >"Delete"</button>
                                            }.into_any()
                                        }}
                                    </div>

                                    // Expanded detail
                                    {move || {
                                        if !is_expanded() { return view! {}.into_any(); }
                                        if let Some(detail) = expanded_detail.get() {
                                            view! {
                                                <div class="px-4 py-3 border-t border-border space-y-3">
                                                    // Leader
                                                    <div>
                                                        <div class="text-xs text-text-tertiary uppercase tracking-wider mb-1">"Leader"</div>
                                                        <span class="text-sm text-text-secondary">{&detail.team.leader_id}</span>
                                                    </div>
                                                    // Members
                                                    <div>
                                                        <div class="text-xs text-text-tertiary uppercase tracking-wider mb-1">"Members"</div>
                                                        <div class="space-y-1">
                                                            {detail.members.iter().map(|m| {
                                                                view! {
                                                                    <div class="flex items-center gap-2 px-2 py-1 bg-surface-primary rounded text-sm">
                                                                        <span class="text-text-secondary flex-1">{&m.agent_id}</span>
                                                                        {(!m.role.is_empty()).then(|| view! {
                                                                            <span class="text-xs text-text-tertiary bg-surface-secondary px-2 py-0.5 rounded">{&m.role}</span>
                                                                        })}
                                                                    </div>
                                                                }
                                                            }).collect::<Vec<_>>()}
                                                        </div>
                                                    </div>
                                                    // Recent tasks
                                                    {(!detail.tasks.is_empty()).then(|| view! {
                                                        <div>
                                                            <div class="text-xs text-text-tertiary uppercase tracking-wider mb-1">"Recent Tasks"</div>
                                                            <div class="space-y-1 text-xs text-text-tertiary">
                                                                {detail.tasks.iter().take(5).map(|t| {
                                                                    let color = match t.status.as_str() {
                                                                        "completed" => "text-emerald-400",
                                                                        "running" => "text-yellow-400",
                                                                        "failed" => "text-red-400",
                                                                        _ => "text-text-tertiary",
                                                                    };
                                                                    view! {
                                                                        <div class="flex items-center gap-2">
                                                                            <span class=color>"●"</span>
                                                                            <span>{&t.agent_id}</span>
                                                                            <span>"→"</span>
                                                                            <span class="flex-1 truncate">{&t.subject}</span>
                                                                            <span class=color>{&t.status}</span>
                                                                        </div>
                                                                    }
                                                                }).collect::<Vec<_>>()}
                                                            </div>
                                                        </div>
                                                    })}
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { <div class="px-4 py-2 text-xs text-text-tertiary">"Loading..."</div> }.into_any()
                                        }
                                    }}
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
```

The expanded detail section should render when `is_expanded()` is true and `expanded_detail` has data, showing leader info, member list with role badges, and recent task history. Follow the mockup from the brainstorming session.

- [ ] **Step 2: Add to `interfaces/webchat/src/views/mod.rs`**

Add `pub mod teams;`

- [ ] **Step 3: Add Teams to dashboard sidebar**

In `interfaces/webchat/src/components/dashboard_sidebar.rs`, add a `SidebarItem` after the existing entries:

```rust
<SidebarItem href="/dashboard/teams" label=Signal::derive(move || "Teams".to_string())>
    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
    <circle cx="9" cy="7" r="4" />
    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
</SidebarItem>
```

- [ ] **Step 4: Wire route in the panel's router**

Find where dashboard routes are defined (likely in `app.rs` or the main router file) and add:
```rust
"/dashboard/teams" => view! { <teams::TeamsView /> }
```

- [ ] **Step 5: Verify panel compilation**

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/teams.rs interfaces/webchat/src/views/mod.rs interfaces/webchat/src/components/dashboard_sidebar.rs
git commit -m "panel: add dashboard Teams management page"
```

---

## Task 10: Panel — Agent List Dropdown Filter

**Files:**
- Modify: `interfaces/webchat/src/components/agents_sidebar.rs`

- [ ] **Step 1: Add filter state and dropdown**

In `AgentsSidebar` component, add:
- A `RwSignal<String>` for the filter value (default "all")
- A `<select>` dropdown at the top of the sidebar with options: All Agents / Channel Agents / Standalone Agents
- Filter the agent list based on the selected value before rendering

To determine which agents have channel bindings, use the existing `agents.bindings` RPC call (same one the channels tab uses) to get the mapping, then filter agents accordingly:
- "all" → show all agents
- "channel" → show agents that appear in the bindings map
- "standalone" → show agents NOT in the bindings map

```rust
let filter = RwSignal::new("all".to_string());
let bindings = RwSignal::new(std::collections::HashMap::<String, String>::new());

// Load bindings alongside agents
Effect::new(move || {
    if state.is_connected.get() {
        let dash = state;
        spawn_local(async move {
            if let Ok(result) = dash.rpc_call("agents.bindings", serde_json::Value::Null).await {
                if let Some(map) = result.get("bindings").and_then(|v| v.as_object()) {
                    let b: HashMap<String, String> = map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect();
                    bindings.set(b);
                }
            }
        });
    }
});
```

Add dropdown before the agent list:
```rust
<select
    class="w-full px-3 py-1.5 bg-surface-secondary border border-border rounded-md text-xs text-text-secondary"
    on:change=move |ev| filter.set(event_target_value(&ev))
>
    <option value="all">"All Agents"</option>
    <option value="channel">"Channel Agents"</option>
    <option value="standalone">"Standalone Agents"</option>
</select>
```

Filter agents before rendering:
```rust
let filtered = move || {
    let list = agents.get();
    let f = filter.get();
    let b = bindings.get();
    match f.as_str() {
        "channel" => list.into_iter().filter(|a| b.contains_key(&a.id)).collect::<Vec<_>>(),
        "standalone" => list.into_iter().filter(|a| !b.contains_key(&a.id)).collect::<Vec<_>>(),
        _ => list,
    }
};
```

For channel agents, show the channel name badge on the right side of each agent item.

- [ ] **Step 2: Verify panel compilation**

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/agents_sidebar.rs
git commit -m "panel: add dropdown filter to agent list sidebar (All/Channel/Standalone)"
```

---

## Task 11: Integration Test and Final Verification

- [ ] **Step 1: Run full compilation**

```bash
cargo check -p alephcore && cargo check --bin aleph-server
```

- [ ] **Step 2: Run core tests**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 3: Build panel**

Run the panel build command (check `justfile` for the correct command, likely `just build` which builds WASM first).

- [ ] **Step 4: Manual smoke test**

Start the server and verify:
1. Dashboard → Teams page loads (empty state)
2. Agent list sidebar dropdown filter works
3. Agent edit → Teams tab shows (empty state)
4. Team tools are registered (check via `team_create` in LLM conversation)

- [ ] **Step 5: Final commit if any fixes needed**

```bash
git add core/ interfaces/
git commit -m "teams: integration fixes and cleanup"
```
