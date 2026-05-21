# Agent / Workspace / Session Relationship Design

> Date: 2026-03-06
> Status: Approved

## Background

Aleph codebase had overlapping and inconsistent concepts for project, user, workspace, and session. This design clarifies the relationships and eliminates redundancy.

## Design Decisions

| Decision | Conclusion |
|----------|------------|
| Project concept | Not introduced — avoid concept pollution |
| User concept | Not independent — equivalent to Agent (AI persona instance) |
| Workspace unification | Merge `gateway::Workspace` and `memory::Workspace` into one struct |
| Agent ↔ Workspace | 1:1 binding (workspace_id = agent_id) |
| Cross-workspace capability | Via memory system's cross-workspace retrieval |
| SessionKey variants | Keep existing 6 variants unchanged |
| Channel routing | All channels default to main agent, manual switch supported |

## Core Model

```
Agent (assistant instance) ──── 1:1 ──── Workspace (work context)
  │                                        │
  │ has many                               │ scopes
  │                                        │
Session (conversation)                  Memory (facts)
  6 variants                            cross-workspace retrieval
```

## Three Entities

### Agent — Assistant Instance

The external identity. Examples: "Health Assistant", "Bitcoin Trading Assistant".

- id, name, description
- soul (persona definition via SOUL.md)
- model, skills, tools
- is_default (main agent flag — exactly one per system)

### Workspace — Unified Work Context

Merged from gateway::Workspace + memory::Workspace. One struct, one table.

- id (= agent_id, enforced 1:1)
- Profile config: model, temperature, tools, system_prompt, cache_strategy, history_limit
- Memory config: decay_rate, permanent_fact_types
- Runtime state: cache_state, env_vars
- Metadata: description, icon, created_at, updated_at, is_archived

### Session — Conversation (Unchanged)

SessionKey 6 variants, bound to Agent via agent_id field:

- **Main** — cross-channel shared session
- **DirectMessage** — DM isolation (3 DmScope modes)
- **Group** — group/channel/thread sessions
- **Task** — cron/webhook/scheduled
- **Subagent** — nested under parent agent
- **Ephemeral** — non-persistent

## Channel Routing

```
Message arrives
  ↓
Route binding match? ──→ yes → use bound Agent
  ↓ no
User manually /switch'd? ──→ yes → use selected Agent
  ↓ no
Route to main agent (system default)
```

- Exactly one main agent per system (`is_default = true`)
- Route bindings can override for specific channel/account/group
- Users can `/switch <agent>` in any channel
- Switch state persisted per channel + peer_id

## Database Schema

### Unified `workspaces` Table

```sql
CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,           -- = agent_id (1:1)
    -- Profile config
    profile TEXT,
    model TEXT,
    temperature REAL,
    tools TEXT,                    -- JSON array
    system_prompt TEXT,
    cache_strategy TEXT,
    history_limit INTEGER,
    -- Memory config
    decay_rate REAL,
    permanent_fact_types TEXT,     -- JSON array
    -- Runtime state
    cache_state TEXT,              -- JSON (CacheState enum)
    env_vars TEXT,                 -- JSON object
    -- Metadata
    description TEXT,
    icon TEXT,
    created_at INTEGER,
    updated_at INTEGER,
    is_archived INTEGER DEFAULT 0
);
```

### Channel Agent Switch Table

Replaces `user_active_workspace`:

```sql
CREATE TABLE channel_active_agent (
    channel TEXT NOT NULL,         -- "telegram", "discord", "cli"
    peer_id TEXT NOT NULL,         -- user peer_id or group_id
    agent_id TEXT NOT NULL,
    updated_at INTEGER,
    PRIMARY KEY (channel, peer_id)
);
```

## File System Structure (Unchanged)

```
~/.aleph/
├── aleph.toml                    -- global config (agents, bindings, profiles)
├── soul.md                       -- global persona (main agent default)
├── workspaces/
│   ├── main/                     -- main agent workspace
│   │   ├── SOUL.md
│   │   ├── MEMORY.md
│   │   └── memory/
│   ├── health/                   -- health assistant workspace
│   │   ├── SOUL.md
│   │   ├── MEMORY.md
│   │   └── memory/
│   └── trader/                   -- trading assistant workspace
│       ├── SOUL.md
│       ├── MEMORY.md
│       └── memory/
├── workspaces.db                 -- unified workspace SQLite
└── state.db                      -- resilience state (sessions, events, traces)
```

## Cross-Workspace Memory Retrieval

```rust
// Default: current workspace only
WorkspaceFilter::Single(agent_id)

// Cross-workspace: when knowledge from other agents is needed
WorkspaceFilter::Multiple(vec!["main", "health"])

// Global: search all workspaces
WorkspaceFilter::All
```

Agents can invoke cross-workspace memory search during conversation to share knowledge without breaking isolation boundaries.

## Code Changes Required

| File | Action |
|------|--------|
| `memory/workspace.rs` | Delete — merge into `gateway/workspace.rs` |
| `memory/workspace_store.rs` | Delete — unified under WorkspaceManager |
| `gateway/workspace.rs` | Refactor: add memory config fields, remove `user_active_workspace`, add `channel_active_agent` |
| `thinker/identity.rs` | Simplify: remove project-level identity, Agent soul = identity |
| `config/agent_resolver.rs` | Enforce workspace_id = agent_id 1:1 binding |
| `gateway/handlers/workspace.rs` | Update RPC: `workspace.switch` → `agent.switch`, adjust params |
| `gateway/inbound_router.rs` | Add channel_active_agent lookup in routing resolution |
