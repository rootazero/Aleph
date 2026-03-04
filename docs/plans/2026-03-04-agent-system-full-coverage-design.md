# Agent System Full Coverage Design (Phase 2)

## Goal

Complete the agent system evolution from config-only "三件套" (Phase 1) to runtime integration.
Phase 1 delivered: Agent Definition types, WorkspaceFileLoader, AgentDefinitionResolver, Config struct integration, AgentRouter::from_bindings, startup wiring.
Phase 2 delivers: runtime bridge, workspace file injection, SubAgent authorization, daily memory, memory isolation, agent lifecycle.

## Architecture: Inside-Out Strategy

```
Layer 0 (Phase 1 - DONE): Config parsing + types + resolver
Layer 1 (Phase 2A): ResolvedAgent → AgentInstance bridge
Layer 2 (Phase 2B): Workspace file injection into ExecutionEngine
Layer 3 (Phase 2C): SubAgent authorization enforcement
Layer 4 (Phase 2D): Per-agent memory isolation + daily memory append
Layer 5 (Phase 2E): Agent lifecycle events + hot reload
```

Each layer builds on the previous — no layer can work without its prerequisite.

---

## Gap E: ResolvedAgent → AgentInstance Bridge

### Problem

`AgentDefinitionResolver::resolve_all()` produces `Vec<ResolvedAgent>` at startup, but `register_agent_handlers` still uses the old `FullGatewayConfig::get_agent_instance_configs()` to create `AgentInstance` objects. The two systems are disconnected.

### Design

Add a conversion path from `ResolvedAgent` to `AgentInstanceConfig`:

```rust
// core/src/gateway/agent_instance.rs
impl AgentInstanceConfig {
    pub fn from_resolved(agent: &ResolvedAgent) -> Self {
        Self {
            agent_id: agent.id.clone(),
            workspace: agent.workspace_path.clone(),
            model: agent.model.clone(),
            fallback_models: vec![],
            max_loops: 10,
            system_prompt: agent.agents_md.clone(),
            tool_whitelist: agent.skills.clone(),
            tool_blacklist: vec![],
        }
    }
}
```

Modify startup in `core/src/bin/aleph/commands/start/mod.rs`:

```rust
// In register_agent_handlers, when resolved_agents is available:
if !resolved_agents.is_empty() {
    // Use config-driven agents (new path)
    for agent in &resolved_agents {
        let config = AgentInstanceConfig::from_resolved(agent);
        let instance = AgentInstance::with_session_manager(config, session_manager.clone())?;
        agent_registry.register(instance).await;
    }
} else {
    // Fallback to legacy gateway config agents
    for agent_config in full_config.get_agent_instance_configs() {
        // ... existing code ...
    }
}
```

### Files

- Modify: `core/src/gateway/agent_instance.rs` — add `from_resolved()`
- Modify: `core/src/bin/aleph/commands/start/mod.rs` — conditional agent creation
- Modify: `core/src/config/agent_resolver.rs` — ensure `ResolvedAgent` is exported

---

## Gap A: Workspace File Injection

### Problem

WorkspaceFileLoader can read SOUL.md, AGENTS.md, MEMORY.md, and daily memory from workspace directories. But none of these files are injected into the agent's execution context.

### Design

#### Injection Points in ExecutionEngine

File: `core/src/gateway/execution_engine/engine.rs`

After workspace profile system_prompt injection (lines 513-518), add workspace file loading:

```
Current:
  resolve active_workspace → load bootstrap → inject profile.system_prompt → build ThinkerConfig

Extended:
  resolve active_workspace
  → load bootstrap
  → inject profile.system_prompt                    (existing)
  → load_workspace_context(agent_workspace_path)    (NEW)
  → inject AGENTS.md as custom_instructions         (NEW)
  → inject MEMORY.md + daily memory as context      (NEW)
  → override SoulManifest if workspace SOUL.md exists (NEW)
  → build ThinkerConfig
```

#### WorkspaceContext Struct

```rust
// core/src/gateway/execution_engine/workspace_context.rs
pub struct WorkspaceContext {
    pub soul: Option<SoulManifest>,
    pub agents_md: Option<String>,
    pub memory_md: Option<String>,
    pub recent_memory: Vec<DailyMemory>,
}
```

#### SoulManifest Priority Chain

```
Session override > Workspace SOUL.md > Profile default > Global default
```

When workspace SOUL.md exists, it replaces the profile's soul (not merge — replace). Session-level overrides still take precedence.

#### ExecutionEngine Changes

1. Add `workspace_loader: Arc<Mutex<WorkspaceFileLoader>>` field to ExecutionEngine
2. Initialize in `ExecutionEngine::new()`
3. In `run_agent_loop()`, after workspace profile injection:

```rust
// Load workspace files for this agent
let ws_context = {
    let mut loader = self.workspace_loader.lock().unwrap_or_else(|e| e.into_inner());
    WorkspaceContext {
        soul: loader.load_soul(&agent_workspace),
        agents_md: loader.load_agents_md(&agent_workspace),
        memory_md: loader.load_memory_md(&agent_workspace, bootstrap_max_chars),
        recent_memory: loader.load_recent_memory(&agent_workspace, 7),
    }
};

// Inject AGENTS.md as custom instructions
if let Some(ref agents_md) = ws_context.agents_md {
    extra_instructions.push(format!("## Agent Instructions\n\n{}", agents_md));
}

// Inject MEMORY.md as context
if let Some(ref memory_md) = ws_context.memory_md {
    extra_instructions.push(format!("## Agent Memory\n\n{}", memory_md));
}

// Inject recent daily memory
if !ws_context.recent_memory.is_empty() {
    let daily = ws_context.recent_memory.iter()
        .map(|m| format!("### {}\n{}", m.date, m.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    extra_instructions.push(format!("## Recent Activity\n\n{}", daily));
}

// Override soul if workspace has SOUL.md
if let Some(workspace_soul) = ws_context.soul {
    soul_manifest = Some(workspace_soul);
}
```

### Files

- Modify: `core/src/gateway/execution_engine/engine.rs` — workspace file injection
- Create: `core/src/gateway/execution_engine/workspace_context.rs` — WorkspaceContext struct
- Modify: `core/src/gateway/workspace_loader.rs` — add `Default` impl

---

## Gap C: Daily Memory Auto-Append

### Problem

`WorkspaceFileLoader::append_daily_memory()` exists but is never called. Session summaries are not persisted to workspace.

### Design

At the end of `ExecutionEngine::execute()`, after `RunCompleted` event emission:

```rust
// After loop completes
if let Some(summary) = extract_session_summary(&loop_result) {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let entry = format!(
        "\n## Session {}\n\n{}\n",
        chrono::Local::now().format("%H:%M"),
        summary
    );
    let mut loader = self.workspace_loader.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = loader.append_daily_memory(&agent_workspace, &date, &entry) {
        tracing::warn!(error = %e, "Failed to append daily memory");
    }
}
```

#### Session Summary Extraction

```rust
fn extract_session_summary(result: &LoopResult) -> Option<String> {
    // Use the last assistant message as a summary
    // Or if compression produced a summary, use that
    result.final_response.as_ref().map(|r| {
        if r.len() > 500 {
            format!("{}...", &r[..r.floor_char_boundary(500)])
        } else {
            r.clone()
        }
    })
}
```

### Files

- Modify: `core/src/gateway/execution_engine/engine.rs` — append after session end

---

## Gap B: SubAgent Authorization

### Problem

`SubagentPolicy.allow` is defined in config but never enforced. Any agent can delegate to any other agent.

### Design

#### SubagentAuthority Trait

```rust
// core/src/agents/sub_agents/authority.rs
pub trait SubagentAuthority: Send + Sync {
    fn can_delegate(&self, parent_agent_id: &str, child_agent_id: &str) -> bool;
}
```

#### ConfigDrivenAuthority

```rust
pub struct ConfigDrivenAuthority {
    policies: HashMap<String, SubagentPolicy>,
}

impl ConfigDrivenAuthority {
    pub fn from_resolved(agents: &[ResolvedAgent]) -> Self {
        let policies = agents.iter()
            .map(|a| (a.id.clone(), a.subagent_policy.clone()))
            .collect();
        Self { policies }
    }
}

impl SubagentAuthority for ConfigDrivenAuthority {
    fn can_delegate(&self, parent_id: &str, child_id: &str) -> bool {
        match self.policies.get(parent_id) {
            // No subagents config at all → allow all (backward compat)
            None => true,
            Some(policy) => {
                if policy.allow.is_empty() {
                    // Empty allow list with explicit config → deny all
                    false
                } else {
                    policy.allow.iter().any(|a| a == "*" || a == child_id)
                }
            }
        }
    }
}
```

#### Default Behavior (Backward Compatibility)

When `SubagentPolicy` is `None` on a `ResolvedAgent` (no `[agents.list.subagents]` section), the agent gets **no entry** in the policies HashMap. `can_delegate()` returns `true` for missing entries — fully backward compatible.

Only when `[agents.list.subagents]` is explicitly configured:
- `allow = ["*"]` → allow all
- `allow = ["reviewer"]` → allow only "reviewer"
- `allow = []` → deny all

#### Enforcement Point

In `SubAgentRegistry::register()` or the dispatcher:

```rust
if !authority.can_delegate(parent_agent_id, child_agent_id) {
    return Err(SubAgentError::Unauthorized {
        parent: parent_agent_id.to_string(),
        child: child_agent_id.to_string(),
    });
}
```

### Files

- Create: `core/src/agents/sub_agents/authority.rs`
- Modify: `core/src/agents/sub_agents/mod.rs` — add authority module
- Modify: `core/src/agents/sub_agents/registry.rs` or dispatcher — add check
- Modify: startup code — create authority from resolved agents

---

## Gap D: Per-Agent Memory Isolation

### Problem

LanceDB memory queries are not scoped to agent workspace. All agents share the same memory pool.

### Design

#### Existing Infrastructure

The memory system already has `WorkspaceFilter`:

```rust
pub enum WorkspaceFilter {
    Single(String),
    Multiple(Vec<String>),
    All,
}
```

And `memory/workspace.rs` already generates SQL WHERE clauses for LanceDB.

#### What's Needed

1. **Pass agent_id/workspace_id through RunRequest → ExecutionEngine → Memory operations**
2. **Set `WorkspaceFilter::Single(agent_id)` when querying memory for a specific agent**
3. **Tag stored facts with agent_id/workspace_id**

#### Implementation

In `ExecutionEngine::run_agent_loop()`:

```rust
// When creating memory context for this run
let memory_filter = WorkspaceFilter::Single(agent_id.clone());
// Pass to memory retrieval calls
```

The existing `ActiveWorkspace` already carries a `memory_filter` field. The key change is to set it based on agent_id when the agent has a dedicated workspace.

### Files

- Modify: `core/src/gateway/execution_engine/engine.rs` — pass workspace_id to memory context
- Possibly modify: `core/src/memory/store/` — if workspace_id not already propagated

---

## Gap F: Agent Lifecycle

### Problem

No lifecycle events, no cleanup, no hot reload support.

### Design

#### Lifecycle Events (via existing EventBus)

```rust
// core/src/gateway/agent_lifecycle.rs
pub enum AgentLifecycleEvent {
    Registered { agent_id: String, workspace: PathBuf },
    Started { agent_id: String },
    Stopped { agent_id: String },
    ConfigReloaded { added: Vec<String>, removed: Vec<String>, updated: Vec<String> },
}
```

#### Startup Flow

```
Config::load()
  → AgentDefinitionResolver::resolve_all()
  → For each ResolvedAgent:
      → AgentInstanceConfig::from_resolved()
      → AgentInstance::with_session_manager()
      → AgentRegistry::register()
      → EventBus::emit(AgentLifecycleEvent::Registered)
```

#### Shutdown Flow

```
Server::shutdown()
  → For each registered agent:
      → EventBus::emit(AgentLifecycleEvent::Stopped)
      → Cleanup temp files in workspace
      → Flush pending daily memory
```

#### Config Hot Reload (Future)

When config change is detected:
1. Re-run resolver
2. Diff old vs new resolved agents
3. Register new agents, update changed agents, unregister removed agents
4. Emit `ConfigReloaded` event

Hot reload is deferred — flagged as future enhancement, not in this phase.

### Files

- Create: `core/src/gateway/agent_lifecycle.rs` — event types
- Modify: `core/src/gateway/agent_instance.rs` — emit lifecycle events
- Modify: `core/src/bin/aleph/commands/start/mod.rs` — emit on startup

---

## Implementation Order

| Task | Gap | Description | Depends On |
|------|-----|-------------|------------|
| 1 | E | ResolvedAgent → AgentInstance bridge | Phase 1 |
| 2 | A | Workspace file injection into ExecutionEngine | Task 1 |
| 3 | C | Daily memory auto-append | Task 2 |
| 4 | B | SubAgent authorization enforcement | Task 1 |
| 5 | D | Per-agent memory isolation | Task 1 |
| 6 | F | Agent lifecycle events | Task 1 |

Tasks 2-6 all depend on Task 1 but are independent of each other.
Tasks 2 and 3 are closely related (both touch ExecutionEngine + workspace files).

## Key Files Modified

| File | Tasks |
|------|-------|
| `core/src/gateway/agent_instance.rs` | 1, 6 |
| `core/src/gateway/execution_engine/engine.rs` | 2, 3, 5 |
| `core/src/bin/aleph/commands/start/mod.rs` | 1, 4, 6 |
| `core/src/agents/sub_agents/authority.rs` (new) | 4 |
| `core/src/agents/sub_agents/registry.rs` | 4 |
| `core/src/gateway/agent_lifecycle.rs` (new) | 6 |
| `core/src/gateway/execution_engine/workspace_context.rs` (new) | 2 |
| `core/src/gateway/workspace_loader.rs` | 2 |

## Backward Compatibility

All changes are backward compatible:
- No `[agents]` section → legacy path (FullGatewayConfig agents) still works
- No `[agents.list.subagents]` → no authorization checks (allow all)
- No workspace SOUL.md → default soul manifest used
- No AGENTS.md/MEMORY.md → no extra instructions injected
- Memory queries without workspace_id → unfiltered (WorkspaceFilter::All)
