# Agent Switching Enhancement Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make agent switching truly functional — per-agent identity, tool enforcement, sub-agent management, and routing unification.

**Architecture:** Workspace-Centric approach. Agent identity/persona anchored in workspace files (SOUL.md, IDENTITY.md, etc.). Existing infrastructure (WorkspaceFiles::load, SoulLayer, WorkspaceFilesLayer) already implemented — this design focuses on wiring the gaps.

**Tech Stack:** Rust, Tokio, AlephTool trait, PromptLayer system, EventBus

**Reference:** Compared with OpenClaw's agent switching system. Aleph's building blocks are more complete; the gap is integration wiring.

---

## Context: What Already Exists

| Component | Location | Status |
|-----------|----------|--------|
| `WorkspaceFiles::load()` + 7 canonical file names | `thinker/workspace_files.rs` | Ready |
| `WorkspaceFilesLayer` (PromptLayer, priority 1550) | `thinker/layers/workspace_files.rs` | Ready, never receives data |
| `SoulLayer` (supports workspace SOUL.md priority) | `thinker/layers/soul.rs` | Ready, never receives workspace data |
| `ResolvedAgent` with `workspace_path` | `config/agent_resolver.rs` | Ready |
| `initialize_workspace()` | `config/agent_resolver.rs` | Only creates AGENTS.md |
| `AgentInstanceConfig.tool_whitelist/blacklist` | `gateway/agent_instance.rs` | Defined, never enforced |
| `AgentInstance.is_tool_allowed()` | `gateway/agent_instance.rs` | Exists, never called |
| `SubAgentDispatcher` | `agents/sub_agents/` | Exists, not exposed as tool |
| `AgentLifecycleEvent` | `gateway/agent_lifecycle.rs` | Only `Registered` variant |
| `SessionContextHandle` | `builtin_tools/agent_manage/` | Shared RwLock, race risk |

---

## Section 1: Prompt Wiring

### Problem
`ExecutionEngine::run_agent_loop()` assembles prompts without loading workspace files. `WorkspaceFilesLayer` and `SoulLayer` are ready but receive no data.

### Solution
In `engine.rs`, after workspace resolution and before prompt assembly:

1. Derive `workspace_path` from `ActiveWorkspace`
2. Call `WorkspaceFiles::load(&ws_path, &config)` to load all 7 canonical files
3. Pass result to `LayerInput::with_workspace(workspace_files)`

### Effect Chain
- `SoulLayer` (priority 50): detects `input.workspace_file("SOUL.md")` → uses agent's SOUL.md over global soul.md
- `WorkspaceFilesLayer` (priority 1550): injects IDENTITY.md, TOOLS.md, MEMORY.md, HEARTBEAT.md, BOOTSTRAP.md
- No changes to Layer code — only wiring in engine

### Files to Modify
- `core/src/gateway/execution_engine/engine.rs` — add workspace files loading
- `core/src/gateway/workspace.rs` — ensure `ActiveWorkspace` exposes `workspace_path()`

---

## Section 2: Tool Enforcement

### Problem
`AgentInstanceConfig` has `tool_whitelist`/`tool_blacklist` and `AgentInstance.is_tool_allowed()` exists, but no enforcement point in the execution chain.

### Solution
New shared handle pattern (like SessionContextHandle):

```rust
pub struct ToolPolicy {
    pub whitelist: Vec<String>,  // empty = allow all
    pub blacklist: Vec<String>,  // empty = deny none
}
pub type ToolPolicyHandle = Arc<RwLock<ToolPolicy>>;

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
```

### Wiring
1. `BuiltinToolConfig` — add `tool_policy: Option<ToolPolicyHandle>`
2. `BuiltinToolRegistry` — store handle, check in `execute_tool()` entry
3. `ToolRegistry` trait — add `tool_policy_handle()` method
4. `ExecutionEngine` — each run reads agent's whitelist/blacklist, writes to handle

### Enforcement Point
```rust
// registry.rs — execute_tool() entry, before capability check
if let Some(ref policy_handle) = self.tool_policy_handle {
    let policy = policy_handle.read().await;
    if !policy.is_allowed(tool_name) {
        return Err(anyhow!("Tool '{}' is not allowed for the current agent", tool_name));
    }
}
```

### Files to Modify
- `core/src/builtin_tools/agent_manage/mod.rs` — add ToolPolicy types
- `core/src/executor/builtin_registry/config.rs` — add field
- `core/src/executor/builtin_registry/registry.rs` — store + enforce
- `core/src/executor/single_step.rs` — trait method
- `core/src/gateway/execution_engine/engine.rs` — inject per-run

---

## Section 3: Workspace Template Expansion

### Problem
`initialize_workspace()` only creates AGENTS.md and `memory/` directory. New agents have empty workspaces — prompt injection gets no content.

### Solution
Extend `AgentCreateTool` (not `initialize_workspace()`) to generate richer templates using user-provided parameters.

### Template Files (only created if not exist)

**SOUL.md** — from `args.system_prompt` or default template:
```markdown
You are {name}, an AI assistant specialized in {description}.

## Tone
- Professional, friendly, concise

## Boundaries
- Focus on your area of expertise
- Suggest switching to another agent for out-of-scope requests
```

**IDENTITY.md** — from `args.name`:
```markdown
- Name: {name}
- Emoji: (robot)
- Theme: professional
```

**TOOLS.md** — empty template:
```markdown
# Tool Notes

Record your tool usage preferences and notes here.
```

### Files to Modify
- `core/src/builtin_tools/agent_manage/create.rs` — add template generation after `initialize_workspace()`

---

## Section 4: SessionContext Race Fix

### Problem
`SessionContextHandle` is `Arc<RwLock<SessionContext>>` shared across all concurrent runs. Run B can overwrite Run A's channel/peer_id before Run A's tool reads it.

### Solution
Snapshot-based injection. In `execute_tool()`, read a snapshot of SessionContext before calling the tool, inject channel/peer_id into the tool arguments.

### Implementation
```rust
// registry.rs — agent_manage tool dispatch
"agent_create" | "agent_switch" | "agent_list" | "agent_delete" => {
    let ctx_snapshot = if let Some(ref h) = self.session_context_handle {
        h.read().await.clone()
    } else {
        SessionContext::default()
    };
    let mut args_with_ctx = arguments.clone();
    if let Some(obj) = args_with_ctx.as_object_mut() {
        obj.insert("__channel".into(), Value::String(ctx_snapshot.channel));
        obj.insert("__peer_id".into(), Value::String(ctx_snapshot.peer_id));
    }
    // call tool with args_with_ctx
}
```

### Tool Side
4 agent_manage tools' Args structs add:
```rust
#[serde(default)]
pub __channel: String,
#[serde(default)]
pub __peer_id: String,
```

Tool `call()` uses `args.__channel` / `args.__peer_id` instead of reading from handle.

### Files to Modify
- `core/src/executor/builtin_registry/registry.rs` — snapshot injection
- `core/src/builtin_tools/agent_manage/{create,switch,list,delete}.rs` — Args + call() changes

---

## Section 5: Sub-Agent Builtin Tools

### Problem
`SubAgentDispatcher` exists but is not exposed as builtin tools. LLM cannot spawn/steer/kill sub-agents during agent loop.

### Solution
3 new builtin tools bridging to SubAgentDispatcher:

```
subagent_spawn(agent_id, task, model?)  → run_id
subagent_steer(run_id, message)         → ack
subagent_kill(run_id)                   → status
```

### Module Structure
```
core/src/builtin_tools/subagent_manage/
  mod.rs       — module root + re-exports
  spawn.rs     — SubagentSpawnTool
  steer.rs     — SubagentSteerTool
  kill.rs      — SubagentKillTool
```

### Wiring
- `BuiltinToolConfig` already has `sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>`
- `BuiltinToolRegistry` — add 3 tool fields + execute_tool dispatch
- `BUILTIN_TOOL_DEFINITIONS` — add 3 definitions
- Startup — verify sub_agent_dispatcher is wired

### Not Implementing (vs OpenClaw)
- Thread focus/unfocus — Aleph has no thread concept; SessionKey::Subagent handles routing
- ACP external harness — out of scope
- List active subagents — extend `agent_list` with `include_subagents` parameter instead

### Files to Create
- `core/src/builtin_tools/subagent_manage/{mod,spawn,steer,kill}.rs`

### Files to Modify
- `core/src/builtin_tools/mod.rs` — add module
- `core/src/executor/builtin_registry/{config,registry,definitions}.rs` — register
- `core/src/executor/single_step.rs` — if needed

---

## Section 6: Lifecycle Events + Routing Unification

### 6a: Lifecycle Events

Extend `AgentLifecycleEvent`:

```rust
pub enum AgentLifecycleEvent {
    Registered { agent_id, workspace, model },               // existing
    Switched { agent_id, channel, peer_id, previous_agent_id },  // new
    Deleted { agent_id, workspace_archived: bool },              // new
    SubagentSpawned { parent_agent_id, child_run_id, task },     // new
    SubagentCompleted { child_run_id, outcome },                 // new
}
```

**EventBus dependency:** Add `event_bus: Option<Arc<GatewayEventBus>>` to `BuiltinToolConfig`. Tools emit events after successful operations.

### 6b: Routing Unification

Establish and document three-level priority chain:

```
Priority (high → low):
1. WorkspaceManager.get_active_agent(channel, peer_id)
   → User's explicit agent_switch result (highest priority)

2. AgentRouter.resolve(channel, peer_id, group_id)
   → Config-layer binding routes

3. Default agent ("main")
   → Fallback
```

**Key behavior:** When `agent_switch` switches back to "main", CLEAR the WorkspaceManager active_agent record (don't set to "main"). This lets routing fall through to AgentRouter bindings, restoring config-defined defaults.

### Files to Modify
- `core/src/gateway/agent_lifecycle.rs` — extend enum
- `core/src/executor/builtin_registry/config.rs` — add event_bus field
- `core/src/builtin_tools/agent_manage/{switch,delete}.rs` — emit events
- `core/src/builtin_tools/subagent_manage/{spawn,kill}.rs` — emit events
- `core/src/gateway/inbound_router.rs` — verify/adjust routing priority
- `core/src/builtin_tools/agent_manage/switch.rs` — clear on switch-to-main
- `core/src/bin/aleph/commands/start/mod.rs` — wire event_bus into BuiltinToolConfig

---

## Implementation Priority

| Phase | Sections | Impact |
|-------|----------|--------|
| Phase 1 | §1 (Prompt) + §3 (Templates) | Agent switching becomes meaningful — different persona/identity |
| Phase 2 | §2 (Tool Enforcement) + §4 (Race Fix) | Safety and correctness |
| Phase 3 | §5 (Sub-Agent) + §6 (Lifecycle + Routing) | Feature completeness |

---

## Design Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Where to store identity | Workspace files (SOUL.md, IDENTITY.md) | Already designed, user-editable, fits Aleph's workspace-centric model |
| Tool enforcement location | `execute_tool()` entry in BuiltinToolRegistry | Single checkpoint, all tools pass through |
| SessionContext race fix | Snapshot injection into arguments | Minimal change, no trait signature changes |
| Sub-agent exposure | 3 separate builtin tools | Consistent with agent_manage pattern |
| Routing priority | Runtime override > Config binding > Default | User intent > config > fallback, matches OpenClaw |
| Template generation | In AgentCreateTool, not initialize_workspace() | Only create has user parameters; initialize_workspace stays generic |
