# Task Coordination System Design

**Date**: 2026-03-21
**Status**: Approved
**Scope**: Swarm task coordination layer — DAG dependencies, team templates, leader protocol

## Background

Aleph's swarm system has a mature perception layer (Event Bus, Collective Memory, Rules Engine, Context Injector) but lacks an **action coordination layer**. Agents can sense each other's activities but have no formal mechanism for: who does what, in what order, and who approves.

ClawTeam (a Claude Code multi-agent plugin) demonstrates that **task DAG + team templates + leader-worker coordination** constitute the soul of multi-agent collaboration. This design absorbs those core concepts into Aleph's Rust kernel, adapted to Aleph's architecture and principles.

### Why kernel, not plugin

- Task coordination IS scheduling — core's job per R3 ("内核只调度，不搬砖")
- The swarm module (event bus, collective memory, context injector) already lives in `src/agents/swarm/`; putting task coordination in a plugin would split the swarm brain in half
- Event-driven dependency resolution (task completed → blocked tasks auto-unblock) requires tight Event Bus integration that plugins cannot achieve without polling

## Naming Convention

The Dispatcher module already defines `Task`, `TaskStatus`, `TaskDependency`, and `TaskGraph` for single-agent execution units (file ops, code exec, AI inference). This system's types use the `Coord` prefix to avoid collision:

- `CoordTask` (not `Task`)
- `CoordTaskStatus` (not `TaskStatus`)
- `CoordTaskId` (not `TaskId`)

All types live in `core::agents::swarm::tasks` module, so fully-qualified paths are `swarm::tasks::CoordTask` vs `dispatcher::agent_types::Task`.

## Data Model

### CoordTask

```rust
pub struct CoordTask {
    pub id: CoordTaskId,               // UUID
    pub team_id: Option<TeamId>,       // nullable — supports standalone tasks
    pub subject: String,               // brief title
    pub description: String,           // detailed description for agent
    pub status: CoordTaskStatus,
    pub owner: Option<AgentId>,        // assigned agent
    pub priority: Priority,            // Low / Normal / High / Critical
    pub result: Option<String>,        // task output summary (set on completion)
    pub metadata: serde_json::Value,   // extensible (LLM free-form, R8)
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub enum CoordTaskStatus {
    Pending,     // ready to start (all dependencies resolved)
    Blocked,     // has unresolved dependencies (derived dynamically from task_dependencies)
    InProgress,  // agent is working on it
    Completed,   // done
    Failed,      // failed (retryable)
    Cancelled,   // cancelled by leader
}
```

Note: `Blocked` status is **derived dynamically** — a task is Blocked if it has any row in `task_dependencies` where the `depends_on` task is not Completed. The `task_dependencies` table is never mutated after creation (append-only). See DAG section.

State transitions:
- `Blocked → Pending` — automatic when all `blocked_by` tasks complete
- `Pending → InProgress` — agent claims the task
- `InProgress → Completed | Failed` — agent reports result
- Any → `Cancelled` — leader cancels

### Team

```rust
pub struct Team {
    pub id: TeamId,               // slug format
    pub name: String,
    pub description: String,
    pub leader: AgentId,
    pub members: Vec<TeamMember>,
    pub status: TeamStatus,       // Active / Completed / Disbanded
    pub created_at: DateTime<Utc>,
}

pub struct TeamMember {
    pub agent_id: AgentId,
    pub role: String,             // free text, LLM interprets (R8)
    pub joined_at: DateTime<Utc>,
}
```

## CoordTaskStore Trait

```rust
#[async_trait]
pub trait CoordTaskStore: Send + Sync {
    // Task CRUD
    async fn create_task(&self, task: NewCoordTask) -> Result<CoordTask>;
    async fn get_task(&self, id: &CoordTaskId) -> Result<Option<CoordTask>>;
    async fn update_task(&self, id: &CoordTaskId, update: CoordTaskUpdate) -> Result<CoordTask>;
    async fn list_tasks(&self, filter: CoordTaskFilter) -> Result<Vec<CoordTask>>;

    // DAG queries (read-only — dependency edges are immutable after creation)
    async fn get_dependencies(&self, id: &CoordTaskId) -> Result<Vec<CoordTaskId>>;
    async fn get_dependents(&self, id: &CoordTaskId) -> Result<Vec<CoordTaskId>>;
    async fn get_newly_unblocked(&self, completed_id: &CoordTaskId) -> Result<Vec<CoordTask>>;
    // ^ Returns tasks whose ALL dependencies are now Completed (including the just-completed one)

    // Team CRUD
    async fn create_team(&self, team: NewTeam) -> Result<Team>;
    async fn get_team(&self, id: &TeamId) -> Result<Option<Team>>;
    async fn update_team(&self, id: &TeamId, update: TeamUpdate) -> Result<Team>;
    async fn list_teams(&self, filter: TeamFilter) -> Result<Vec<Team>>;
    async fn add_member(&self, team_id: &TeamId, member: TeamMember) -> Result<()>;
    async fn remove_member(&self, team_id: &TeamId, agent_id: &AgentId) -> Result<()>;
}

pub struct CoordTaskFilter {
    pub team_id: Option<TeamId>,
    pub status: Option<CoordTaskStatus>,
    pub owner: Option<AgentId>,
}

pub struct CoordTaskUpdate {
    pub status: Option<CoordTaskStatus>,
    pub owner: Option<AgentId>,
    pub result: Option<String>,
    pub metadata: Option<serde_json::Value>,
}
```

## SQLite Schema

```sql
CREATE TABLE coord_tasks (
    id TEXT PRIMARY KEY,
    team_id TEXT,
    subject TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',  -- stored status (pending/in_progress/completed/failed/cancelled)
    owner TEXT,
    priority TEXT NOT NULL DEFAULT 'normal',
    result TEXT,                              -- task output summary
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    FOREIGN KEY (team_id) REFERENCES coord_teams(id) ON DELETE CASCADE
);

-- Immutable after creation. Never deleted — preserves full DAG history.
-- Blocked status is derived: a task is Blocked if ANY depends_on task has status != 'completed'.
CREATE TABLE coord_task_dependencies (
    task_id TEXT NOT NULL,
    depends_on TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on),
    FOREIGN KEY (task_id) REFERENCES coord_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY (depends_on) REFERENCES coord_tasks(id) ON DELETE CASCADE
);

CREATE TABLE coord_teams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    leader TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL
);

CREATE TABLE coord_team_members (
    team_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT '',
    joined_at TEXT NOT NULL,
    PRIMARY KEY (team_id, agent_id),
    FOREIGN KEY (team_id) REFERENCES coord_teams(id) ON DELETE CASCADE
);

CREATE INDEX idx_coord_tasks_team_status ON coord_tasks(team_id, status);
CREATE INDEX idx_coord_tasks_owner ON coord_tasks(owner);
```

Note: `ON DELETE CASCADE` ensures `team_disband` can delete the team row and all related tasks/members/dependencies are cleaned up in a single transaction.

## DAG Dependency Resolution

### Core principle: immutable edges, derived status

The `coord_task_dependencies` table is **append-only** — edges are never deleted or mutated after creation. This preserves the full DAG structure for auditing, debugging, and retry scenarios.

`Blocked` status is derived dynamically via query:

```sql
-- A task is "blocked" if it has ANY dependency that is not completed
SELECT t.id FROM coord_tasks t
WHERE t.status = 'pending'  -- only check stored-pending tasks
  AND EXISTS (
    SELECT 1 FROM coord_task_dependencies d
    JOIN coord_tasks dep ON dep.id = d.depends_on
    WHERE d.task_id = t.id AND dep.status != 'completed'
  );
```

When listing tasks, the `list_tasks` query enriches stored status with this derivation:
- Stored `pending` + has unresolved deps → report as `Blocked`
- Stored `pending` + no unresolved deps → report as `Pending`
- All other statuses → report as stored

### Unlock flow

When `task_update(status=Completed)` is called:

1. Update the task's stored status to `completed` + set `completed_at`
2. Query for newly unblocked tasks:
   ```sql
   -- Tasks that depended on the completed task AND now have ALL deps completed
   SELECT t.* FROM coord_tasks t
   JOIN coord_task_dependencies d ON d.task_id = t.id
   WHERE d.depends_on = {completed_id}
     AND t.status = 'pending'
     AND NOT EXISTS (
       SELECT 1 FROM coord_task_dependencies d2
       JOIN coord_tasks dep ON dep.id = d2.depends_on
       WHERE d2.task_id = t.id AND dep.status != 'completed'
     )
   ```
3. Publish events to Event Bus:
   - `TaskCompleted { task_id, agent_id }`
   - `TaskUnblocked { task_id, unblocked_by }` for each newly unblocked task
   - `AllTasksCompleted { team_id }` when all tasks in the team are completed
4. Context Injector picks up events → next agent call sees updated task board

### Retry support

Because edges are preserved, retrying a failed task is straightforward:
- `task_update(id, status=pending)` → re-derives as Blocked or Pending based on current deps
- If the failed task's dependents were already completed, no cascading undo needed (LLM decides whether to redo them)

### Cycle detection

On `create_task`, BFS from each `blocked_by` task backwards through `coord_task_dependencies`. If the new task ID is reachable → reject with error. Task counts are typically <100, so BFS cost is negligible.

## Event Bus Integration

New event types in `swarm/events.rs`:

```rust
pub enum ImportantEvent {
    // ... existing variants (all include timestamp: u64)
    TaskCompleted { task_id: String, agent_id: String, timestamp: u64 },
    TaskUnblocked { task_id: String, unblocked_by: String, timestamp: u64 },
    TaskFailed { task_id: String, reason: String, timestamp: u64 },
    AllTasksCompleted { team_id: String, timestamp: u64 },
}
```

Note: Uses `String` for identifiers and includes `timestamp: u64` to match existing `ImportantEvent` variant conventions.

Existing Rules Engine and Context Injector process these without modification — they already dispatch by event tier.

## Team Template System

### Format: Markdown + YAML frontmatter

```markdown
---
name: code-review-team
description: Code review team with architect + reviewers + summarizer
leader: architect
members:
  - name: architect
    role: Architect, assigns review tasks and makes final decisions
    model: claude-sonnet-4-20250514
  - name: reviewer-1
    role: Code reviewer, focuses on logic correctness and security
  - name: reviewer-2
    role: Code reviewer, focuses on performance and maintainability
  - name: summarizer
    role: Summarizer, consolidates all review findings into final report
tasks:
  - key: analyze
    subject: Analyze code change scope
    owner: architect
    priority: high
  - key: review-logic
    subject: Review logic and security
    owner: reviewer-1
    blocked_by: [analyze]
  - key: review-perf
    subject: Review performance and maintainability
    owner: reviewer-2
    blocked_by: [analyze]
  - key: consolidate
    subject: Consolidate review report
    owner: summarizer
    blocked_by: [review-logic, review-perf]
variables:
  - name: goal
    description: Review target (commit range, PR, file list, etc.)
---

# Code Review Team

You are {agent_name}, {agent_role}.

## Team Goal

{goal}

## Coordination Protocol

- Report completion via `task_update` with findings in metadata
- Escalate critical issues to architect via `session_send`
- Architect can cancel or reassign tasks at any time
```

### Template resolution order

1. User: `~/.aleph/templates/`
2. Plugin: `{plugin}/templates/` (namespaced as `plugin-name:template-name`)
3. Built-in: `core/assets/templates/` (embedded at compile time or copied at install)

### `blocked_by` uses key references

Each task in a template has a unique `key` field (slug format). `blocked_by` references these keys, not subject text. This eliminates ambiguity from duplicate subjects or whitespace/case issues. `team_launch` resolves keys to CoordTaskIds at creation time. Keys are template-local — they are not stored in the database.

### Variable substitution

Built-in variables: `{agent_name}`, `{agent_role}`, `{team_name}`, `{leader_name}`
Template-declared variables: passed via `team_launch(variables={...})`

### Launch flow

```
team_launch(template, variables)
  1. Find template file (user → plugin → built-in)
  2. Parse frontmatter + body
  3. Substitute variables
  4. Create Team (SQLite)
  5. For each member → call agent_create (reuse existing tool)
     └─ Markdown body becomes agent's SOUL.md
  6. Create all Tasks, resolve subject refs → TaskIds
  7. Publish TeamCreated event
  8. Return team overview to LLM
```

## Builtin Tools

| Tool | Schema (key params) | Notes |
|------|---------------------|-------|
| `task_create` | `subject, description?, team_id?, owner?, blocked_by[], priority, metadata` | Non-empty blocked_by → auto Blocked status |
| `task_update` | `task_id, status?, owner?, metadata` | Completed → triggers resolve_dependencies |
| `task_list` | `team_id?, status?, owner?` | Returns tasks with dependency info |
| `task_wait` | `task_ids[] \| team_id, timeout_seconds=300` | Polling with early exit on Event Bus (see below) |
| `team_create` | `name, description?, leader` | Manual team creation (no template) |
| `team_launch` | `template, name?, variables{}` | Template → agents + tasks in one call |
| `team_list` | (no params) | All teams with status overview |
| `team_disband` | `team_id, archive=true` | Cancel tasks, optionally archive workspaces |

Registered in `executor/builtin_registry/groups.rs` as `task_coordination` group (must be added to the `ALL_GROUPS` array so the existing `test_all_builtin_tools_have_a_group` test passes). Available to all agents — permission is prompt-guided, not tool-gated (R8).

### `task_wait` Mechanics

`task_wait` is the most complex tool. It must block the calling agent's tool execution until conditions are met, without stalling the entire system.

**Implementation**: `tokio::select!` on Event Bus broadcast + timeout:

```rust
async fn call(&self, args: TaskWaitArgs) -> Result<ToolOutput> {
    let mut receiver = self.event_bus.subscribe();
    let deadline = Instant::now() + Duration::from_secs(args.timeout_seconds);

    loop {
        // Check if already satisfied (query SQLite)
        let tasks = self.store.list_tasks(filter).await?;
        if all_completed(&tasks) {
            return Ok(format_completion_summary(&tasks));
        }

        // Wait for next relevant event or timeout
        tokio::select! {
            event = receiver.recv() => {
                match event {
                    Ok(TaskCompleted { .. } | AllTasksCompleted { .. }) => continue, // re-check
                    Ok(TaskFailed { .. }) => continue, // re-check (may want to report failure)
                    _ => continue, // ignore unrelated events
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                // Timeout — return current status, not an error
                let tasks = self.store.list_tasks(filter).await?;
                return Ok(format_timeout_summary(&tasks));
            }
        }
    }
}
```

**Agent loop impact**: While `task_wait` blocks, the calling agent is in `ACTING` state — this is the same as any long-running tool (e.g., HTTP requests). The agent cannot process other messages during this time. This is intentional for the leader pattern: the leader waits for team results, then acts on them.

**Timeout behavior**: Returns current task status (not an error), so the LLM can decide what to do next — retry, cancel, or check on stuck agents.

### `team_launch` Failure Handling

`team_launch` creates multiple agents and tasks. Partial failure strategy:

1. Create Team record first (single SQLite insert)
2. Create agents sequentially — if any fails, delete already-created agents + team record and return error
3. Create all tasks in a single SQLite transaction — atomic, all or nothing
4. If task creation fails after agents are created — delete agents + team and return error

This is a simple "all or nothing" approach. No partial teams exist after a failed launch.

### `team_disband` and Agent Lifecycle

Agents created by `team_launch` are **permanent named agents** — they persist in AgentRegistry after the team disbands. This matches Aleph's agent model (agents are peers, not ephemeral workers).

- `team_disband(archive=false)` — cancels tasks, marks team disbanded, agents remain
- `team_disband(archive=true)` — additionally calls `agent_delete` for each team member (archives workspaces)

The LLM (or user) decides which mode to use. For one-off tasks, `archive=true`. For recurring teams, `archive=false` preserves the agents for reuse.

### Relationship with Existing Tools

- **`get_team_activity`** (swarm) — queries CollectiveMemory for low-level event streams (who accessed what file, what tool was called). Stays as-is for observability.
- **`task_list`** — structured task board view (status, dependencies, owners). Higher-level, action-oriented.
- These are complementary: `task_list` shows "what needs to be done", `get_team_activity` shows "what's happening right now".

## Prompt Templates

### Leader injection

When an agent is a team's leader, Context Injector appends:

```
## You are Team Leader

You are the leader of team "{team_name}". Your responsibilities:

### Task Management
- Use `task_list` to view the team task board
- Use `task_create` to assign new tasks (set owner and blocked_by)
- Use `task_update` to cancel or reassign tasks
- Use `task_wait` to wait for critical tasks

### Coordination (your judgment, not system rules)
- Agent task failed → you decide: retry, reassign, or skip
- Task taking too long → you decide whether to intervene
- Unexpected discovery → you decide whether to adjust plan
- All tasks complete → summarize results, report to user via session_send

### Team Members
{member_list}

### Current Task Board
{task_board}
```

### Member injection

```
## You are a Team Member

You are {role} in team "{team_name}". Leader is {leader_name}.

### Workflow
1. `task_list --owner {self_id}` — check your assigned tasks
2. `task_update(task_id, status="in_progress")` — start working
3. `task_update(task_id, status="completed", metadata={summary})` — done
4. `task_update(task_id, status="failed", metadata={reason})` — failed

### Collaboration
- Blocked or need decision → `session_send({leader_id}, "describe issue")`
- Found info relevant to teammate → `session_send(teammate_id, "info")`

### Your Tasks
{my_tasks}
```

These are **dynamically generated** on every agent call — always reflects latest task state from SQLite.

### Context Injector Integration Detail

The current `ContextInjector` holds only `Arc<AgentMessageBus>`. Task board injection requires querying `CoordTaskStore` directly (fresh state, not event history). Changes needed:

1. `ContextInjector` gains an `Option<Arc<dyn CoordTaskStore>>` field (Option because swarm can run without task coordination)
2. `SwarmCoordinator` passes the `CoordTaskStore` when constructing `ContextInjector`
3. `inject_task_context()` queries the store for the agent's team membership and tasks
4. If the agent is not in any team, this method returns `None` (no injection)

This changes the `ContextInjector` constructor signature, which propagates to `SwarmCoordinator::new()`. The `SwarmCoordinator` already holds all swarm components, so adding `CoordTaskStore` is a natural extension (~15 lines in coordinator.rs).

## File Structure

### New files

```
src/agents/swarm/tasks/
├── mod.rs              # TaskStore trait, model types (~80 lines)
├── store.rs            # SqliteTaskStore implementation (~200 lines)
├── dag.rs              # resolve_dependencies + cycle detection (~80 lines)
└── template.rs         # Template parser + variable substitution (~100 lines)

src/builtin_tools/task_manage/
├── mod.rs
├── create.rs           # task_create tool
├── update.rs           # task_update tool (triggers DAG unlock)
├── list.rs             # task_list tool
└── wait.rs             # task_wait tool (Event Bus subscription)

src/builtin_tools/team_manage/
├── mod.rs
├── create.rs           # team_create tool
├── launch.rs           # team_launch tool (template → agents + tasks)
├── list.rs             # team_list tool
└── disband.rs          # team_disband tool
```

### Modified files

```
src/agents/swarm/events.rs           # +4 event types
src/agents/swarm/coordinator.rs      # + TaskStore integration
src/agents/swarm/context_injector.rs # + inject_task_context()
src/executor/builtin_registry/groups.rs      # + task_coordination group
src/executor/builtin_registry/definitions.rs # + 8 tool definitions
```

### Template locations

```
~/.aleph/templates/        # user-defined
core/assets/templates/     # built-in
{plugin}/templates/        # plugin-provided
```

### Estimated size

| Module | Lines | Description |
|--------|-------|-------------|
| tasks/mod.rs | ~100 | trait + model definitions (Coord-prefixed types) |
| tasks/store.rs | ~250 | SQLite CRUD + derived Blocked status |
| tasks/dag.rs | ~80 | cycle detection (resolution is query-based now) |
| tasks/template.rs | ~120 | frontmatter parsing + key resolution + variable substitution |
| task_manage/ (4 tools) | ~250 | task_wait has select! loop |
| team_manage/ (4 tools) | ~300 | launch has failure rollback |
| events.rs changes | ~20 | new event types |
| context_injector.rs changes | ~80 | task context injection + CoordTaskStore dependency |
| builtin_registry changes | ~50 | group registration, builder, definitions match arms |
| migration | ~30 | SQLite table creation |
| **Total** | **~1280** | excluding tests |

The ~1280 estimate is more realistic than the original ~990, accounting for: derived Blocked status queries, task_wait select! loop, team_launch rollback logic, builder/registry boilerplate, and SQLite migration. Still well within R3 compliance — zero new dependencies, all scheduling logic.

## YAGNI — Explicitly Not Doing

- **Cost tracking** — use metadata field if needed, no dedicated table
- **Plan approval protocol** — leader uses session_send for decisions, no formal protocol
- **Dashboard** — WebChat already shows sessions/agents, extend later if needed
- **Mailbox system** — session_send already covers inter-agent messaging
- **Process-level isolation** — Aleph agents are in-process instances, no tmux/worktree needed

## Architectural Compliance

| Principle | How this design complies |
|-----------|------------------------|
| R3 Core Minimalism | ~1280 lines, zero new dependencies. This is scheduling, not heavy lifting |
| R8 LLM Sovereignty | Leader decisions, task assignment, prioritization all LLM-driven. Code only does mechanical dependency resolution |
| R9 Everything is a Tool | 8 builtin tools cover the full lifecycle |
| R10 Intelligence in Prompt | Coordination intelligence lives in prompt templates, not code logic |
| P1 Low Coupling | TaskStore trait abstracts storage; events flow through existing Event Bus |
| P2 High Cohesion | All task logic in `swarm/tasks/`, all tools in `task_manage/` + `team_manage/` |
| P3 Extensibility | Plugins can provide templates; TaskStore trait is swappable |
| P4 Dependency Inversion | Core defines TaskStore trait, SQLite is one implementation |
| P6 Simplicity | Minimal additions to existing swarm, no new subsystems |
