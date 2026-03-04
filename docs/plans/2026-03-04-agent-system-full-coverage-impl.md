# Agent System Full Coverage Implementation Plan (Phase 2)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete the agent system evolution by bridging config-driven agent definitions to runtime execution, injecting workspace files, enforcing SubAgent authorization, isolating per-agent memory, auto-appending daily logs, and adding lifecycle events.

**Architecture:** Inside-Out strategy — bridge ResolvedAgent to AgentInstance first (Layer 1), then inject workspace files into ExecutionEngine (Layer 2), then add authorization/memory/lifecycle (Layer 3-5). Each layer extends the previous.

**Tech Stack:** Rust, Tokio, serde, tracing, chrono, LanceDB (memory filtering)

**Design Doc:** `docs/plans/2026-03-04-agent-system-full-coverage-design.md`

---

### Task 1: ResolvedAgent → AgentInstance Bridge

**Files:**
- Modify: `core/src/gateway/agent_instance.rs`
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Context:** `AgentInstanceConfig` (line 17) has fields: agent_id, workspace, model, fallback_models, max_loops, system_prompt, tool_whitelist, tool_blacklist. `ResolvedAgent` (agent_resolver.rs:42) has: id, name, is_default, workspace_path, profile, soul, agents_md, memory_md, model, skills, subagent_policy. We need a conversion method.

**Step 1: Write the failing test**

In `core/src/gateway/agent_instance.rs`, add inside the existing `#[cfg(test)] mod tests` block (after existing tests around line 645):

```rust
#[test]
fn test_agent_instance_config_from_resolved() {
    use crate::config::agent_resolver::ResolvedAgent;
    use crate::config::types::agents_def::SubagentPolicy;
    use crate::config::types::profile::ProfileConfig;
    use std::path::PathBuf;

    let resolved = ResolvedAgent {
        id: "coding".to_string(),
        name: "Code Expert".to_string(),
        is_default: false,
        workspace_path: PathBuf::from("/tmp/test-workspace"),
        profile: ProfileConfig::default(),
        soul: None,
        agents_md: Some("Be a great coder.".to_string()),
        memory_md: None,
        model: "claude-opus-4-6".to_string(),
        skills: vec!["git_*".to_string(), "fs_*".to_string()],
        subagent_policy: SubagentPolicy::default(),
    };

    let config = AgentInstanceConfig::from_resolved(&resolved);
    assert_eq!(config.agent_id, "coding");
    assert_eq!(config.workspace, PathBuf::from("/tmp/test-workspace"));
    assert_eq!(config.model, "claude-opus-4-6");
    assert_eq!(config.system_prompt.as_deref(), Some("Be a great coder."));
    assert_eq!(config.tool_whitelist, vec!["git_*", "fs_*"]);
    assert!(config.tool_blacklist.is_empty());
    assert_eq!(config.max_loops, 10);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::agent_instance::tests::test_agent_instance_config_from_resolved 2>&1 | tail -5`
Expected: FAIL — `from_resolved` method doesn't exist

**Step 3: Add `from_resolved` method**

In `core/src/gateway/agent_instance.rs`, add to the `impl AgentInstanceConfig` block (or create one after the struct definition around line 35):

```rust
impl AgentInstanceConfig {
    /// Create from a resolved agent definition.
    ///
    /// Maps ResolvedAgent fields to AgentInstanceConfig:
    /// - system_prompt ← agents_md (workspace AGENTS.md content)
    /// - tool_whitelist ← skills
    /// - workspace ← workspace_path
    pub fn from_resolved(agent: &crate::config::agent_resolver::ResolvedAgent) -> Self {
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

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::agent_instance::tests::test_agent_instance_config_from_resolved 2>&1 | tail -5`
Expected: PASS

**Step 5: Modify startup to use from_resolved**

In `core/src/bin/aleph/commands/start/mod.rs`, find `register_agent_handlers` (around line 262). The current code at lines 343-360 creates agents from `full_config.get_agent_instance_configs()`. Replace the agent creation block with:

```rust
// Create agents: prefer config-driven ResolvedAgents, fallback to legacy
let agent_registry = Arc::new(AgentRegistry::new());
if !loaded_app_config.agents.list.is_empty() {
    // New path: use ResolvedAgents from AgentDefinitionResolver
    let mut resolver = alephcore::AgentDefinitionResolver::new();
    let resolved_agents = resolver.resolve_all(
        &loaded_app_config.agents,
        &loaded_app_config.profiles,
    );
    for agent in &resolved_agents {
        let config = alephcore::gateway::AgentInstanceConfig::from_resolved(agent);
        let agent_id = config.agent_id.clone();
        match alephcore::gateway::AgentInstance::with_session_manager(
            config,
            session_manager.clone(),
        ) {
            Ok(instance) => {
                agent_registry.register(instance).await;
                if !daemon {
                    println!("  Registered agent: {} (config-driven)", agent_id);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to create agent '{}': {}", agent_id, e);
            }
        }
    }
} else {
    // Legacy path: use FullGatewayConfig agents
    for agent_config in full_config.get_agent_instance_configs() {
        let agent_id = agent_config.agent_id.clone();
        match alephcore::gateway::AgentInstance::with_session_manager(
            agent_config,
            session_manager.clone(),
        ) {
            Ok(agent) => {
                agent_registry.register(agent).await;
                if !daemon {
                    println!("  Registered agent: {} (with SQLite persistence)", agent_id);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to create agent '{}': {}", agent_id, e);
            }
        }
    }
}
```

Note: The `resolved_agents` variable from the top-level startup code (lines 1007-1009) is not available inside `register_agent_handlers` because it's a separate function. We re-resolve here. This is cheap since workspace files are mtime-cached.

**Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: Compiles (warnings OK)

**Step 7: Commit**

```bash
git add core/src/gateway/agent_instance.rs core/src/bin/aleph/commands/start/mod.rs
git commit -m "gateway: bridge ResolvedAgent to AgentInstance with from_resolved()"
```

---

### Task 2: Workspace File Injection into ExecutionEngine

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs`
- Modify: `core/src/gateway/workspace_loader.rs` (add Default impl)

**Context:** ExecutionEngine (engine.rs:33) has fields: config, active_runs, provider_registry, tool_registry, tools, session_manager, workspace_manager, memory_backend. The `run_agent_loop` method (around line 400) resolves active_workspace, builds extra_instructions (lines 500-523), then constructs ThinkerConfig (lines 532-542). We inject workspace files into extra_instructions and optionally override soul.

**Step 1: Add Default impl to WorkspaceFileLoader**

In `core/src/gateway/workspace_loader.rs`, add after the `impl WorkspaceFileLoader` block:

```rust
impl Default for WorkspaceFileLoader {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Add workspace_loader field to ExecutionEngine**

In `core/src/gateway/execution_engine/engine.rs`, add a new field to the struct (after line 47):

```rust
    /// Workspace file loader for agent-scoped SOUL.md/AGENTS.md/MEMORY.md
    workspace_loader: std::sync::Mutex<crate::gateway::workspace_loader::WorkspaceFileLoader>,
```

In `ExecutionEngine::new()` (line 60), add to the struct initialization:

```rust
    workspace_loader: std::sync::Mutex::new(
        crate::gateway::workspace_loader::WorkspaceFileLoader::new(),
    ),
```

**Step 3: Add workspace file injection in run_agent_loop**

In `engine.rs`, after the existing workspace profile system_prompt injection (around line 518-523, the `if let Some(ref ws_prompt) = active_workspace.profile.system_prompt` block), add:

```rust
// Load workspace files from agent's workspace directory
let agent_workspace_dir = agent.config().workspace.clone();
{
    let mut loader = self.workspace_loader.lock().unwrap_or_else(|e| e.into_inner());

    // Inject AGENTS.md as agent-specific instructions
    if let Some(agents_md) = loader.load_agents_md(&agent_workspace_dir) {
        if !agents_md.is_empty() {
            extra_instructions.push(format!("## Agent Instructions\n\n{}", agents_md));
        }
    }

    // Inject MEMORY.md as persistent agent memory
    if let Some(memory_md) = loader.load_memory_md(&agent_workspace_dir, 20_000) {
        if !memory_md.is_empty() {
            extra_instructions.push(format!("## Agent Memory\n\n{}", memory_md));
        }
    }

    // Inject recent daily memory logs
    let recent = loader.load_recent_memory(&agent_workspace_dir, 7);
    if !recent.is_empty() {
        let daily = recent.iter()
            .map(|m| format!("### {}\n{}", m.date, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");
        extra_instructions.push(format!("## Recent Activity\n\n{}", daily));
    }

    // Override soul manifest if workspace has SOUL.md
    if let Some(workspace_soul) = loader.load_soul(&agent_workspace_dir) {
        soul = Some(workspace_soul);
    }
}
```

**Important:** The variable `soul` is used later in ThinkerConfig construction (line 532). Check that `soul` is declared as `let mut soul = ...` earlier in the function. If it's `let soul = ...`, change it to `let mut soul = ...`.

**Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: Compiles

**Step 5: Write a unit test for WorkspaceFileLoader Default**

In `core/src/gateway/workspace_loader.rs`, add to the tests module:

```rust
#[test]
fn test_default_creates_empty_loader() {
    let loader = WorkspaceFileLoader::default();
    assert!(loader.cache.is_empty());
}
```

**Step 6: Run tests**

Run: `cargo test -p alephcore --lib gateway::workspace_loader -- --nocapture 2>&1 | tail -10`
Expected: All 8 tests pass

**Step 7: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs core/src/gateway/workspace_loader.rs
git commit -m "engine: inject workspace AGENTS.md/MEMORY.md/SOUL.md into agent execution"
```

---

### Task 3: Daily Memory Auto-Append

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Context:** After the agent loop completes with `LoopResult::Completed { summary, .. }` (engine.rs line 630), we append the summary to the daily memory log file in the agent's workspace.

**Step 1: Add daily memory append after loop completion**

In `engine.rs`, find where `LoopResult::Completed` is handled (around line 630). After the `info!` log and before `Ok(summary)`, add:

```rust
LoopResult::Completed { summary, .. } => {
    info!(run_id = %run_id, "Agent loop completed successfully");

    // Append session summary to daily memory log
    {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let time = chrono::Local::now().format("%H:%M").to_string();
        let truncated_summary = if summary.len() > 500 {
            format!("{}...", &summary[..summary.floor_char_boundary(500)])
        } else {
            summary.clone()
        };
        let entry = format!("\n## Session {}\n\n{}\n", time, truncated_summary);
        let mut loader = self.workspace_loader.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = loader.append_daily_memory(&agent_workspace_dir, &date, &entry) {
            tracing::warn!(error = %e, "Failed to append daily memory");
        }
    }

    Ok(summary)
}
```

**Important:** The variable `agent_workspace_dir` must be in scope here. It was declared in Task 2's injection block. Make sure it's declared BEFORE the agent loop execution (not inside a limited scope block). If needed, move its declaration earlier:

```rust
let agent_workspace_dir = agent.config().workspace.clone();
```

**Step 2: Add chrono dependency check**

Run: `grep chrono core/Cargo.toml` — chrono should already be a dependency. If not, add it.

**Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: Compiles

**Step 4: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: auto-append session summary to daily memory log"
```

---

### Task 4: SubAgent Authorization

**Files:**
- Create: `core/src/agents/sub_agents/authority.rs`
- Modify: `core/src/agents/sub_agents/mod.rs`
- Modify: `core/src/agents/sub_agents/dispatcher.rs`

**Context:** `SubagentPolicy` has `allow: Vec<String>`. The dispatcher (dispatcher.rs) has `dispatch_sync()` and `dispatch_parallel_sync()` methods that spawn sub-agents. We add an authorization check before spawning.

**Step 1: Write the failing test**

Create `core/src/agents/sub_agents/authority.rs`:

```rust
//! SubAgent authorization enforcement.
//!
//! Checks whether a parent agent is allowed to delegate to a child agent
//! based on the SubagentPolicy defined in config.

use std::collections::HashMap;
use crate::config::types::agents_def::SubagentPolicy;

/// Trait for checking sub-agent delegation authorization.
pub trait SubagentAuthority: Send + Sync {
    /// Check if parent_agent_id is allowed to delegate to child_agent_id.
    fn can_delegate(&self, parent_agent_id: &str, child_agent_id: &str) -> bool;
}

/// Config-driven authorization using SubagentPolicy from agent definitions.
pub struct ConfigDrivenAuthority {
    /// Map of agent_id → SubagentPolicy (only agents with explicit config)
    policies: HashMap<String, SubagentPolicy>,
}

impl ConfigDrivenAuthority {
    /// Create from resolved agent definitions.
    ///
    /// Only agents with explicit `[agents.list.subagents]` config get entries.
    /// Agents without config are not in the map (→ allow all, backward compat).
    pub fn from_policies(policies: HashMap<String, SubagentPolicy>) -> Self {
        Self { policies }
    }

    /// Create from resolved agents, extracting only non-default policies.
    pub fn from_resolved(agents: &[crate::config::agent_resolver::ResolvedAgent]) -> Self {
        let policies = agents
            .iter()
            .filter(|a| !a.subagent_policy.allow.is_empty())
            .map(|a| (a.id.clone(), a.subagent_policy.clone()))
            .collect();
        Self { policies }
    }
}

impl SubagentAuthority for ConfigDrivenAuthority {
    fn can_delegate(&self, parent_id: &str, child_id: &str) -> bool {
        match self.policies.get(parent_id) {
            // No explicit policy → allow all (backward compat)
            None => true,
            Some(policy) => {
                policy.allow.iter().any(|a| a == "*" || a == child_id)
            }
        }
    }
}

/// Permissive authority that allows all delegation (default/fallback).
pub struct PermissiveAuthority;

impl SubagentAuthority for PermissiveAuthority {
    fn can_delegate(&self, _parent_id: &str, _child_id: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissive_allows_all() {
        let auth = PermissiveAuthority;
        assert!(auth.can_delegate("any", "any"));
    }

    #[test]
    fn test_config_no_policy_allows_all() {
        let auth = ConfigDrivenAuthority::from_policies(HashMap::new());
        assert!(auth.can_delegate("main", "coding"));
    }

    #[test]
    fn test_config_wildcard_allows_all() {
        let mut policies = HashMap::new();
        policies.insert("main".to_string(), SubagentPolicy {
            allow: vec!["*".to_string()],
        });
        let auth = ConfigDrivenAuthority::from_policies(policies);
        assert!(auth.can_delegate("main", "coding"));
        assert!(auth.can_delegate("main", "reviewer"));
    }

    #[test]
    fn test_config_specific_allows_listed() {
        let mut policies = HashMap::new();
        policies.insert("main".to_string(), SubagentPolicy {
            allow: vec!["coding".to_string(), "reviewer".to_string()],
        });
        let auth = ConfigDrivenAuthority::from_policies(policies);
        assert!(auth.can_delegate("main", "coding"));
        assert!(auth.can_delegate("main", "reviewer"));
        assert!(!auth.can_delegate("main", "hacker"));
    }

    #[test]
    fn test_config_empty_allow_denies_all() {
        // Agent without explicit subagents config → not in map → allow
        // But if we explicitly add empty, it means deny
        // Note: from_resolved filters out empty allow lists, so this only
        // happens when manually constructed
        let mut policies = HashMap::new();
        policies.insert("strict".to_string(), SubagentPolicy {
            allow: vec![],
        });
        let auth = ConfigDrivenAuthority::from_policies(policies);
        // Empty allow list → no entry matches → deny
        assert!(!auth.can_delegate("strict", "anything"));
        // Other agents not in map → allow
        assert!(auth.can_delegate("other", "anything"));
    }
}
```

**Step 2: Run test to verify it passes**

First, add the module to `core/src/agents/sub_agents/mod.rs`. Add this line with the other module declarations:

```rust
pub mod authority;
```

And add to the public exports:

```rust
pub use authority::{SubagentAuthority, ConfigDrivenAuthority, PermissiveAuthority};
```

Run: `cargo test -p alephcore --lib agents::sub_agents::authority -- --nocapture 2>&1 | tail -10`
Expected: PASS (5 tests)

**Step 3: Commit**

```bash
git add core/src/agents/sub_agents/authority.rs core/src/agents/sub_agents/mod.rs
git commit -m "agents: add SubagentAuthority trait with config-driven enforcement"
```

**Step 4: Integrate authority into dispatcher** (optional — can be deferred)

In `core/src/agents/sub_agents/dispatcher.rs`, add an `authority` field to `SubAgentDispatcher`:

```rust
/// Optional authorization checker
authority: Option<Arc<dyn SubagentAuthority>>,
```

In `dispatch_sync()` (around line 312), add before spawning:

```rust
// Check authorization
if let Some(ref auth) = self.authority {
    let parent_id = request.parent_agent_id.as_deref().unwrap_or("main");
    let child_id = request.target_agent_id.as_deref().unwrap_or(&self.default_agent);
    if !auth.can_delegate(parent_id, child_id) {
        return Err(ExecutionError::Internal(format!(
            "Agent '{}' is not authorized to delegate to '{}'",
            parent_id, child_id
        )));
    }
}
```

Note: The exact field names on `SubAgentRequest` for parent/child agent IDs depend on the struct definition. Read `core/src/agents/sub_agents/traits.rs` to find the correct field names. Adjust accordingly.

**Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: Compiles

**Step 6: Commit**

```bash
git add core/src/agents/sub_agents/dispatcher.rs
git commit -m "agents: enforce SubagentAuthority in dispatcher before delegation"
```

---

### Task 5: Per-Agent Memory Isolation

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Context:** `ActiveWorkspace` (workspace.rs:194) already has `memory_filter: WorkspaceFilter`. When created via `from_workspace_id()` or `from_manager()`, it sets `WorkspaceFilter::Single(workspace_id)`. The key is to use the agent's workspace_id (agent_id) when building the ActiveWorkspace, so memory queries are automatically scoped.

**Step 1: Modify workspace resolution to use agent_id**

In `engine.rs`, find the workspace resolution block (around line 416-448). Currently it resolves workspace from route metadata or user's active workspace. After this resolution, override the memory_filter with the agent's ID:

```rust
// After active_workspace resolution (around line 448):

// Override memory filter with agent-specific scoping
let active_workspace = {
    let mut ws = active_workspace;
    // If agent has a dedicated workspace, scope memory to agent_id
    let agent_id = agent.id();
    if agent_id != "main" {
        ws.memory_filter = crate::memory::workspace::WorkspaceFilter::Single(
            agent_id.to_string(),
        );
    }
    ws
};
```

This ensures that non-main agents get memory isolation by default, while "main" agent retains whatever workspace filter was already set.

**Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: Compiles

**Step 3: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: scope memory queries to agent workspace for non-main agents"
```

---

### Task 6: Agent Lifecycle Events

**Files:**
- Create: `core/src/gateway/agent_lifecycle.rs`
- Modify: `core/src/gateway/mod.rs`
- Modify: `core/src/gateway/agent_instance.rs`

**Context:** `GatewayEventBus` (event_bus.rs:178) has `publish_json()` which serializes any `Serialize` type to JSON and broadcasts it. `TopicEvent::new(topic, data)` creates topic-aware events.

**Step 1: Create lifecycle event types**

Create `core/src/gateway/agent_lifecycle.rs`:

```rust
//! Agent lifecycle events.
//!
//! Emitted via GatewayEventBus when agents are registered, started, or stopped.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Agent lifecycle event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLifecycleEvent {
    /// Agent was registered in the registry
    Registered {
        agent_id: String,
        workspace: PathBuf,
        model: String,
    },
    /// Agent execution started
    Started {
        agent_id: String,
        run_id: String,
    },
    /// Agent execution completed
    Completed {
        agent_id: String,
        run_id: String,
        success: bool,
    },
    /// Agent was unregistered (e.g., during config reload)
    Unregistered {
        agent_id: String,
    },
}

impl AgentLifecycleEvent {
    /// Get the event topic string for EventBus routing.
    pub fn topic(&self) -> &'static str {
        match self {
            Self::Registered { .. } => "agent.lifecycle.registered",
            Self::Started { .. } => "agent.lifecycle.started",
            Self::Completed { .. } => "agent.lifecycle.completed",
            Self::Unregistered { .. } => "agent.lifecycle.unregistered",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_event_serialization() {
        let event = AgentLifecycleEvent::Registered {
            agent_id: "coding".to_string(),
            workspace: PathBuf::from("/tmp/ws"),
            model: "claude-opus-4-6".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"registered\""));
        assert!(json.contains("\"agent_id\":\"coding\""));
    }

    #[test]
    fn test_lifecycle_topics() {
        let reg = AgentLifecycleEvent::Registered {
            agent_id: "main".to_string(),
            workspace: PathBuf::from("/tmp"),
            model: "test".to_string(),
        };
        assert_eq!(reg.topic(), "agent.lifecycle.registered");

        let started = AgentLifecycleEvent::Started {
            agent_id: "main".to_string(),
            run_id: "run-1".to_string(),
        };
        assert_eq!(started.topic(), "agent.lifecycle.started");
    }
}
```

**Step 2: Register the module**

In `core/src/gateway/mod.rs`, add:

```rust
pub mod agent_lifecycle;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib gateway::agent_lifecycle -- --nocapture 2>&1 | tail -10`
Expected: PASS (2 tests)

**Step 4: Emit lifecycle events at startup**

In `core/src/bin/aleph/commands/start/mod.rs`, after agents are registered (in the new config-driven path from Task 1), add:

```rust
// After agent_registry.register(instance).await:
let lifecycle_event = alephcore::gateway::agent_lifecycle::AgentLifecycleEvent::Registered {
    agent_id: agent_id.clone(),
    workspace: config.workspace.clone(),
    model: config.model.clone(),
};
let topic_event = alephcore::gateway::event_bus::TopicEvent::new(
    lifecycle_event.topic(),
    serde_json::to_value(&lifecycle_event).unwrap_or_default(),
);
let _ = event_bus.publish_json(&topic_event);
```

**Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: Compiles

**Step 6: Commit**

```bash
git add core/src/gateway/agent_lifecycle.rs core/src/gateway/mod.rs core/src/bin/aleph/commands/start/mod.rs
git commit -m "gateway: add agent lifecycle events (registered/started/completed)"
```

---

## Post-Implementation

### Run all tests

```bash
cargo test -p alephcore --lib 2>&1 | tail -15
```

Expected: All existing tests pass + new tests pass. Pre-existing failures in `tools::markdown_skill::loader::tests` are expected.

### Verify backward compatibility

Create a minimal config without `[agents]` section and verify it still works:

```bash
cargo check -p alephcore 2>&1 | tail -5
```
