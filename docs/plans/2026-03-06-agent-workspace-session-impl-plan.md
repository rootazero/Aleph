# Agent/Workspace/Session Unification — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Merge two Workspace definitions into one, enforce Agent↔Workspace 1:1, add channel-level agent switching, simplify identity resolution.

**Architecture:** gateway::Workspace absorbs memory::Workspace fields. WorkspaceManager gains `channel_active_agent` table (replaces `user_active_workspace`). IdentityResolver drops project layer. InboundMessageRouter gains agent switch lookup.

**Tech Stack:** Rust, rusqlite, serde, tokio

**Design Doc:** `docs/plans/2026-03-06-agent-workspace-session-design.md`

---

### Task 1: Unify Workspace Struct

Merge `memory::Workspace` + `memory::WorkspaceConfig` fields into `gateway::Workspace`.

**Files:**
- Modify: `core/src/gateway/workspace.rs:49-107` (Workspace struct)
- Modify: `core/src/gateway/workspace.rs:311-330` (WorkspaceManagerConfig)
- Reference: `core/src/memory/workspace.rs:20-116` (fields to absorb)

**Step 1: Add memory config fields to gateway::Workspace**

In `core/src/gateway/workspace.rs`, add fields to the `Workspace` struct (after line 56):

```rust
pub struct Workspace {
    pub id: String,
    pub profile: String,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub cache_state: CacheState,
    pub env_vars: HashMap<String, String>,
    pub description: Option<String>,
    // --- NEW: merged from memory::Workspace ---
    pub name: String,
    pub icon: Option<String>,
    pub is_archived: bool,
    // --- NEW: merged from memory::WorkspaceConfig ---
    pub decay_rate: Option<f64>,
    pub permanent_fact_types: Vec<String>,
    pub default_model: Option<String>,
    pub system_prompt_override: Option<String>,
    pub allowed_tools: Vec<String>,
}
```

**Step 2: Update Workspace::new() constructor**

Update `new()` (line 75) to accept and initialize the new fields. Provide defaults for backward compatibility:

```rust
pub fn new(id: impl Into<String>, profile: impl Into<String>, description: Option<String>) -> Self {
    let id = id.into();
    let name = id.clone();
    Self {
        id,
        profile: profile.into(),
        created_at: Utc::now(),
        last_active_at: Utc::now(),
        cache_state: CacheState::None,
        env_vars: HashMap::new(),
        description,
        name,
        icon: None,
        is_archived: false,
        decay_rate: None,
        permanent_fact_types: Vec::new(),
        default_model: None,
        system_prompt_override: None,
        allowed_tools: Vec::new(),
    }
}
```

**Step 3: Update WorkspaceManager SQLite schema**

In `WorkspaceManager::new()` (around line 342), update the CREATE TABLE statement:

```sql
CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    profile TEXT NOT NULL,
    created_at INTEGER,
    last_active_at INTEGER,
    cache_state TEXT,
    env_vars TEXT,
    description TEXT,
    name TEXT NOT NULL,
    icon TEXT,
    is_archived INTEGER DEFAULT 0,
    decay_rate REAL,
    permanent_fact_types TEXT,
    default_model TEXT,
    system_prompt_override TEXT,
    allowed_tools TEXT
)
```

**Step 4: Update all WorkspaceManager CRUD methods**

Update `create()` (line 438), `get()` (line 489), `list()` (line 528) to read/write the new columns. The `get()` method must deserialize JSON arrays for `permanent_fact_types` and `allowed_tools`.

**Step 5: Remove the `archived` column reference**

The old schema had `archived INTEGER DEFAULT 0`. The new struct uses `is_archived: bool`. Ensure the column name is `is_archived` in SQL and the Rust field matches.

**Step 6: Run `cargo check -p alephcore` to verify compilation**

Expected: Errors in files still importing old memory::Workspace. That's fine — we fix those in later tasks.

**Step 7: Commit**

```
workspace: merge memory config fields into gateway Workspace struct
```

---

### Task 2: Move WorkspaceFilter and WorkspaceContext to gateway::workspace

These types are used by the memory system but belong with the unified Workspace.

**Files:**
- Modify: `core/src/gateway/workspace.rs` (add WorkspaceFilter, WorkspaceContext)
- Reference: `core/src/memory/workspace.rs:120-152` (WorkspaceFilter)
- Reference: `core/src/memory/workspace.rs:303-342` (WorkspaceContext)

**Step 1: Copy WorkspaceFilter enum to gateway/workspace.rs**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceFilter {
    Single(String),
    Multiple(Vec<String>),
    All,
}

impl WorkspaceFilter {
    pub fn to_sql_filter(&self) -> String {
        match self {
            Self::Single(id) => format!("workspace = '{}'", id.replace('\'', "''")),
            Self::Multiple(ids) => {
                let quoted: Vec<String> = ids.iter()
                    .map(|id| format!("'{}'", id.replace('\'', "''")))
                    .collect();
                format!("workspace IN ({})", quoted.join(", "))
            }
            Self::All => "1=1".to_string(),
        }
    }
}
```

**Step 2: Copy WorkspaceContext to gateway/workspace.rs**

Bring over the struct and its methods. Adjust imports — it references `NamespaceScope` and `SearchFilter` from the memory module. These are fine as cross-module dependencies (memory types used by workspace context).

**Step 3: Run `cargo check -p alephcore`**

**Step 4: Commit**

```
workspace: move WorkspaceFilter and WorkspaceContext to gateway module
```

---

### Task 3: Replace `user_active_workspace` with `channel_active_agent`

**Files:**
- Modify: `core/src/gateway/workspace.rs:181-188` (remove UserActiveWorkspace)
- Modify: `core/src/gateway/workspace.rs:665-730` (replace set_active/get_active/get_active_id)

**Step 1: Remove UserActiveWorkspace struct**

Delete struct at lines 181-188.

**Step 2: Update SQL schema — drop old table, create new**

In `WorkspaceManager::new()`, replace:
```sql
CREATE TABLE IF NOT EXISTS user_active_workspace (...)
```
with:
```sql
CREATE TABLE IF NOT EXISTS channel_active_agent (
    channel TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    updated_at INTEGER,
    PRIMARY KEY (channel, peer_id)
)
```

**Step 3: Replace set_active / get_active / get_active_id methods**

Replace with channel-scoped methods:

```rust
pub fn set_active_agent(&self, channel: &str, peer_id: &str, agent_id: &str) -> Result<(), WorkspaceError> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let now = Utc::now().timestamp();
    conn.execute(
        "INSERT INTO channel_active_agent (channel, peer_id, agent_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(channel, peer_id) DO UPDATE SET agent_id = ?3, updated_at = ?4",
        params![channel, peer_id, agent_id, now],
    ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
    Ok(())
}

pub fn get_active_agent(&self, channel: &str, peer_id: &str) -> Result<Option<String>, WorkspaceError> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = conn.prepare(
        "SELECT agent_id FROM channel_active_agent WHERE channel = ?1 AND peer_id = ?2"
    ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
    let result = stmt.query_row(params![channel, peer_id], |row| row.get::<_, String>(0));
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(WorkspaceError::Database(e.to_string())),
    }
}
```

**Step 4: Update ActiveWorkspace::from_manager()**

Change signature from `from_manager(manager, user_id)` to `from_manager(manager, agent_id)`. Since Agent↔Workspace is 1:1, the workspace_id IS the agent_id. Simplify the method to just load the workspace by agent_id directly.

```rust
pub fn from_manager(manager: &WorkspaceManager, agent_id: &str) -> Result<Self, WorkspaceError> {
    let workspace = manager.get(agent_id)?;
    let profile = manager.get_profile(&workspace.profile)?;
    Ok(Self {
        workspace_id: workspace.id.clone(),
        profile,
        memory_filter: WorkspaceFilter::Single(workspace.id),
    })
}
```

**Step 5: Update tests that reference set_active / get_active**

**Step 6: Run `cargo check -p alephcore`**

**Step 7: Commit**

```
workspace: replace user_active_workspace with channel_active_agent
```

---

### Task 4: Update All Consumers of memory::Workspace → gateway::Workspace

**Files:**
- Modify: `core/src/memory/mod.rs` (remove pub mod workspace, workspace_store)
- Modify: `core/src/gateway/mod.rs` (ensure workspace is exported)
- Modify: All files importing from `crate::memory::workspace`

**Step 1: Find all imports of memory::workspace types**

Search for:
- `use crate::memory::workspace::`
- `use crate::memory::workspace_store`
- `memory::workspace::Workspace`
- `memory::workspace::WorkspaceFilter`
- `memory::workspace::WorkspaceContext`
- `memory::workspace::WorkspaceConfig`
- `memory::workspace::DEFAULT_WORKSPACE`

**Step 2: Update each import to point to gateway::workspace**

Key files expected (from exploration):
- `core/src/memory/store/types.rs` — SearchFilter uses WorkspaceFilter
- `core/src/memory/fact_retrieval.rs` — uses WorkspaceContext
- `core/src/builtin_tools/memory_search.rs` — workspace isolation
- `core/src/builtin_tools/memory_browse.rs` — workspace isolation
- `core/src/memory/integration_tests/workspace_isolation.rs` — test imports
- `core/src/gateway/handlers/workspace.rs` — uses memory::Workspace + workspace_store

For each file, change:
```rust
// Before
use crate::memory::workspace::{Workspace, WorkspaceFilter};
// After
use crate::gateway::workspace::{Workspace, WorkspaceFilter};
```

**Step 3: Move DEFAULT_WORKSPACE constant**

Add to `gateway/workspace.rs`:
```rust
pub const DEFAULT_WORKSPACE: &str = "default";
```

**Step 4: Run `cargo check -p alephcore` — fix remaining import errors iteratively**

**Step 5: Commit**

```
workspace: redirect all memory::workspace imports to gateway::workspace
```

---

### Task 5: Delete memory/workspace.rs and memory/workspace_store.rs

**Files:**
- Delete: `core/src/memory/workspace.rs`
- Delete: `core/src/memory/workspace_store.rs`
- Modify: `core/src/memory/mod.rs` (remove module declarations)

**Step 1: Remove module declarations from memory/mod.rs**

Delete lines like:
```rust
pub mod workspace;
pub mod workspace_store;
```

**Step 2: Delete the files**

```bash
rm core/src/memory/workspace.rs
rm core/src/memory/workspace_store.rs
```

**Step 3: Run `cargo check -p alephcore`**

Fix any remaining compilation errors. There may be test files in `memory/integration_tests/` that still reference old paths.

**Step 4: Run `cargo test -p alephcore --lib` to verify no regressions**

**Step 5: Commit**

```
workspace: delete memory/workspace.rs and workspace_store.rs (merged into gateway)
```

---

### Task 6: Update RPC Handlers for Unified Workspace

The workspace handlers currently use `memory::workspace_store` for CRUD and `WorkspaceManager` only for switch/getActive. Unify to use WorkspaceManager for everything.

**Files:**
- Modify: `core/src/gateway/handlers/workspace.rs:1-476`

**Step 1: Remove memory_store/workspace_store dependencies**

Remove imports:
```rust
// DELETE these
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::workspace::Workspace;
use crate::memory::workspace_store;
```

Replace with:
```rust
use crate::gateway::workspace::{Workspace, WorkspaceManager};
```

**Step 2: Rewrite handle_create()**

Use `WorkspaceManager::create()` instead of `workspace_store::create_workspace()`.

**Step 3: Rewrite handle_list() and handle_get()**

Use `WorkspaceManager::list()` and `WorkspaceManager::get()`.

**Step 4: Rewrite handle_update()**

Use `WorkspaceManager` for updates (may need a new `update()` method on WorkspaceManager).

**Step 5: Rewrite handle_switch()**

Change from `workspace.switch` semantics to `agent.switch` semantics:
- Params: `{ agent_id, channel, peer_id }`
- Calls `WorkspaceManager::set_active_agent(channel, peer_id, agent_id)`

**Step 6: Rewrite handle_get_active()**

- Params: `{ channel, peer_id }`
- Calls `WorkspaceManager::get_active_agent(channel, peer_id)`
- Returns agent_id (or default main agent if None)

**Step 7: Update handler registration (if method names changed)**

If the RPC dispatch table references method names like `"workspace.switch"`, decide whether to keep old names or rename to `"agent.switch"`. Recommend keeping `"workspace.switch"` for backward compat, or adding aliases.

**Step 8: Run `cargo check -p alephcore`**

**Step 9: Commit**

```
handlers: unify workspace RPC handlers to use WorkspaceManager
```

---

### Task 7: Simplify IdentityResolver — Remove Project Layer

**Files:**
- Modify: `core/src/thinker/identity.rs:47-228`

**Step 1: Remove project-related fields**

In `IdentityResolver` struct (line 47), remove:
```rust
project_ids: Vec<String>,
projects_base_dir: Option<PathBuf>,
```

**Step 2: Remove project-related methods**

Delete:
- `add_project()` (line 78)
- `resolve_project_dir()` (line 126)
- `load_project_soul()` (line 135)

**Step 3: Simplify resolve() method**

The resolution chain becomes: session override → global soul → default. Remove the project iteration loop (lines ~113-124).

```rust
pub fn resolve(&self) -> SoulManifest {
    // 1. Session override (highest priority)
    if let Some(ref soul) = self.session_override {
        return soul.clone();
    }
    // 2. Global soul
    if let Some(soul) = self.load_global_soul() {
        return soul;
    }
    // 3. Default
    SoulManifest::default()
}
```

**Step 4: Simplify list_sources()**

Remove `IdentitySourceType::Project` variant. Update the method to only list Global and Session sources.

**Step 5: Remove IdentitySourceType::Project variant**

```rust
pub enum IdentitySourceType {
    Global,
    Session,
}
```

**Step 6: Update tests — remove project-related test cases**

**Step 7: Run `cargo test -p alephcore --lib` to verify**

**Step 8: Commit**

```
identity: simplify IdentityResolver, remove project layer
```

---

### Task 8: Enforce Agent↔Workspace 1:1 in AgentResolver

**Files:**
- Modify: `core/src/config/agent_resolver.rs:145-163` (resolve_workspace_path)

**Step 1: Enforce workspace_id = agent_id**

In `resolve_workspace_path()` (line 145), ensure the workspace directory name always equals agent_id. Remove the ability for `AgentDefinition.workspace` to set an arbitrary path that doesn't match the agent_id.

```rust
fn resolve_workspace_path(agent_id: &str, defaults: &AgentDefaults) -> PathBuf {
    let root = defaults.workspace_root.as_ref()
        .map(|p| resolve_user_path(p))
        .unwrap_or_else(default_workspace_root);
    root.join(agent_id)
}
```

**Step 2: Update ResolvedAgent construction**

In `resolve_one()` (line 166), ensure `workspace_path` is always `{root}/{agent_id}`. Log a warning if `AgentDefinition.workspace` is set (deprecated, ignored).

**Step 3: Auto-create workspace in WorkspaceManager**

When resolving an agent, ensure a matching Workspace row exists in `workspaces.db`. Add a call in the resolver or at startup:

```rust
// In startup or resolve_all()
if workspace_manager.get(agent_id).is_err() {
    workspace_manager.create(agent_id, profile_name, description)?;
}
```

**Step 4: Update tests**

**Step 5: Run `cargo check -p alephcore`**

**Step 6: Commit**

```
agent_resolver: enforce workspace_id = agent_id 1:1 binding
```

---

### Task 9: Add Agent Switch to InboundMessageRouter

**Files:**
- Modify: `core/src/gateway/inbound_router.rs:838-866` (resolve_session_key_with_agent)
- Modify: `core/src/gateway/inbound_router.rs:153-250` (InboundMessageRouter struct, add workspace_manager field)

**Step 1: Add WorkspaceManager to InboundMessageRouter**

Add field:
```rust
workspace_manager: Option<Arc<WorkspaceManager>>,
```

Add builder method:
```rust
pub fn with_workspace_manager(mut self, manager: Arc<WorkspaceManager>) -> Self {
    self.workspace_manager = Some(manager);
    self
}
```

**Step 2: Update resolve_agent_id_async()**

Before falling back to default agent, check `channel_active_agent`:

```rust
async fn resolve_agent_id_async(&self, msg: &InboundMessage) -> String {
    // 1. Check route bindings (existing logic)
    if let Some(agent_id) = self.agent_router.as_ref()
        .and_then(|r| r.resolve_for_message(msg)) {
        return agent_id;
    }

    // 2. Check channel_active_agent (NEW)
    if let Some(ref manager) = self.workspace_manager {
        if let Ok(Some(agent_id)) = manager.get_active_agent(
            &msg.channel_id, &msg.sender_id.0
        ) {
            return agent_id;
        }
    }

    // 3. Default to main agent
    self.config.default_agent.clone()
}
```

**Step 3: Run `cargo check -p alephcore`**

**Step 4: Commit**

```
inbound_router: add channel_active_agent lookup for agent switching
```

---

### Task 10: Integration Verification

**Files:**
- All modified files

**Step 1: Full compilation check**

```bash
cargo check -p alephcore
```

**Step 2: Run all library tests**

```bash
cargo test -p alephcore --lib
```

**Step 3: Fix any remaining test failures**

Known pre-existing failures (ignore): `tools::markdown_skill::loader::tests` (2 tests).

**Step 4: Final commit**

```
multi: complete Agent/Workspace/Session unification
```

---

## Task Dependency Graph

```
Task 1 (Unify Workspace struct)
  ↓
Task 2 (Move WorkspaceFilter/Context)
  ↓
Task 3 (channel_active_agent table)
  ↓
Task 4 (Update all imports)
  ↓
Task 5 (Delete old files)
  ↓
Task 6 (Update RPC handlers)     Task 7 (Simplify IdentityResolver)     Task 8 (Enforce 1:1 in AgentResolver)
  ↓                                ↓                                       ↓
  └────────────────────────────────┴───────────────────────────────────────┘
                                   ↓
                          Task 9 (InboundRouter agent switch)
                                   ↓
                          Task 10 (Integration verification)
```

Tasks 6, 7, 8 are independent of each other and can be parallelized.
