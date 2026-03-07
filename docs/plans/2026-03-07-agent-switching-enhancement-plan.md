# Agent Switching Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the existing per-agent workspace infrastructure so that agent switching produces real identity/tool/prompt changes.

**Architecture:** Workspace-Centric. All building blocks exist (WorkspaceFiles::load, SoulLayer, WorkspaceFilesLayer, ToolPolicy fields). This plan wires them together across 3 phases: identity (§1+§3), safety (§2+§4), features (§5+§6).

**Tech Stack:** Rust, Tokio, AlephTool trait, PromptLayer system, EventBus

**Design Doc:** `docs/plans/2026-03-07-agent-switching-enhancement-design.md`

---

## Phase 1: Agent Identity (§1 Prompt Wiring + §3 Templates)

### Task 1: Expose workspace_path on ActiveWorkspace

`ActiveWorkspace` (workspace.rs:326) has `workspace_id` but no `workspace_path`. The engine needs the filesystem path to load workspace files.

**Files:**
- Modify: `core/src/gateway/workspace.rs`

**Step 1: Add workspace_path field and accessor**

In `ActiveWorkspace` struct (~line 326), add a `workspace_path` field:

```rust
pub struct ActiveWorkspace {
    pub workspace_id: String,
    pub profile: ProfileConfig,
    pub memory_filter: WorkspaceFilter,
    pub workspace_path: Option<PathBuf>,  // ← NEW
}
```

In `from_manager()` (~line 344) and `from_workspace_id()` (~line 383), populate it from `WorkspaceManager::workspace_dir()` or the workspace record's path. In `default_global()` (~line 415), set it to `dirs::home_dir().map(|h| h.join(".aleph"))`.

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (new field has Option, Default still works)

**Step 3: Commit**

```bash
git add core/src/gateway/workspace.rs
git commit -m "workspace: add workspace_path to ActiveWorkspace"
```

---

### Task 2: Wire workspace files into prompt assembly

Load workspace files from agent's workspace_path and pass to LayerInput so SoulLayer and WorkspaceFilesLayer receive data.

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Step 1: Load workspace files after active_workspace resolution**

Find the block after smart_recall_config injection (~line 513 area, after "Updated tool smart recall config"). Add:

```rust
// Load workspace files for per-agent identity injection
let workspace_files = active_workspace.workspace_path.as_ref().map(|ws_path| {
    crate::thinker::workspace_files::WorkspaceFiles::load(
        ws_path,
        &crate::thinker::workspace_files::WorkspaceFilesConfig::default(),
    )
});
```

**Step 2: Pass workspace_files to LayerInput**

Find where `LayerInput` is constructed (search for `LayerInput::` or `.with_workspace`). Chain `.with_workspace_opt(workspace_files.as_ref())` onto the LayerInput builder. The `with_workspace_opt()` method (prompt_layer.rs:121) takes `Option<&WorkspaceFiles>` — exactly what we have.

If LayerInput is constructed multiple times (e.g., for different prompt paths), ensure all paths receive workspace_files.

**Step 3: Verify compilation and test**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: wire workspace files into prompt assembly"
```

---

### Task 3: Expand workspace templates in AgentCreateTool

When creating a new agent, generate SOUL.md, IDENTITY.md, TOOLS.md templates in the workspace directory using user-provided parameters.

**Files:**
- Modify: `core/src/builtin_tools/agent_manage/create.rs`

**Step 1: Add template generation after initialize_workspace()**

In `AgentCreateTool::call()`, after `initialize_workspace()` succeeds and before `AgentInstance::with_session_manager()`, add file generation:

```rust
// Generate SOUL.md from system_prompt or default template
let soul_path = workspace_path.join("SOUL.md");
if !soul_path.exists() {
    let soul_content = if let Some(ref prompt) = args.system_prompt {
        prompt.clone()
    } else {
        format!(
            "You are {name}, an AI assistant{desc}.\n\n\
             ## Tone\n- Professional, friendly, concise\n\n\
             ## Boundaries\n- Focus on your area of expertise\n\
             - Suggest switching to another agent for out-of-scope requests\n",
            name = args.name.as_deref().unwrap_or(&args.id),
            desc = args.description.as_ref()
                .map(|d| format!(" specialized in {}", d))
                .unwrap_or_default(),
        )
    };
    let _ = std::fs::write(&soul_path, &soul_content);
}

// Generate IDENTITY.md
let identity_path = workspace_path.join("IDENTITY.md");
if !identity_path.exists() {
    let identity_content = format!(
        "- Name: {}\n- Emoji: \u{1F916}\n- Theme: professional\n",
        args.name.as_deref().unwrap_or(&args.id),
    );
    let _ = std::fs::write(&identity_path, &identity_content);
}

// Generate TOOLS.md (empty template)
let tools_path = workspace_path.join("TOOLS.md");
if !tools_path.exists() {
    let _ = std::fs::write(&tools_path, "# Tool Notes\n\nRecord your tool usage preferences and notes here.\n");
}
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 3: Commit**

```bash
git add core/src/builtin_tools/agent_manage/create.rs
git commit -m "agent_create: generate SOUL.md, IDENTITY.md, TOOLS.md templates"
```

---

## Phase 2: Safety & Correctness (§2 Tool Enforcement + §4 Race Fix)

### Task 4: Add ToolPolicy types

Define the ToolPolicy struct and handle type for tool enforcement.

**Files:**
- Modify: `core/src/builtin_tools/agent_manage/mod.rs`

**Step 1: Add ToolPolicy types**

After the `SessionContext` types, add:

```rust
/// Per-agent tool access policy injected by ExecutionEngine each run.
#[derive(Debug, Clone, Default)]
pub struct ToolPolicy {
    /// If non-empty, only these tools are allowed
    pub whitelist: Vec<String>,
    /// These tools are always denied
    pub blacklist: Vec<String>,
}

impl ToolPolicy {
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        if !self.whitelist.is_empty() && !self.whitelist.iter().any(|w| w == tool_name) {
            return false;
        }
        if self.blacklist.iter().any(|b| b == tool_name) {
            return false;
        }
        true
    }
}

pub type ToolPolicyHandle = Arc<RwLock<ToolPolicy>>;

pub fn new_tool_policy_handle() -> ToolPolicyHandle {
    Arc::new(RwLock::new(ToolPolicy::default()))
}
```

Add re-exports at bottom: `pub use ...::{ToolPolicy, ToolPolicyHandle, new_tool_policy_handle};`

**Step 2: Add unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_policy_empty_allows_all() {
        let policy = ToolPolicy::default();
        assert!(policy.is_allowed("search"));
        assert!(policy.is_allowed("bash"));
    }

    #[test]
    fn test_tool_policy_whitelist() {
        let policy = ToolPolicy {
            whitelist: vec!["search".into(), "web_fetch".into()],
            blacklist: vec![],
        };
        assert!(policy.is_allowed("search"));
        assert!(!policy.is_allowed("bash"));
    }

    #[test]
    fn test_tool_policy_blacklist() {
        let policy = ToolPolicy {
            whitelist: vec![],
            blacklist: vec!["bash".into()],
        };
        assert!(policy.is_allowed("search"));
        assert!(!policy.is_allowed("bash"));
    }

    #[test]
    fn test_tool_policy_whitelist_and_blacklist() {
        let policy = ToolPolicy {
            whitelist: vec!["search".into(), "bash".into()],
            blacklist: vec!["bash".into()],
        };
        assert!(policy.is_allowed("search"));
        assert!(!policy.is_allowed("bash")); // blacklist wins
    }
}
```

**Step 3: Verify**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/builtin_tools/agent_manage/mod.rs
git commit -m "agent_manage: add ToolPolicy types with is_allowed logic"
```

---

### Task 5: Wire ToolPolicy into registry and engine

Add ToolPolicy handle to BuiltinToolConfig, BuiltinToolRegistry, ToolRegistry trait, and enforce in execute_tool(). Inject per-run from ExecutionEngine.

**Files:**
- Modify: `core/src/executor/builtin_registry/config.rs`
- Modify: `core/src/executor/builtin_registry/registry.rs`
- Modify: `core/src/executor/single_step.rs`
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Step 1: Add to BuiltinToolConfig**

In `config.rs`, add field:
```rust
pub tool_policy: Option<crate::builtin_tools::agent_manage::ToolPolicyHandle>,
```

**Step 2: Add to BuiltinToolRegistry struct and with_config()**

In `registry.rs`, add field to struct:
```rust
tool_policy_handle: Option<crate::builtin_tools::agent_manage::ToolPolicyHandle>,
```

In `with_config()`, initialize:
```rust
let tool_policy_handle = config.tool_policy.clone()
    .or_else(|| Some(crate::builtin_tools::agent_manage::new_tool_policy_handle()));
```

Add to `Self { ... }` constructor.

**Step 3: Add to ToolRegistry trait**

In `single_step.rs`, add method with default:
```rust
fn tool_policy_handle(&self) -> Option<std::sync::Arc<tokio::sync::RwLock<crate::builtin_tools::agent_manage::ToolPolicy>>> {
    None
}
```

Implement in `impl ToolRegistry for BuiltinToolRegistry`:
```rust
fn tool_policy_handle(&self) -> Option<Arc<RwLock<crate::builtin_tools::agent_manage::ToolPolicy>>> {
    self.tool_policy_handle.clone()
}
```

**Step 4: Enforce in execute_tool()**

In `registry.rs`, at the top of `execute_tool()`, before the capability check:
```rust
// Enforce per-agent tool policy
if let Some(ref policy_handle) = self.tool_policy_handle {
    let policy = policy_handle.read().await;
    if !policy.is_allowed(tool_name) {
        return Box::pin(async move {
            Err(anyhow::anyhow!(
                "Tool '{}' is not allowed for the current agent. \
                 Use agent_list to check available tools, or switch to an agent that has access.",
                tool_name
            ))
        });
    }
}
```

**Step 5: Inject per-run in ExecutionEngine**

In `engine.rs`, after the session_context injection block, add:
```rust
// Propagate tool policy from agent instance
if let Some(tp_handle) = self.tool_registry.tool_policy_handle() {
    let mut tp = tp_handle.write().await;
    // Read from agent's config — need agent_id to look up AgentInstance
    // For now, use empty policy (allow all) as default
    // TODO: wire agent instance lookup when AgentRegistry is available on engine
    let _ = tp; // placeholder
}
```

Note: Full wiring of agent instance lookup requires engine to have AgentRegistry reference. For now, set up the handle infrastructure. The actual policy population will be completed in Task 8 (startup wiring).

**Step 6: Verify**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/executor/builtin_registry/config.rs core/src/executor/builtin_registry/registry.rs core/src/executor/single_step.rs core/src/gateway/execution_engine/engine.rs
git commit -m "tool_policy: wire enforcement into registry and engine"
```

---

### Task 6: SessionContext snapshot injection (race fix)

Replace direct handle reads in agent_manage tools with snapshot values injected via arguments.

**Files:**
- Modify: `core/src/executor/builtin_registry/registry.rs`
- Modify: `core/src/builtin_tools/agent_manage/create.rs`
- Modify: `core/src/builtin_tools/agent_manage/switch.rs`
- Modify: `core/src/builtin_tools/agent_manage/list.rs`
- Modify: `core/src/builtin_tools/agent_manage/delete.rs`

**Step 1: Inject snapshot in registry.rs execute_tool()**

In the agent_manage match arms, before calling `tool.call_json()`:
```rust
"agent_create" | "agent_switch" | "agent_list" | "agent_delete" => {
    // Snapshot session context to avoid race with concurrent runs
    let arguments = if let Some(ref h) = self.session_context_handle {
        let ctx = h.read().await;
        let mut args = arguments;
        if let Some(obj) = args.as_object_mut() {
            obj.insert("__channel".into(), serde_json::Value::String(ctx.channel.clone()));
            obj.insert("__peer_id".into(), serde_json::Value::String(ctx.peer_id.clone()));
        }
        args
    } else {
        arguments
    };
    // ... existing call_json dispatch
}
```

**Step 2: Add __channel / __peer_id to all 4 tool Args structs**

In each tool's Args struct, add:
```rust
#[serde(default)]
pub __channel: String,
#[serde(default)]
pub __peer_id: String,
```

**Step 3: Update call() to use args fields instead of handle**

In each tool's `call()`, replace:
```rust
let ctx = self.session_ctx.read().await;
let channel = ctx.channel.clone();
let peer_id = ctx.peer_id.clone();
```
with:
```rust
let channel = args.__channel.clone();
let peer_id = args.__peer_id.clone();
```

Remove the `session_ctx: SessionContextHandle` field from tool structs (AgentCreateTool, AgentSwitchTool, AgentListTool, AgentDeleteTool) since they no longer read from it directly. The handle remains on BuiltinToolRegistry for the snapshot pattern.

**Step 4: Verify**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/executor/builtin_registry/registry.rs core/src/builtin_tools/agent_manage/
git commit -m "agent_manage: snapshot session context to fix race condition"
```

---

## Phase 3: Features (§5 Sub-Agent + §6 Lifecycle & Routing)

### Task 7: Create subagent_manage module with 3 tools

Expose SubAgentDispatcher as 3 builtin tools: spawn, steer, kill.

**Files:**
- Create: `core/src/builtin_tools/subagent_manage/mod.rs`
- Create: `core/src/builtin_tools/subagent_manage/spawn.rs`
- Create: `core/src/builtin_tools/subagent_manage/steer.rs`
- Create: `core/src/builtin_tools/subagent_manage/kill.rs`
- Modify: `core/src/builtin_tools/mod.rs`

**Step 1: Read SubAgentDispatcher API**

Read `core/src/agents/sub_agents/` to understand the spawn/cancel API. Design tool Args to match.

**Step 2: Create module structure**

Follow the exact pattern from `agent_manage/` — each tool implements `AlephTool` with `Args` (JsonSchema + Deserialize) and `Output` (Serialize).

```rust
// spawn.rs
pub struct SubagentSpawnTool {
    dispatcher: Arc<RwLock<SubAgentDispatcher>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SubagentSpawnArgs {
    pub agent_id: String,
    pub task: String,
    #[serde(default)]
    pub model: Option<String>,
}

// steer.rs
pub struct SubagentSteerArgs {
    pub run_id: String,
    pub message: String,
}

// kill.rs
pub struct SubagentKillArgs {
    pub run_id: String,
}
```

**Step 3: Implement AlephTool for each**

Each tool's `call()` delegates to SubAgentDispatcher methods. Handle errors gracefully.

**Step 4: Register in mod.rs**

In `core/src/builtin_tools/mod.rs`, add `pub mod subagent_manage;` and re-exports.

**Step 5: Verify**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/builtin_tools/subagent_manage/ core/src/builtin_tools/mod.rs
git commit -m "subagent_manage: add spawn, steer, kill builtin tools"
```

---

### Task 8: Register subagent tools in registry + definitions

Wire the 3 new tools into BuiltinToolRegistry and BUILTIN_TOOL_DEFINITIONS.

**Files:**
- Modify: `core/src/executor/builtin_registry/definitions.rs`
- Modify: `core/src/executor/builtin_registry/registry.rs`

**Step 1: Add 3 definitions to BUILTIN_TOOL_DEFINITIONS**

```rust
BuiltinToolDefinition {
    name: "subagent_spawn",
    description: "Spawn a sub-agent to handle a task autonomously",
    requires_config: true,
},
BuiltinToolDefinition {
    name: "subagent_steer",
    description: "Send additional instructions to a running sub-agent",
    requires_config: true,
},
BuiltinToolDefinition {
    name: "subagent_kill",
    description: "Terminate a running sub-agent",
    requires_config: true,
},
```

Add `"subagent_spawn" | "subagent_steer" | "subagent_kill" => None,` in `create_tool_boxed()`.

**Step 2: Add tool fields + dispatch in registry.rs**

Follow the agent_manage pattern: add fields to struct, initialize in `with_config()` when `sub_agent_dispatcher` is available, add match arms in `execute_tool()`.

**Step 3: Verify**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/executor/builtin_registry/
git commit -m "subagent_manage: register tools in registry and definitions"
```

---

### Task 9: Extend AgentLifecycleEvent + wire EventBus

Add Switched, Deleted, SubagentSpawned, SubagentCompleted variants. Add event_bus to BuiltinToolConfig. Emit events from tools.

**Files:**
- Modify: `core/src/gateway/agent_lifecycle.rs`
- Modify: `core/src/executor/builtin_registry/config.rs`
- Modify: `core/src/builtin_tools/agent_manage/switch.rs`
- Modify: `core/src/builtin_tools/agent_manage/delete.rs`
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Step 1: Extend the enum**

In `agent_lifecycle.rs`, add variants:
```rust
pub enum AgentLifecycleEvent {
    Registered { agent_id: String, workspace: PathBuf, model: String },
    Switched { agent_id: String, channel: String, peer_id: String, previous_agent_id: String },
    Deleted { agent_id: String, workspace_archived: bool },
    SubagentSpawned { parent_agent_id: String, child_run_id: String, task: String },
    SubagentCompleted { child_run_id: String, outcome: String },
}
```

**Step 2: Add event_bus to BuiltinToolConfig**

```rust
pub event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
```

**Step 3: Wire event_bus in startup**

In `start/mod.rs`, add `event_bus: Some(event_bus.clone())` to the `BuiltinToolConfig` block.

**Step 4: Store event_bus in BuiltinToolRegistry, pass to tools**

Add `event_bus` field to registry struct. Pass to agent_manage and subagent_manage tools during construction. Tools call `event_bus.publish_json(&event)` after successful operations.

**Step 5: Emit events from switch and delete tools**

In `switch.rs` call(), after successful `set_active_agent()`:
```rust
if let Some(ref bus) = self.event_bus {
    let _ = bus.publish_json(&AgentLifecycleEvent::Switched {
        agent_id: args.agent_id.clone(),
        channel: channel.clone(),
        peer_id: peer_id.clone(),
        previous_agent_id: current.unwrap_or_default(),
    });
}
```

Similar pattern for `delete.rs`.

**Step 6: Verify**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/gateway/agent_lifecycle.rs core/src/executor/builtin_registry/config.rs core/src/builtin_tools/agent_manage/ core/src/bin/aleph/commands/start/mod.rs
git commit -m "lifecycle: add Switched/Deleted/Subagent events, wire EventBus"
```

---

### Task 10: Unify routing priority

Fix `resolve_agent_id_async()` to use correct priority: user override > binding > default. Make switch-to-main clear the override.

**Files:**
- Modify: `core/src/gateway/inbound_router.rs`
- Modify: `core/src/builtin_tools/agent_manage/switch.rs`

**Step 1: Flip priority in resolve_agent_id_async()**

Current order (inbound_router.rs ~line 1316):
1. AgentRouter (bindings) — highest
2. WorkspaceManager (user switch) — second
3. Default — fallback

Change to:
```rust
async fn resolve_agent_id_async(&self, channel: &str, sender_id: &str) -> String {
    // 1. User's explicit agent switch (highest priority)
    if let Some(ref manager) = self.workspace_manager {
        if let Ok(Some(agent_id)) = manager.get_active_agent(channel, sender_id) {
            return agent_id;
        }
    }

    // 2. Config-layer binding routes
    if let Some(router) = &self.agent_router {
        let resolved = router.route(None, Some(channel), None).await;
        let resolved_id = resolved.agent_id();
        if resolved_id != router.default_agent() {
            return resolved_id.to_string();
        }
    }

    // 3. Fallback to default agent
    self.config.default_agent.clone()
}
```

**Step 2: Clear override on switch-to-main**

In `switch.rs`, when `args.agent_id == "main"`, call `workspace_mgr.clear_active_agent(channel, peer_id)` instead of `set_active_agent(channel, peer_id, "main")`.

If `clear_active_agent()` doesn't exist on WorkspaceManager, add it — it should DELETE the row from the SQLite table rather than setting agent_id to "main". This lets routing fall through to AgentRouter bindings.

**Step 3: Verify**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/gateway/inbound_router.rs core/src/builtin_tools/agent_manage/switch.rs core/src/gateway/workspace.rs
git commit -m "routing: user override > binding > default, clear on switch-to-main"
```

---

### Task 11: Wire remaining startup dependencies

Ensure all new dependencies (event_bus, tool_policy, workspace_manager on engine) are wired at startup.

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Step 1: Wire event_bus into BuiltinToolConfig**

In the `tool_config` block, add:
```rust
event_bus: Some(event_bus.clone()),
```

**Step 2: Wire tool_policy handle**

```rust
tool_policy: Some(crate::builtin_tools::agent_manage::new_tool_policy_handle()),
```

**Step 3: Wire workspace_manager into ExecutionEngine**

After engine creation, chain:
```rust
if let Some(ref wm) = workspace_manager {
    engine = engine.with_workspace_manager(wm.clone());
}
```

**Step 4: Verify full build**

Run: `cargo check --bin aleph`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/bin/aleph/commands/start/mod.rs
git commit -m "startup: wire event_bus, tool_policy, workspace_manager"
```

---

### Task 12: Final integration test

Build everything end-to-end, verify no regressions.

**Step 1: Full compile check**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: PASS with no errors, minimal warnings

**Step 2: Run existing tests**

Run: `cargo test -p alephcore --lib -- --skip tools::markdown_skill`
Expected: All non-pre-existing-failure tests pass

**Step 3: Verify tool definitions count**

The system should now have the original tools + 4 agent_manage + 3 subagent_manage = total increase of 7.

**Step 4: Final commit (if any fixups needed)**

```bash
git commit -m "agent-switching: final integration fixes"
```
