# Task Coordination System Design

**Date**: 2026-03-21
**Status**: Approved
**Scope**: Swarm task coordination layer — DAG dependencies, team templates, leader protocol

## Background

Aleph's swarm system has a mature perception layer (Event Bus, Collective Memory, Rules Engine, Context Injector) but lacks an **action coordination layer**. Agents can sense each other's activities but have no formal mechanism for: who does what, in what order, and who approves.

ClawTeam (a Claude Code multi-agent plugin) demonstrates that **task DAG + team templates + leader-worker coordination** constitute the soul of multi-agent collaboration. This design absorbs those core concepts into Aleph's Rust kernel, adapted to Aleph's architecture and principles.

### Why kernel, not plugin

- Task coordination IS scheduling — core's job per R3 ("内核只调度，不搬砖")
- The swarm module (event bus, collective memory, context injector) already lives in `core/src/agents/swarm/`; putting task coordination in a plugin would split the swarm brain in half
- Event-driven dependency resolution (task completed → blocked tasks auto-unblock) requires tight Event Bus integration that plugins cannot achieve without polling

## Data Model

### Task

```rust
pub struct Task {
    pub id: TaskId,                    // UUID
    pub team_id: Option<TeamId>,       // nullable — supports standalone tasks
    pub subject: String,               // brief title
    pub description: String,           // detailed description for agent
    pub status: TaskStatus,
    pub owner: Option<AgentId>,        // assigned agent
    pub blocked_by: Vec<TaskId>,       // unresolved dependencies (derived from task_dependencies table)
    pub priority: Priority,            // Low / Normal / High / Critical
    pub metadata: serde_json::Value,   // extensible (LLM free-form, R8)
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub enum TaskStatus {
    Pending,     // ready to start (all dependencies resolved)
    Blocked,     // has unresolved blocked_by
    InProgress,  // agent is working on it
    Completed,   // done
    Failed,      // failed (retryable)
    Cancelled,   // cancelled by leader
}
```

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

## TaskStore Trait

```rust
#[async_trait]
pub trait TaskStore: Send + Sync {
    // Task CRUD
    async fn create_task(&self, task: NewTask) -> Result<Task>;
    async fn get_task(&self, id: &TaskId) -> Result<Option<Task>>;
    async fn update_task(&self, id: &TaskId, update: TaskUpdate) -> Result<Task>;
    async fn list_tasks(&self, filter: TaskFilter) -> Result<Vec<Task>>;

    // DAG operation — the core primitive
    async fn resolve_dependencies(&self, completed_id: &TaskId) -> Result<Vec<Task>>;

    // Team CRUD
    async fn create_team(&self, team: NewTeam) -> Result<Team>;
    async fn get_team(&self, id: &TeamId) -> Result<Option<Team>>;
    async fn update_team(&self, id: &TeamId, update: TeamUpdate) -> Result<Team>;
    async fn list_teams(&self, filter: TeamFilter) -> Result<Vec<Team>>;
    async fn add_member(&self, team_id: &TeamId, member: TeamMember) -> Result<()>;
    async fn remove_member(&self, team_id: &TeamId, agent_id: &AgentId) -> Result<()>;
}

pub struct TaskFilter {
    pub team_id: Option<TeamId>,
    pub status: Option<TaskStatus>,
    pub owner: Option<AgentId>,
}

pub struct TaskUpdate {
    pub status: Option<TaskStatus>,
    pub owner: Option<AgentId>,
    pub metadata: Option<serde_json::Value>,
}
```

## SQLite Schema

```sql
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    team_id TEXT,
    subject TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',
    owner TEXT,
    priority TEXT NOT NULL DEFAULT 'normal',
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    FOREIGN KEY (team_id) REFERENCES teams(id)
);

CREATE TABLE task_dependencies (
    task_id TEXT NOT NULL,
    depends_on TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on),
    FOREIGN KEY (task_id) REFERENCES tasks(id),
    FOREIGN KEY (depends_on) REFERENCES tasks(id)
);

CREATE TABLE teams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    leader TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL
);

CREATE TABLE team_members (
    team_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT '',
    joined_at TEXT NOT NULL,
    PRIMARY KEY (team_id, agent_id),
    FOREIGN KEY (team_id) REFERENCES teams(id)
);

CREATE INDEX idx_tasks_team_status ON tasks(team_id, status);
CREATE INDEX idx_tasks_owner ON tasks(owner);
```

## DAG Dependency Resolution

### Unlock flow

When `task_update(status=Completed)` is called:

1. In a single SQLite transaction:
   - `DELETE FROM task_dependencies WHERE depends_on = {completed_id}`
   - `SELECT tasks WHERE status='blocked' AND id NOT IN (SELECT task_id FROM task_dependencies)` — tasks with no remaining dependencies
   - `UPDATE` those tasks `SET status='pending'`
2. Publish events to Event Bus:
   - `TaskCompleted { task_id, agent_id }`
   - `TaskUnblocked { task_id, unblocked_by }` for each newly unblocked task
   - `AllTasksCompleted { team_id }` when applicable
3. Context Injector picks up events → next agent call sees updated task board

### Cycle detection

On `create_task`, BFS from each `blocked_by` task backwards through `task_dependencies`. If the new task ID is reachable → reject with error. Task counts are typically <100, so BFS cost is negligible.

## Event Bus Integration

New event types in `swarm/events.rs`:

```rust
pub enum ImportantEvent {
    // ... existing variants
    TaskCompleted { task_id: TaskId, agent_id: AgentId },
    TaskUnblocked { task_id: TaskId, unblocked_by: TaskId },
    TaskFailed { task_id: TaskId, reason: String },
    AllTasksCompleted { team_id: TeamId },
}
```

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
  - subject: Analyze code change scope
    owner: architect
    priority: high
  - subject: Review logic and security
    owner: reviewer-1
    blocked_by: [Analyze code change scope]
  - subject: Review performance and maintainability
    owner: reviewer-2
    blocked_by: [Analyze code change scope]
  - subject: Consolidate review report
    owner: summarizer
    blocked_by: [Review logic and security, Review performance and maintainability]
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

### `blocked_by` uses subject references

In templates, `blocked_by: [Analyze code change scope]` references tasks by subject text (not IDs). `team_launch` resolves subjects to TaskIds at creation time.

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
| `task_wait` | `task_ids[] \| team_id, timeout_seconds=300` | Event Bus subscription, no polling |
| `team_create` | `name, description?, leader` | Manual team creation (no template) |
| `team_launch` | `template, name?, variables{}` | Template → agents + tasks in one call |
| `team_list` | (no params) | All teams with status overview |
| `team_disband` | `team_id, archive=true` | Cancel tasks, optionally archive workspaces |

Registered in `executor/builtin_registry/groups.rs` as `task_coordination` group. Available to all agents — permission is prompt-guided, not tool-gated (R8).

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

## File Structure

### New files

```
core/src/agents/swarm/tasks/
├── mod.rs              # TaskStore trait, model types (~80 lines)
├── store.rs            # SqliteTaskStore implementation (~200 lines)
├── dag.rs              # resolve_dependencies + cycle detection (~80 lines)
└── template.rs         # Template parser + variable substitution (~100 lines)

core/src/builtin_tools/task_manage/
├── mod.rs
├── create.rs           # task_create tool
├── update.rs           # task_update tool (triggers DAG unlock)
├── list.rs             # task_list tool
└── wait.rs             # task_wait tool (Event Bus subscription)

core/src/builtin_tools/team_manage/
├── mod.rs
├── create.rs           # team_create tool
├── launch.rs           # team_launch tool (template → agents + tasks)
├── list.rs             # team_list tool
└── disband.rs          # team_disband tool
```

### Modified files

```
core/src/agents/swarm/events.rs           # +4 event types
core/src/agents/swarm/coordinator.rs      # + TaskStore integration
core/src/agents/swarm/context_injector.rs # + inject_task_context()
core/src/executor/builtin_registry/groups.rs      # + task_coordination group
core/src/executor/builtin_registry/definitions.rs # + 8 tool definitions
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
| tasks/mod.rs | ~80 | trait + model definitions |
| tasks/store.rs | ~200 | SQLite CRUD |
| tasks/dag.rs | ~80 | dependency resolution + cycle detection |
| tasks/template.rs | ~100 | frontmatter parsing + variable substitution |
| task_manage/ (4 tools) | ~200 | thin shells delegating to TaskStore |
| team_manage/ (4 tools) | ~250 | launch is the most complex |
| events.rs changes | ~20 | new event types |
| context_injector.rs changes | ~60 | task context injection |
| **Total** | **~990** | excluding tests |

## YAGNI — Explicitly Not Doing

- **Cost tracking** — use metadata field if needed, no dedicated table
- **Plan approval protocol** — leader uses session_send for decisions, no formal protocol
- **Dashboard** — WebChat already shows sessions/agents, extend later if needed
- **Mailbox system** — session_send already covers inter-agent messaging
- **Process-level isolation** — Aleph agents are in-process instances, no tmux/worktree needed

## Architectural Compliance

| Principle | How this design complies |
|-----------|------------------------|
| R3 Core Minimalism | ~990 lines, zero new dependencies. This is scheduling, not heavy lifting |
| R8 LLM Sovereignty | Leader decisions, task assignment, prioritization all LLM-driven. Code only does mechanical dependency resolution |
| R9 Everything is a Tool | 8 builtin tools cover the full lifecycle |
| R10 Intelligence in Prompt | Coordination intelligence lives in prompt templates, not code logic |
| P1 Low Coupling | TaskStore trait abstracts storage; events flow through existing Event Bus |
| P2 High Cohesion | All task logic in `swarm/tasks/`, all tools in `task_manage/` + `team_manage/` |
| P3 Extensibility | Plugins can provide templates; TaskStore trait is swappable |
| P4 Dependency Inversion | Core defines TaskStore trait, SQLite is one implementation |
| P6 Simplicity | Minimal additions to existing swarm, no new subsystems |
