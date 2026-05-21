# Team Module Refactor — Registered Agent Teams

**Date:** 2026-03-26
**Status:** Approved
**Scope:** Core (team tools, SQLite, sub-agent cleanup) + Panel (agent list, agent edit, dashboard teams)

## Background

Team functionality went through two iterations:
1. **V1** — Registered agent teams: created too many agents, polluted the registry
2. **V2** — Leader + sub-agent mode: reused sub-agent module with persona injection

After studying Claude Code, OpenClaw, and TeamClaw patterns, V2 is found to be architecturally wrong — sub-agents are ephemeral task workers, not team members. This refactor returns to registered agent teams (V1) with proper management UX to address the registry pollution concern.

## Core Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Team members | Registered agents (not sub-agents) | Persistent identity, independent config, reusable across teams |
| Sub-agent role | Restored to original: temporary, inherits parent, auto-destroyed | Clean separation from team concept |
| Collaboration model | Leader agent dispatches members via tools | Fits R8 (LLM sovereignty) — leader LLM decides coordination |
| Leader identity | Current conversation agent, auto-assigned | No extra registration, natural UX (C) |
| Member execution | Sync RPC (independent sessions), parallelizable | Simple, no async message bus needed |
| Persistence | SQLite | Dynamic runtime data, relational (team↔agent M:N), lightweight |
| Dashboard team page | Read-only + disband/delete | Creation via conversation (R9), dashboard is overview |
| Agent list team display | Not in sidebar list, shown in agent edit Teams tab (read-only) | Avoids list clutter when agent belongs to many teams |

## Data Model (SQLite)

Three tables, all in the existing Aleph SQLite database:

```sql
CREATE TABLE teams (
    id           TEXT PRIMARY KEY,   -- UUID
    name         TEXT NOT NULL,
    description  TEXT DEFAULT '',
    leader_id    TEXT NOT NULL,      -- agent_id of the conversation agent that created the team
    status       TEXT NOT NULL DEFAULT 'active',  -- active | disbanded
    created_at   INTEGER NOT NULL,   -- unix timestamp
    disbanded_at INTEGER
);

CREATE TABLE team_members (
    team_id   TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    agent_id  TEXT NOT NULL,         -- registered agent id
    role      TEXT DEFAULT '',       -- LLM-assigned role, e.g. "code-reviewer"
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (team_id, agent_id)
);

CREATE TABLE team_tasks (
    id           TEXT PRIMARY KEY,   -- UUID
    team_id      TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    agent_id     TEXT NOT NULL,      -- executor agent
    subject      TEXT NOT NULL,      -- task description
    status       TEXT NOT NULL DEFAULT 'pending',  -- pending | running | completed | failed
    result       TEXT,               -- execution result summary
    created_at   INTEGER NOT NULL,
    completed_at INTEGER
);
```

**Key constraints:**
- `ON DELETE CASCADE` — permanent deletion (dashboard "Delete" button) auto-cleans members and tasks. Disbanding is a status UPDATE, not a DELETE.
- `leader_id` is NOT in `team_members` — leader coordinates, doesn't execute
- `agent_id` is a logical FK — agents live in config.toml, not SQLite. If an agent is deleted from config while still in a team, queries should gracefully show "[deleted agent]" instead of crashing.
- `role` is free text assigned by LLM at team creation time
- `team_tasks.result` stores summary text for `team_status` queries and dashboard display

## Tools (4 tools, replacing existing team_manage module)

### 1. `team_create`

Creates a team with mixed members (existing or newly created agents).

```json
{
  "name": "team_create",
  "params": {
    "name": "string — team name",
    "description": "string? — team description",
    "members": [
      {
        "agent_id": "string? — existing agent id (mutually exclusive with create)",
        "create": {
          "id": "string — agent id (must be unique)",
          "name": "string? — display name",
          "model": "string? — model override",
          "profile": "string? — references [profiles.<name>] in config",
          "identity": {
            "emoji": "string?",
            "description": "string?"
          }
        },
        "role": "string? — role description"
      }
    ]
  }
}
```

The `create` field uses a simplified subset of `AgentDefinition` — only the fields an LLM would reasonably set during team creation. Full agent customization (tool permissions, subagent policy, etc.) can be done later via the agent edit page.

**Behavior:**
- `leader_id` auto-set to the current conversation's agent_id
- For `create` members: register as new agent by writing to config.toml (same mechanism as `agents.create` RPC), then add to team
- Insert into `teams` + `team_members`
- Return: team overview (id, name, leader, members with roles)

### 2. `team_delegate`

Delegates a task to a team member. Launches an independent agent session.

```json
{
  "name": "team_delegate",
  "params": {
    "team_id": "string — team id",
    "agent_id": "string — target member agent_id",
    "task": "string — task description/instruction"
  }
}
```

**Behavior:**
- Validate agent_id is a member of the team
- Insert `team_tasks` record (status=running)
- Start target agent's independent session, send `task` as user message
- Wait for session completion, capture result
- Update `team_tasks` (status=completed/failed, result=summary)
- Return execution result to leader

**Delegation Execution Model:**
- **Session creation:** Uses `ExecutionEngine::create_session()` with the target agent's config, creating a fresh session. This is the same infrastructure used for handling incoming channel messages — not sub-agent spawning.
- **Completion criteria:** The member agent session runs its full agent loop (think → act cycles) until it produces a final response with no pending tool calls, same as a normal conversation turn.
- **Timeout:** Configurable per-delegation, default 5 minutes. On timeout, the task is marked `failed` with a timeout error message.
- **Result capture:** The member agent's final assistant message text is stored in `team_tasks.result`. Full conversation history remains in the session for later inspection.
- **Tool access:** The member agent session respects the member's own tool permissions (`allowed_tools`, `denied_tools`), not the leader's.

**Parallelism:** Leader can invoke multiple `team_delegate` calls in one reasoning turn; execution engine handles them concurrently.

### 3. `team_status`

Queries team state and task history.

```json
{
  "name": "team_status",
  "params": {
    "team_id": "string — team id"
  }
}
```

**Returns:** Team info, member list with roles, task history (who did what, status, result summary).

### 4. `team_disband`

Marks a team as disbanded.

```json
{
  "name": "team_disband",
  "params": {
    "team_id": "string — team id"
  }
}
```

**Behavior:**
- Set `teams.status = 'disbanded'`, record `disbanded_at`
- Member agents are NOT deleted — they persist in the registry
- Dashboard manual disband calls this same logic

## Code Removal

### Remove entirely:
- `src/builtin_tools/team_manage/` — all files (create.rs, list.rs, launch.rs, mod.rs)

### Remove team-specific types from `swarm/tasks/`:
- `Team`, `TeamMember`, `NewTeam`, `TeamFilter`, `TeamUpdate` structs
- Team-related methods on `CoordTaskStore` trait and implementations
- Team in-memory state in the store
- **Preserve:** `CoordTask`, `CoordTaskStatus`, task CRUD operations, DAG dependency logic — these serve the general `task_manage` tools and coordinator, not just teams

### Clean up:
- Sub-agent module: remove team-related fields (`persona`, `team_id`, any team coupling)
- Sub-agent restored to pure purpose: temporary worker, inherits parent agent's full config, auto-destroyed on task completion

## Sub-agent vs Team Member

| Aspect | Sub-agent | Team member |
|--------|-----------|-------------|
| Identity | Temporary, no persistent id | Registered agent with persistent id |
| Config | Inherits parent agent entirely | Independent (model, skills, profile) |
| Lifecycle | Created for task, destroyed after | Persists indefinitely |
| Visibility | Not in panel agent list | In agent list, has edit page |
| Creation | Any agent can spawn at any time | Must be added to a team via `team_create` |
| Use case | Quick one-off subtask | Specialized role in a team |

## Panel UI Changes

### 1. Agent List Sidebar — Dropdown Filter

**Location:** `interfaces/webchat/src/components/agents_sidebar.rs`

- Add a dropdown at the top of the agent list: **All Agents** / **Channel Agents** / **Standalone Agents**
- Default: All Agents (current behavior preserved)
- Channel Agents: agents bound to at least one social platform channel (Telegram, Slack, etc.)
- Standalone Agents: agents with no channel bindings
- Channel agents show channel name badge on the right
- No team info displayed in the list (moved to Teams tab)

### 2. Agent Edit Page — New Teams Tab

**Location:** `interfaces/webchat/src/views/agents/` (new file: `teams.rs`)

- New "Teams" tab added after the existing Channels tab (last position)
- Read-only display of all teams this agent belongs to
- Each team entry shows: team name, status badge, agent's role in that team, leader name, member count
- Empty state: hint text "To add this agent to a team, ask any agent to create a team with this agent as a member."
- No edit controls — team membership managed through conversation only

### 3. Dashboard — Teams Management Page

**Location:** `interfaces/webchat/src/views/` (new file: `teams.rs` or `dashboard_teams.rs`)

Add "Teams" to dashboard sidebar navigation.

**Page layout:**
- Header: "Teams" title + summary stats (N active, N disbanded)
- Team cards, **collapsed by default**:
  - Collapsed: ▶ team name + status badge + member count + Disband/Delete button
  - Expanded (click to toggle): leader info, member list with role badges, recent tasks with status indicators, creation timestamp
- Active teams: "Disband" button (sets status to disbanded)
- Disbanded teams: "Delete" button (permanently removes from SQLite, CASCADE cleans members/tasks)

### 4. Dashboard Sidebar Navigation

**Location:** `interfaces/webchat/src/components/dashboard_sidebar.rs`

- Add "Teams" entry to the sidebar navigation menu

## RPC Endpoints (Panel ↔ Core)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `teams.list` | GET | List all teams (with member count, task count, status) |
| `teams.get` | GET | Get single team detail (members, tasks, leader) |
| `teams.disband` | POST | Mark team as disbanded |
| `teams.delete` | POST | Permanently delete a disbanded team (must validate status=disbanded before deleting) |
| `agents.teams` | GET | Get all teams a specific agent belongs to (for Teams tab) |

## Migration Notes

- Existing in-memory TeamStore data is ephemeral — no migration needed
- SQLite tables created on first access (standard Aleph pattern)
- Existing sub-agent sessions are unaffected — they just lose team-related fields
- No config.toml schema changes needed
