# Agent Management Builtin Tools — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create four builtin tools (agent_create, agent_switch, agent_list, agent_delete) that let the LLM in agent loop manage agents at runtime.

**Architecture:** Tools share `Arc` references to `AgentRegistry` and `WorkspaceManager` (injected via `BuiltinToolConfig`). A `SessionContextHandle` (channel+peer) is injected by `ExecutionEngine` each run, following the existing `workspace_handle` pattern.

**Tech Stack:** Rust, async_trait, schemars (JsonSchema), serde, tokio::sync::RwLock

---

## Design

### Problem

When a user sends "帮我切换到交易助手agent" via Telegram, the system:

1. Routes to the `main` agent (only one registered)
2. Task router classifies as `simple`
3. LLM generates fake "switched successfully" text
4. No agent/workspace is actually created or switched

The infrastructure exists (`AgentRegistry`, `WorkspaceManager`, `workspace.create`/`workspace.switch` RPC handlers, `InboundRouter.resolve_agent_id_async()`) but nothing connects the agent loop to these capabilities.

## Solution

Four builtin tools that bridge the agent loop to `AgentRegistry` + `WorkspaceManager`:

| Tool | Responsibility |
|------|---------------|
| `agent_create` | Create workspace + AgentInstance + register + auto-switch |
| `agent_switch` | Switch active agent for current channel/peer |
| `agent_list` | List all agents + show current active |
| `agent_delete` | Remove agent, auto-fallback to main |

## Shared Context Injection

New `SessionContextHandle` following the `default_workspace_handle` pattern:

```rust
pub struct SessionContext {
    pub channel: String,   // "telegram", "discord", "rpc"
    pub peer_id: String,   // sender identifier
}
```

- `BuiltinToolRegistry` holds the handle, exposes `session_context_handle()`
- `ExecutionEngine.run_agent_loop()` extracts channel/peer from `SessionKey` at run start
- All four agent tools share the same handle

## Tool Schemas

### agent_create

```rust
struct AgentCreateArgs {
    id: String,                    // URL-safe slug [a-z0-9-_]
    name: Option<String>,          // display name, defaults to id
    description: Option<String>,
    model: Option<String>,         // defaults to main agent's model
    system_prompt: Option<String>, // written to AGENTS.md
}

struct AgentCreateOutput {
    agent_id: String,
    workspace_path: String,
    switched: bool,
    message: String,
}
```

**Flow:**
1. `WorkspaceManager.create(id, "default", description)`
2. `initialize_workspace(~/.aleph/workspaces/{id}, name)`
3. `AgentInstance::new(config)` → `AgentRegistry.register()`
4. `WorkspaceManager.set_active_agent(channel, peer_id, id)`
5. If `system_prompt` provided, write to `{workspace}/AGENTS.md`

### agent_switch

```rust
struct AgentSwitchArgs {
    agent_id: String,
}

struct AgentSwitchOutput {
    agent_id: String,
    previous_agent: String,
    message: String,
}
```

**Flow:**
1. `AgentRegistry.get(agent_id)` — verify exists
2. `WorkspaceManager.get_active_agent(channel, peer_id)` — get previous
3. `WorkspaceManager.set_active_agent(channel, peer_id, agent_id)`

### agent_list

```rust
struct AgentListArgs {}  // no params

struct AgentListOutput {
    agents: Vec<AgentInfo>,      // id, name, workspace_path, model
    active_agent: Option<String>,
}
```

### agent_delete

```rust
struct AgentDeleteArgs {
    agent_id: String,
}

struct AgentDeleteOutput {
    deleted: bool,
    message: String,
}
```

**Flow:**
1. Reject if `agent_id == "main"`
2. If active agent == agent_id → `set_active_agent(channel, peer_id, "main")`
3. `AgentRegistry.remove(agent_id)`
4. `WorkspaceManager.archive(agent_id)`

## Safety Constraints

- Cannot delete the `main` agent
- Deleting the active agent auto-switches to `main`
- `id` validated as URL-safe slug: `[a-z0-9][a-z0-9_-]*`
- `agent_create` auto-switches after creation (one fewer step)

## Registration

In `BuiltinToolRegistry::with_config()`, registered conditionally when both `AgentRegistry` and `WorkspaceManager` are available (same pattern as `memory_search`).

## Data Flow

```
User: "帮我切换到交易助手agent"
  ↓
InboundRouter → main agent → AgentLoop
  ↓
LLM decides: create then switch
  ↓
Calls agent_create(id="trading", name="交易助手", ...)
  ↓
  1. WorkspaceManager.create("trading", ...)
  2. initialize_workspace()
  3. AgentInstance::new() → AgentRegistry.register()
  4. WorkspaceManager.set_active_agent("telegram", peer_id, "trading")
  ↓
Returns success → LLM informs user
  ↓
Next message: InboundRouter.resolve_agent_id_async() → "trading" ✓
```

## Files to Create/Modify

| File | Action |
|------|--------|
| `core/src/builtin_tools/agent_manage/mod.rs` | New — module with shared types |
| `core/src/builtin_tools/agent_manage/create.rs` | New — AgentCreateTool |
| `core/src/builtin_tools/agent_manage/switch.rs` | New — AgentSwitchTool |
| `core/src/builtin_tools/agent_manage/list.rs` | New — AgentListTool |
| `core/src/builtin_tools/agent_manage/delete.rs` | New — AgentDeleteTool |
| `core/src/builtin_tools/mod.rs` | Modify — add `pub mod agent_manage;` |
| `core/src/executor/builtin_registry/registry.rs` | Modify — register tools + session context handle |
| `core/src/gateway/execution_engine/engine.rs` | Modify — inject channel/peer into session context handle |

---

## Implementation Tasks

### Task 1: Create shared types module (`agent_manage/mod.rs`)

**Files:**
- Create: `core/src/builtin_tools/agent_manage/mod.rs`
- Modify: `core/src/builtin_tools/mod.rs`

**Step 1: Create the module with SessionContext and shared types**

```rust
// core/src/builtin_tools/agent_manage/mod.rs
//! Agent management tools — create, switch, list, delete agents at runtime.

pub mod create;
pub mod delete;
pub mod list;
pub mod switch;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Shared session context injected by ExecutionEngine each run.
/// Agent tools read this to know which channel/peer to bind agent switches to.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub channel: String,
    pub peer_id: String,
}

/// Thread-safe handle to the current session context.
pub type SessionContextHandle = Arc<RwLock<SessionContext>>;

/// Create a new default session context handle.
pub fn new_session_context_handle() -> SessionContextHandle {
    Arc::new(RwLock::new(SessionContext::default()))
}

// Re-exports
pub use create::{AgentCreateArgs, AgentCreateOutput, AgentCreateTool};
pub use delete::{AgentDeleteArgs, AgentDeleteOutput, AgentDeleteTool};
pub use list::{AgentListArgs, AgentListOutput, AgentListTool};
pub use switch::{AgentSwitchArgs, AgentSwitchOutput, AgentSwitchTool};
```

**Step 2: Register the module in `builtin_tools/mod.rs`**

Add `pub mod agent_manage;` after `pub mod arena;` (line 36) and add re-exports.

**Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Compilation errors about missing `create.rs`, `delete.rs`, etc. (expected — we create them next)

---

### Task 2: Implement `AgentCreateTool`

**Files:**
- Create: `core/src/builtin_tools/agent_manage/create.rs`

**Step 1: Write the tool**

```rust
// core/src/builtin_tools/agent_manage/create.rs
//! AgentCreateTool — create a new agent with workspace, register, and auto-switch.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::agent_resolver::initialize_workspace;
use crate::error::Result;
use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use crate::gateway::workspace::WorkspaceManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::SessionContextHandle;

/// Arguments for creating a new agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentCreateArgs {
    /// Agent identifier (URL-safe slug: lowercase letters, digits, hyphens, underscores)
    pub id: String,
    /// Human-readable display name (defaults to id if omitted)
    #[serde(default)]
    pub name: Option<String>,
    /// Description of the agent's purpose
    #[serde(default)]
    pub description: Option<String>,
    /// AI model override (defaults to main agent's model)
    #[serde(default)]
    pub model: Option<String>,
    /// Custom system prompt written to the agent's AGENTS.md
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// Output from agent creation.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCreateOutput {
    /// The created agent's ID
    pub agent_id: String,
    /// Path to the agent's workspace directory
    pub workspace_path: String,
    /// Whether the active agent was switched to this new agent
    pub switched: bool,
    /// Human-readable status message
    pub message: String,
}

/// Tool that creates a new agent with its workspace and registers it.
#[derive(Clone)]
pub struct AgentCreateTool {
    agent_registry: Arc<AgentRegistry>,
    workspace_manager: Arc<WorkspaceManager>,
    session_context: SessionContextHandle,
}

impl AgentCreateTool {
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        workspace_manager: Arc<WorkspaceManager>,
        session_context: SessionContextHandle,
    ) -> Self {
        Self {
            agent_registry,
            workspace_manager,
            session_context,
        }
    }
}

/// Validate agent ID: must be [a-z0-9][a-z0-9_-]* and 1-64 chars.
fn validate_agent_id(id: &str) -> std::result::Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("Agent ID must be 1-64 characters".to_string());
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("Agent ID must start with a lowercase letter or digit".to_string());
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' && c != '_' {
            return Err(format!(
                "Agent ID contains invalid character '{}'. Only [a-z0-9_-] allowed",
                c
            ));
        }
    }
    Ok(())
}

#[async_trait]
impl AlephTool for AgentCreateTool {
    const NAME: &'static str = "agent_create";
    const DESCRIPTION: &'static str =
        "Create a new agent with its own workspace and memory. \
         The new agent is automatically activated for the current conversation. \
         Use this when the user wants a specialized assistant (e.g., trading, coding, health).";

    type Args = AgentCreateArgs;
    type Output = AgentCreateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        let display_name = args.name.as_deref().unwrap_or(&args.id);
        notify_tool_start(Self::NAME, &format!("Creating agent: {}", display_name));

        // 1. Validate ID
        validate_agent_id(&args.id).map_err(crate::error::AlephError::other)?;

        // 2. Check for duplicates
        if self.agent_registry.get(&args.id).await.is_some() {
            return Err(crate::error::AlephError::other(format!(
                "Agent '{}' already exists. Use agent_switch to switch to it.",
                args.id
            )));
        }

        // 3. Create workspace in WorkspaceManager (SQLite)
        let description = args.description.as_deref();
        self.workspace_manager
            .create(&args.id, "default", description)
            .await
            .map_err(|e| crate::error::AlephError::other(format!("Failed to create workspace: {}", e)))?;

        // 4. Resolve workspace path and initialize directory
        let workspace_path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".aleph")
            .join("workspaces")
            .join(&args.id);

        let agent_name = args.name.clone().unwrap_or_else(|| args.id.clone());
        if let Err(e) = initialize_workspace(&workspace_path, &agent_name) {
            tracing::warn!(agent_id = %args.id, error = %e, "Failed to initialize workspace directory");
        }

        // 5. Write custom system_prompt to AGENTS.md if provided
        if let Some(ref prompt) = args.system_prompt {
            let agents_md_path = workspace_path.join("AGENTS.md");
            let content = format!(
                "# {} Workspace\n\n## Instructions\n\n{}\n",
                agent_name, prompt
            );
            if let Err(e) = std::fs::write(&agents_md_path, &content) {
                tracing::warn!(agent_id = %args.id, error = %e, "Failed to write AGENTS.md");
            }
        }

        // 6. Create AgentInstance and register
        let model = args.model.unwrap_or_else(|| "claude-sonnet-4-5".to_string());
        let config = AgentInstanceConfig {
            agent_id: args.id.clone(),
            workspace: workspace_path.clone(),
            model,
            max_loops: 20,
            system_prompt: args.system_prompt.clone(),
            ..Default::default()
        };

        let instance = AgentInstance::new(config)
            .map_err(|e| crate::error::AlephError::other(format!("Failed to create agent instance: {}", e)))?;
        self.agent_registry.register(instance).await;

        // 7. Auto-switch active agent for current channel/peer
        let mut switched = false;
        let ctx = self.session_context.read().await;
        if !ctx.channel.is_empty() && !ctx.peer_id.is_empty() {
            if let Err(e) = self.workspace_manager.set_active_agent(&ctx.channel, &ctx.peer_id, &args.id) {
                tracing::warn!(error = %e, "Failed to set active agent");
            } else {
                switched = true;
            }
        }

        let message = if switched {
            format!("Agent '{}' created and activated", agent_name)
        } else {
            format!("Agent '{}' created (manual switch needed)", agent_name)
        };

        info!(agent_id = %args.id, workspace = %workspace_path.display(), switched, "Agent created");
        notify_tool_result(Self::NAME, &message, true);

        Ok(AgentCreateOutput {
            agent_id: args.id,
            workspace_path: workspace_path.to_string_lossy().to_string(),
            switched,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_agent_id() {
        assert!(validate_agent_id("main").is_ok());
        assert!(validate_agent_id("trading-assistant").is_ok());
        assert!(validate_agent_id("agent_01").is_ok());
        assert!(validate_agent_id("").is_err());
        assert!(validate_agent_id("Agent").is_err()); // uppercase
        assert!(validate_agent_id("-bad").is_err()); // starts with hyphen
        assert!(validate_agent_id("has space").is_err());
        assert!(validate_agent_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn test_create_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let tmp = tempfile::tempdir().unwrap();
        let wm = Arc::new(WorkspaceManager::new(tmp.path().join("ws.db")).unwrap());
        let ctx = super::super::new_session_context_handle();
        let tool = AgentCreateTool::new(registry, wm, ctx);
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_create");
    }
}
```

---

### Task 3: Implement `AgentSwitchTool`

**Files:**
- Create: `core/src/builtin_tools/agent_manage/switch.rs`

**Step 1: Write the tool**

```rust
// core/src/builtin_tools/agent_manage/switch.rs
//! AgentSwitchTool — switch the active agent for the current channel/peer.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::workspace::WorkspaceManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::SessionContextHandle;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentSwitchArgs {
    /// Agent ID to switch to (must already exist)
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSwitchOutput {
    /// The agent ID that is now active
    pub agent_id: String,
    /// The previously active agent ID
    pub previous_agent: String,
    /// Human-readable status message
    pub message: String,
}

#[derive(Clone)]
pub struct AgentSwitchTool {
    agent_registry: Arc<AgentRegistry>,
    workspace_manager: Arc<WorkspaceManager>,
    session_context: SessionContextHandle,
}

impl AgentSwitchTool {
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        workspace_manager: Arc<WorkspaceManager>,
        session_context: SessionContextHandle,
    ) -> Self {
        Self {
            agent_registry,
            workspace_manager,
            session_context,
        }
    }
}

#[async_trait]
impl AlephTool for AgentSwitchTool {
    const NAME: &'static str = "agent_switch";
    const DESCRIPTION: &'static str =
        "Switch to an existing agent for the current conversation. \
         Future messages will be handled by the specified agent with its own workspace and memory.";

    type Args = AgentSwitchArgs;
    type Output = AgentSwitchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};
        notify_tool_start(Self::NAME, &format!("Switching to agent: {}", args.agent_id));

        // 1. Verify agent exists
        if self.agent_registry.get(&args.agent_id).await.is_none() {
            return Err(crate::error::AlephError::other(format!(
                "Agent '{}' not found. Use agent_list to see available agents, or agent_create to create one.",
                args.agent_id
            )));
        }

        // 2. Get current active agent
        let ctx = self.session_context.read().await;
        if ctx.channel.is_empty() || ctx.peer_id.is_empty() {
            return Err(crate::error::AlephError::other(
                "Cannot switch agent: no active session context (channel/peer unknown)"
            ));
        }

        let previous = self
            .workspace_manager
            .get_active_agent(&ctx.channel, &ctx.peer_id)
            .map_err(|e| crate::error::AlephError::other(format!("Failed to get active agent: {}", e)))?
            .unwrap_or_else(|| "main".to_string());

        // 3. Set new active agent
        self.workspace_manager
            .set_active_agent(&ctx.channel, &ctx.peer_id, &args.agent_id)
            .map_err(|e| crate::error::AlephError::other(format!("Failed to switch agent: {}", e)))?;

        let message = format!("Switched from '{}' to '{}'", previous, args.agent_id);
        info!(from = %previous, to = %args.agent_id, "Agent switched");
        notify_tool_result(Self::NAME, &message, true);

        Ok(AgentSwitchOutput {
            agent_id: args.agent_id,
            previous_agent: previous,
            message,
        })
    }
}
```

---

### Task 4: Implement `AgentListTool`

**Files:**
- Create: `core/src/builtin_tools/agent_manage/list.rs`

**Step 1: Write the tool**

```rust
// core/src/builtin_tools/agent_manage/list.rs
//! AgentListTool — list all registered agents and the currently active one.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::workspace::WorkspaceManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::SessionContextHandle;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentListArgs {}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub workspace_path: String,
    pub model: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentListOutput {
    pub agents: Vec<AgentInfo>,
    pub active_agent: Option<String>,
    pub total: usize,
}

#[derive(Clone)]
pub struct AgentListTool {
    agent_registry: Arc<AgentRegistry>,
    workspace_manager: Arc<WorkspaceManager>,
    session_context: SessionContextHandle,
}

impl AgentListTool {
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        workspace_manager: Arc<WorkspaceManager>,
        session_context: SessionContextHandle,
    ) -> Self {
        Self {
            agent_registry,
            workspace_manager,
            session_context,
        }
    }
}

#[async_trait]
impl AlephTool for AgentListTool {
    const NAME: &'static str = "agent_list";
    const DESCRIPTION: &'static str =
        "List all available agents and show which one is currently active.";

    type Args = AgentListArgs;
    type Output = AgentListOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};
        notify_tool_start(Self::NAME, "Listing agents");

        // Get active agent for current session
        let ctx = self.session_context.read().await;
        let active_agent = if !ctx.channel.is_empty() && !ctx.peer_id.is_empty() {
            self.workspace_manager
                .get_active_agent(&ctx.channel, &ctx.peer_id)
                .ok()
                .flatten()
        } else {
            None
        };

        // List all registered agents
        let agent_ids = self.agent_registry.list().await;
        let mut agents = Vec::with_capacity(agent_ids.len());

        for id in &agent_ids {
            if let Some(instance) = self.agent_registry.get(id).await {
                let config = instance.config();
                agents.push(AgentInfo {
                    id: id.clone(),
                    workspace_path: config.workspace.to_string_lossy().to_string(),
                    model: config.model.clone(),
                    is_active: active_agent.as_deref() == Some(id.as_str()),
                });
            }
        }

        let total = agents.len();
        notify_tool_result(Self::NAME, &format!("{} agents found", total), true);

        Ok(AgentListOutput {
            agents,
            active_agent,
            total,
        })
    }
}
```

---

### Task 5: Implement `AgentDeleteTool`

**Files:**
- Create: `core/src/builtin_tools/agent_manage/delete.rs`

**Step 1: Write the tool**

```rust
// core/src/builtin_tools/agent_manage/delete.rs
//! AgentDeleteTool — remove an agent from the registry and archive its workspace.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::workspace::WorkspaceManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::SessionContextHandle;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentDeleteArgs {
    /// Agent ID to delete (cannot be "main")
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentDeleteOutput {
    pub deleted: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct AgentDeleteTool {
    agent_registry: Arc<AgentRegistry>,
    workspace_manager: Arc<WorkspaceManager>,
    session_context: SessionContextHandle,
}

impl AgentDeleteTool {
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        workspace_manager: Arc<WorkspaceManager>,
        session_context: SessionContextHandle,
    ) -> Self {
        Self {
            agent_registry,
            workspace_manager,
            session_context,
        }
    }
}

#[async_trait]
impl AlephTool for AgentDeleteTool {
    const NAME: &'static str = "agent_delete";
    const DESCRIPTION: &'static str =
        "Delete an agent and archive its workspace. Cannot delete the 'main' agent. \
         If the deleted agent is currently active, automatically switches back to 'main'.";

    type Args = AgentDeleteArgs;
    type Output = AgentDeleteOutput;

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};
        notify_tool_start(Self::NAME, &format!("Deleting agent: {}", args.agent_id));

        // 1. Cannot delete main
        if args.agent_id == "main" {
            return Err(crate::error::AlephError::other(
                "Cannot delete the 'main' agent"
            ));
        }

        // 2. Verify agent exists
        if self.agent_registry.get(&args.agent_id).await.is_none() {
            return Err(crate::error::AlephError::other(format!(
                "Agent '{}' not found",
                args.agent_id
            )));
        }

        // 3. If this is the active agent, switch back to main
        let ctx = self.session_context.read().await;
        if !ctx.channel.is_empty() && !ctx.peer_id.is_empty() {
            if let Ok(Some(active)) = self.workspace_manager.get_active_agent(&ctx.channel, &ctx.peer_id) {
                if active == args.agent_id {
                    let _ = self.workspace_manager.set_active_agent(&ctx.channel, &ctx.peer_id, "main");
                    info!(agent_id = %args.agent_id, "Switched back to main before deleting");
                }
            }
        }

        // 4. Remove from registry
        self.agent_registry.remove(&args.agent_id).await;

        // 5. Archive workspace in WorkspaceManager
        let _ = self.workspace_manager.archive(&args.agent_id).await;

        let message = format!("Agent '{}' deleted and workspace archived", args.agent_id);
        info!(agent_id = %args.agent_id, "Agent deleted");
        notify_tool_result(Self::NAME, &message, true);

        Ok(AgentDeleteOutput {
            deleted: true,
            message,
        })
    }
}
```

---

### Task 6: Add `SessionContextHandle` to `BuiltinToolConfig` and register tools

**Files:**
- Modify: `core/src/executor/builtin_registry/config.rs` — add `agent_registry`, `workspace_manager` fields
- Modify: `core/src/executor/builtin_registry/registry.rs` — add tool fields, registration, execution dispatch, `session_context_handle()` method
- Modify: `core/src/executor/single_step.rs` — add `session_context_handle()` to `ToolRegistry` trait

**Step 1: Add fields to `BuiltinToolConfig`**

In `config.rs`, add after the `gateway_context` field:

```rust
    /// Agent registry for agent management tools
    pub agent_registry: Option<Arc<crate::gateway::agent_instance::AgentRegistry>>,
    /// Workspace manager for agent management tools
    pub workspace_manager: Option<Arc<crate::gateway::workspace::WorkspaceManager>>,
```

**Step 2: Add `session_context_handle()` to `ToolRegistry` trait**

In `single_step.rs`, add after `smart_recall_config_handle()`:

```rust
    /// Get the shared session context handle for agent management tools.
    fn session_context_handle(&self) -> Option<Arc<tokio::sync::RwLock<crate::builtin_tools::agent_manage::SessionContext>>> {
        None
    }
```

**Step 3: Add tool fields to `BuiltinToolRegistry`**

In `registry.rs`, add fields:

```rust
    /// Agent management tools (optional - requires AgentRegistry + WorkspaceManager)
    pub(crate) agent_create_tool: Option<crate::builtin_tools::agent_manage::AgentCreateTool>,
    pub(crate) agent_switch_tool: Option<crate::builtin_tools::agent_manage::AgentSwitchTool>,
    pub(crate) agent_list_tool: Option<crate::builtin_tools::agent_manage::AgentListTool>,
    pub(crate) agent_delete_tool: Option<crate::builtin_tools::agent_manage::AgentDeleteTool>,
    /// Session context handle for agent tools
    session_context_handle: Option<crate::builtin_tools::agent_manage::SessionContextHandle>,
```

**Step 4: Initialize tools in `with_config()`**

After the sessions tools block, add:

```rust
        // Add agent management tools (if AgentRegistry + WorkspaceManager are available)
        let (agent_create_tool, agent_switch_tool, agent_list_tool, agent_delete_tool, session_context_handle) =
            if let (Some(ref ar), Some(ref wm)) = (&config.agent_registry, &config.workspace_manager) {
                let ctx = crate::builtin_tools::agent_manage::new_session_context_handle();
                let create = crate::builtin_tools::agent_manage::AgentCreateTool::new(
                    Arc::clone(ar), Arc::clone(wm), Arc::clone(&ctx),
                );
                let switch = crate::builtin_tools::agent_manage::AgentSwitchTool::new(
                    Arc::clone(ar), Arc::clone(wm), Arc::clone(&ctx),
                );
                let list = crate::builtin_tools::agent_manage::AgentListTool::new(
                    Arc::clone(ar), Arc::clone(wm), Arc::clone(&ctx),
                );
                let delete = crate::builtin_tools::agent_manage::AgentDeleteTool::new(
                    Arc::clone(ar), Arc::clone(wm), Arc::clone(&ctx),
                );

                for (name, desc) in [
                    ("agent_create", crate::builtin_tools::agent_manage::AgentCreateTool::DESCRIPTION),
                    ("agent_switch", crate::builtin_tools::agent_manage::AgentSwitchTool::DESCRIPTION),
                    ("agent_list", crate::builtin_tools::agent_manage::AgentListTool::DESCRIPTION),
                    ("agent_delete", crate::builtin_tools::agent_manage::AgentDeleteTool::DESCRIPTION),
                ] {
                    tools.insert(
                        name.to_string(),
                        UnifiedTool::new(&format!("builtin:{}", name), name, desc, ToolSource::Builtin),
                    );
                }

                info!("Registered agent management tools (agent_create, agent_switch, agent_list, agent_delete)");
                (Some(create), Some(switch), Some(list), Some(delete), Some(ctx))
            } else {
                (None, None, None, None, None)
            };
```

**Step 5: Add fields to `Self` constructor and execution dispatch**

Add to the struct initializer at bottom of `with_config()`:
```rust
            agent_create_tool,
            agent_switch_tool,
            agent_list_tool,
            agent_delete_tool,
            session_context_handle,
```

Add to `execute_tool()` match arms (before the `_ =>` default):
```rust
            "agent_create" => Box::pin(async move {
                let tool = self.agent_create_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("agent_create not available: no AgentRegistry/WorkspaceManager configured")
                })?;
                tool.call_json(arguments).await
            }),
            "agent_switch" => Box::pin(async move {
                let tool = self.agent_switch_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("agent_switch not available: no AgentRegistry/WorkspaceManager configured")
                })?;
                tool.call_json(arguments).await
            }),
            "agent_list" => Box::pin(async move {
                let tool = self.agent_list_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("agent_list not available: no AgentRegistry/WorkspaceManager configured")
                })?;
                tool.call_json(arguments).await
            }),
            "agent_delete" => Box::pin(async move {
                let tool = self.agent_delete_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("agent_delete not available: no AgentRegistry/WorkspaceManager configured")
                })?;
                tool.call_json(arguments).await
            }),
```

**Step 6: Implement `session_context_handle()` on `BuiltinToolRegistry`**

```rust
    fn session_context_handle(&self) -> Option<Arc<RwLock<crate::builtin_tools::agent_manage::SessionContext>>> {
        self.session_context_handle.clone()
    }
```

---

### Task 7: Inject session context in ExecutionEngine

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Step 1: After workspace handle propagation (around line 502), add session context injection**

Find the block starting with `// Propagate workspace_id to workspace-aware tools` and add after the smart recall config block:

```rust
        // Propagate channel/peer context to agent management tools
        if let Some(ctx_handle) = self.tool_registry.session_context_handle() {
            let (channel, peer_id) = match &request.session_key {
                crate::gateway::router::SessionKey::DirectMessage { channel, peer_id, .. } => {
                    (channel.clone(), peer_id.clone())
                }
                crate::gateway::router::SessionKey::Group { channel, peer_id, .. } => {
                    (channel.clone(), peer_id.clone())
                }
                _ => (String::new(), String::new()),
            };
            if !channel.is_empty() {
                let mut ctx = ctx_handle.write().await;
                ctx.channel = channel.clone();
                ctx.peer_id = peer_id.clone();
                debug!(
                    run_id = run_id,
                    channel = %channel,
                    peer_id = %peer_id,
                    "Updated session context handle for agent tools"
                );
            }
        }
```

---

### Task 8: Wire AgentRegistry + WorkspaceManager into BuiltinToolConfig at startup

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs` — pass `agent_registry` and `workspace_manager` to `BuiltinToolConfig`

**Step 1: Find where `BuiltinToolConfig` is constructed and add the two fields**

Search for `BuiltinToolConfig` construction in `start/mod.rs` and add:
```rust
                agent_registry: Some(Arc::clone(&agent_registry)),
                workspace_manager: Some(Arc::clone(&workspace_manager)),
```

---

### Task 9: Compile and test

**Step 1: Compile**

Run: `cargo check -p alephcore`

**Step 2: Run unit tests**

Run: `cargo test -p alephcore --lib agent_manage`

**Step 3: Commit**

```bash
git add core/src/builtin_tools/agent_manage/
git add -u
git commit -m "builtin_tools: add agent_create/switch/list/delete tools

Bridge AgentRegistry and WorkspaceManager to agent loop via four
builtin tools. SessionContextHandle injects channel/peer info
from ExecutionEngine so tools know which conversation to bind."
```

---

### Task 10: Build and manual test

**Step 1: Full build**

Run: `cd /path/to/aleph && just all`

**Step 2: Restart Aleph and test via Telegram**

Send: "帮我切换到交易助手agent"

Expected: LLM calls `agent_create` → workspace created → agent registered → auto-switched → next message routed to new agent.
