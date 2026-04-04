# Multi-Agent Swarm Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enhance Aleph's multi-agent collaboration system — upgrade Spawn mode with named teammates, clean up code, and complete three swarm TODOs.

**Architecture:** Extend `SubagentTool` with optional `name`/`team_name` fields that auto-create lightweight teams and inject team tools. Reuse existing `TeamStore`, `MessageRouter`, and `CoordTaskStore` for communication. Complete `ContextInjector` wiring, interrupt mechanism, and LLM aggregator.

**Tech Stack:** Rust, tokio, serde_json, SQLite (rusqlite), tokio_util::CancellationToken, DashMap

**Spec:** `docs/superpowers/specs/2026-04-04-multi-agent-swarm-enhancement-design.md`

---

## File Structure

### New Files
| File | Responsibility |
|------|----------------|
| `src/agent_loop/subagent_runner.rs` | `run_subagent()` execution logic, AgentDef construction |
| `src/agent_loop/subagent_teammates.rs` | Teammate registration, team auto-creation, tool injection |

### Modified Files
| File | Changes |
|------|---------|
| `src/agent_loop/subagent_tool.rs` | Slim down to action dispatch only; add `SendMessage`/`ReadInbox` actions, `name`/`team_name` params |
| `src/agent_loop/mod.rs` | Add `pub mod subagent_runner; pub mod subagent_teammates;` |
| `src/agents/swarm/context_injector.rs` | Wire `inject_task_context()`; add `agent_tokens`/`pending_interrupts` for interrupt mechanism |
| `src/agents/swarm/aggregator.rs` | Integrate `AiProvider` into `IntelligenceLayer` for LLM summarization |
| `src/agents/swarm/coordinator.rs` | Wire `InboxContextProvider` into `ContextInjector` at init |

### Unchanged Files (reference only)
| File | Why Referenced |
|------|----------------|
| `src/agent_loop/background_tracker.rs` | Reuse `BackgroundAgentTracker` as-is |
| `src/teams/store.rs` | Reuse `TeamStore` trait + `SqliteTeamStore` |
| `src/teams/messages/router.rs` | Reuse `MessageRouter` for teammate messaging |
| `src/teams/messages/store.rs` | Reuse `SqliteMessageStore` |
| `src/teams/context.rs` | Reuse `TeamInboxContextProvider` |
| `src/agents/swarm/tasks/mod.rs` | Reuse `CoordTaskStore` trait + `CoordTask` types |

---

## Task 1: Extract `run_subagent()` into `subagent_runner.rs`

**Files:**
- Create: `src/agent_loop/subagent_runner.rs`
- Modify: `src/agent_loop/subagent_tool.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Create `subagent_runner.rs` with `run_subagent()` extracted from `subagent_tool.rs`**

Create `src/agent_loop/subagent_runner.rs`:

```rust
//! Sub-agent execution logic.
//!
//! Contains `run_subagent()` — the async function that builds and runs
//! a temporary `AgentLoop` for a sub-task. Extracted from `subagent_tool.rs`
//! for single-responsibility.

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::loop_core::{AgentLoop, LoopConfig, LoopRunResult, NoopCallback};
use super::prompt_builder::{PromptBuilder, PromptSection, Stability};
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::LoopToolRegistry;
use crate::agents::AgentDef;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Factory that builds a fresh LoopToolRegistry for the sub-agent.
///
/// The factory is responsible for providing the parent's tools minus
/// the "subagent" tool itself (to prevent infinite recursion).
pub type ToolRegistryFactory = Arc<dyn Fn() -> LoopToolRegistry + Send + Sync>;

/// Factory that builds a SafetyGuard for the sub-agent.
///
/// SafetyGuard is not Clone, so we use a factory to produce a fresh instance
/// each time a sub-agent is spawned.
pub type SafetyGuardFactory = Arc<dyn Fn() -> SafetyGuard + Send + Sync>;

/// Run a sub-agent to completion.
///
/// This is a module-level async function so it can be spawned in a
/// background tokio task (which requires `'static`).
pub async fn run_subagent(
    provider: Arc<dyn AiProvider>,
    agent_def: AgentDef,
    task: String,
    context_summary: Option<String>,
    model: Option<String>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    child_chain: super::chain_context::ChainContext,
    timeout_secs: u64,
) -> Result<LoopRunResult, String> {
    // Apply model override: explicit arg > agent_def.model_hint > default
    let resolved_model = model.or_else(|| agent_def.model_hint.clone());
    let bridge = if let Some(m) = resolved_model {
        AiProviderBridge::new(provider).with_model(m)
    } else {
        AiProviderBridge::new(provider)
    };

    // Build tool registry, then filter to agent's allowed tools
    let mut registry = (tool_registry_factory)();
    registry.retain(|name| agent_def.is_tool_allowed(name));

    // Build prompt for sub-agent via Section Registry.
    let mut prompt_builder = PromptBuilder::for_agent(&agent_def);

    // Inject parent context if provided
    if let Some(summary) = context_summary {
        prompt_builder.register(PromptSection {
            name: "parent_context".to_string(),
            stability: Stability::Dynamic,
            priority: 500,
            protected: false,
            content: format!("## Context from parent agent\n\n{}", summary),
        });
    }

    // Build loop config from agent definition
    let config = LoopConfig {
        max_iterations: agent_def.max_iterations.unwrap_or(25) as usize,
        token_budget: agent_def.token_budget.unwrap_or(100_000) as usize,
    };

    // Create and run the agent loop
    let mut agent_loop = AgentLoop::new(
        bridge,
        registry,
        prompt_builder,
        (safety_guard_factory)(),
        config,
        CancellationToken::new(),
    )
    .with_chain(child_chain);

    let mut callback = NoopCallback;
    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    let run_result =
        tokio::time::timeout(timeout_duration, agent_loop.run(&task, &mut callback)).await;

    match run_result {
        Err(_elapsed) => Err(format!("Sub-agent timed out after {}s", timeout_secs)),
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(format!("sub-agent failed: {}", e)),
    }
}
```

- [ ] **Step 2: Register module in `mod.rs`**

In `src/agent_loop/mod.rs`, add after line 24 (`pub mod subagent_tool;`):

```rust
pub mod subagent_runner;
```

- [ ] **Step 3: Update `subagent_tool.rs` to import from `subagent_runner`**

In `src/agent_loop/subagent_tool.rs`, replace the imports and remove the moved code:

Replace:
```rust
use super::loop_core::{AgentLoop, LoopConfig, NoopCallback};
use super::prompt_builder::{PromptBuilder, PromptSection, Stability};
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::{LoopTool, LoopToolRegistry, ToolResult};
```

With:
```rust
use super::subagent_runner::{run_subagent, SafetyGuardFactory, ToolRegistryFactory};
use super::tool::{LoopTool, LoopToolRegistry, ToolResult};
```

Remove from `subagent_tool.rs`:
- The `ToolRegistryFactory` type alias (lines 31)
- The `SafetyGuardFactory` type alias (lines 37)
- The entire `run_subagent()` function (lines 165-229)

The `SubagentTool` struct, `parse_args()`, and `impl LoopTool` remain.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors.

- [ ] **Step 5: Run existing tests**

Run: `cargo test -p alephcore subagent`
Expected: All existing subagent tests pass unchanged.

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/subagent_runner.rs src/agent_loop/subagent_tool.rs src/agent_loop/mod.rs
git commit -m "refactor(agent_loop): extract run_subagent into subagent_runner.rs"
```

---

## Task 2: Create `subagent_teammates.rs` — Teammate Registration

**Files:**
- Create: `src/agent_loop/subagent_teammates.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/agent_loop/subagent_teammates.rs` with test module first:

```rust
//! Teammate lifecycle management for named sub-agents.
//!
//! Handles auto-creation of lightweight teams, member registration,
//! and cleanup when teammates complete their work.

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::store::TeamStore;
use crate::teams::types::{NewTeam, NewTeamMember};

/// Manages teammate registration and lifecycle.
pub struct TeammateManager {
    team_store: Arc<dyn TeamStore>,
}

impl TeammateManager {
    pub fn new(team_store: Arc<dyn TeamStore>) -> Self {
        Self { team_store }
    }

    /// Ensure a team exists with the given name, creating it if necessary.
    /// The `parent_agent_id` becomes the team leader.
    /// Returns the team ID.
    pub async fn ensure_team(
        &self,
        team_name: &str,
        parent_agent_id: &str,
    ) -> Result<String> {
        // Check if team already exists by listing and filtering
        let teams = self.team_store.list_teams().await?;
        if let Some(existing) = teams.iter().find(|t| t.name == team_name) {
            return Ok(existing.id.clone());
        }

        // Create new team
        let team = self.team_store.create_team(NewTeam {
            name: team_name.to_string(),
            description: format!("Auto-created team for teammate collaboration"),
            leader_id: parent_agent_id.to_string(),
        }).await?;

        Ok(team.id)
    }

    /// Register a named agent as a member of a team.
    pub async fn register_teammate(
        &self,
        team_id: &str,
        agent_name: &str,
        role: &str,
    ) -> Result<()> {
        self.team_store.add_member(NewTeamMember {
            team_id: team_id.to_string(),
            agent_id: agent_name.to_string(),
            role: role.to_string(),
        }).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::store::SqliteTeamStore;

    async fn setup() -> TeammateManager {
        let store = SqliteTeamStore::new_in_memory().await;
        TeammateManager::new(Arc::new(store))
    }

    #[tokio::test]
    async fn ensure_team_creates_new_team() {
        let mgr = setup().await;
        let team_id = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        assert!(!team_id.is_empty());
    }

    #[tokio::test]
    async fn ensure_team_returns_existing() {
        let mgr = setup().await;
        let id1 = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        let id2 = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn register_teammate_succeeds() {
        let mgr = setup().await;
        let team_id = mgr.ensure_team("analysis", "parent-agent").await.unwrap();
        mgr.register_teammate(&team_id, "researcher", "worker").await.unwrap();
    }

    #[tokio::test]
    async fn team_name_without_agent_name_is_caller_responsibility() {
        // TeammateManager doesn't validate — caller (SubagentTool) validates
        let mgr = setup().await;
        let team_id = mgr.ensure_team("test", "leader").await.unwrap();
        // Empty agent_name is technically allowed at this layer
        mgr.register_teammate(&team_id, "", "worker").await.unwrap();
    }
}
```

- [ ] **Step 2: Register module in `mod.rs`**

In `src/agent_loop/mod.rs`, add:

```rust
pub mod subagent_teammates;
```

- [ ] **Step 3: Run tests to verify they fail or pass depending on `SqliteTeamStore::new_in_memory` availability**

Run: `cargo test -p alephcore subagent_teammates`
Expected: Tests compile and pass (using in-memory SQLite).

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/subagent_teammates.rs src/agent_loop/mod.rs
git commit -m "feat(agent_loop): add TeammateManager for auto team creation and registration"
```

---

## Task 3: Extend `SubagentAction` with `name`, `team_name`, `SendMessage`, `ReadInbox`

**Files:**
- Modify: `src/agent_loop/subagent_tool.rs`

- [ ] **Step 1: Write failing tests for new parse_args behavior**

Add these tests to the existing `#[cfg(test)] mod tests` in `subagent_tool.rs`:

```rust
#[test]
fn test_parse_args_with_name_and_team() {
    let action = parse_args(&json!({
        "task": "research auth module",
        "name": "researcher",
        "team_name": "analysis"
    }))
    .unwrap();
    match action {
        SubagentAction::Run(args) => {
            assert_eq!(args.name.as_deref(), Some("researcher"));
            assert_eq!(args.team_name.as_deref(), Some("analysis"));
        }
        _ => panic!("expected SubagentAction::Run"),
    }
}

#[test]
fn test_parse_args_team_without_name_is_error() {
    let result = parse_args(&json!({
        "task": "do work",
        "team_name": "analysis"
    }));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("team_name requires name"));
}

#[test]
fn test_parse_args_send_message() {
    let action = parse_args(&json!({
        "action": "send_message",
        "to": "researcher",
        "text": "Check the auth module"
    }))
    .unwrap();
    match action {
        SubagentAction::SendMessage { to, text } => {
            assert_eq!(to, "researcher");
            assert_eq!(text, "Check the auth module");
        }
        _ => panic!("expected SubagentAction::SendMessage"),
    }
}

#[test]
fn test_parse_args_read_inbox() {
    let action = parse_args(&json!({
        "action": "read_inbox",
        "team_name": "analysis"
    }))
    .unwrap();
    match action {
        SubagentAction::ReadInbox { team_name } => {
            assert_eq!(team_name, "analysis");
        }
        _ => panic!("expected SubagentAction::ReadInbox"),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore subagent -- test_parse_args_with_name`
Expected: FAIL — `SubagentAction::SendMessage` variant doesn't exist yet.

- [ ] **Step 3: Extend `SubagentAction` and `RunArgs`**

In `subagent_tool.rs`, replace the existing enums:

```rust
/// Parsed arguments for the subagent tool.
#[derive(Debug)]
enum SubagentAction {
    /// Run a new sub-agent task.
    Run(RunArgs),
    /// Check status of a background sub-agent.
    CheckStatus(String),
    /// Send a message to a named teammate.
    SendMessage { to: String, text: String },
    /// Read inbox messages.
    ReadInbox { team_name: String },
}

#[derive(Debug)]
struct RunArgs {
    task: String,
    agent_type: Option<String>,
    model: Option<String>,
    timeout_secs: u64,
    run_in_background: bool,
    context_summary: Option<String>,
    /// Optional name — makes the agent addressable.
    name: Option<String>,
    /// Optional team name — enables shared tasks and messages.
    team_name: Option<String>,
}
```

- [ ] **Step 4: Update `parse_args()` to handle new fields and actions**

Replace the entire `parse_args()` function:

```rust
/// Parse the input JSON into a SubagentAction.
fn parse_args(input: &Value) -> Result<SubagentAction, String> {
    // Check for explicit action field first
    let action = input.get("action").and_then(|v| v.as_str());

    match action {
        Some("send_message") => {
            let to = input
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or("send_message requires 'to' field")?
                .to_string();
            let text = input
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("send_message requires 'text' field")?
                .to_string();
            return Ok(SubagentAction::SendMessage { to, text });
        }
        Some("read_inbox") => {
            let team_name = input
                .get("team_name")
                .and_then(|v| v.as_str())
                .ok_or("read_inbox requires 'team_name' field")?
                .to_string();
            return Ok(SubagentAction::ReadInbox { team_name });
        }
        Some("check_status") => {
            let request_id = input
                .get("request_id")
                .and_then(|v| v.as_str())
                .ok_or("check_status requires 'request_id' field")?
                .to_string();
            return Ok(SubagentAction::CheckStatus(request_id));
        }
        Some("run") | None => { /* fall through to run logic */ }
        Some(unknown) => {
            return Err(format!(
                "Unknown action '{}'. Valid actions: run, check_status, send_message, read_inbox",
                unknown
            ));
        }
    }

    // Check for status-check mode (backward compat: request_id without task)
    let request_id = input
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // If request_id is provided without task, this is a status check
    if let Some(rid) = request_id {
        if task.is_none() || task.as_ref().map_or(false, |t| t.trim().is_empty()) {
            return Ok(SubagentAction::CheckStatus(rid));
        }
    }

    // Otherwise, this is a run action — task is required
    let task = task.ok_or_else(|| {
        "missing required field: task (or provide request_id to check background status)"
            .to_string()
    })?;

    if task.trim().is_empty() {
        return Err("task must not be empty".to_string());
    }

    let agent_type = input
        .get("agent_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let model = input
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let timeout_secs = input
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);

    let run_in_background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let context_summary = input
        .get("context_summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let team_name = input
        .get("team_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Validate: team_name requires name
    if team_name.is_some() && name.is_none() {
        return Err("team_name requires name — provide a name for the teammate".to_string());
    }

    Ok(SubagentAction::Run(RunArgs {
        task,
        agent_type,
        model,
        timeout_secs,
        run_in_background,
        context_summary,
        name,
        team_name,
    }))
}
```

- [ ] **Step 5: Update `schema()` to include new fields**

In the `schema()` method of `impl LoopTool for SubagentTool`, replace the JSON:

```rust
fn schema(&self) -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["run", "check_status", "send_message", "read_inbox"],
                "description": "Action to perform. Default: run.",
                "default": "run"
            },
            "task": {
                "type": "string",
                "description": "A clear description of the task for the sub-agent to complete."
            },
            "agent_type": {
                "type": "string",
                "description": "The type of agent to use (e.g., 'explore', 'coder', 'researcher', 'plan', 'verify'). Defaults to 'default'."
            },
            "model": {
                "type": "string",
                "description": "Model hint for the sub-agent (e.g., 'fast', 'deep')."
            },
            "timeout_secs": {
                "type": "integer",
                "description": "Maximum time in seconds for the sub-agent to run. Default: 120.",
                "default": 120
            },
            "run_in_background": {
                "type": "boolean",
                "description": "If true, run the sub-agent in the background and return immediately with a request_id.",
                "default": false
            },
            "context_summary": {
                "type": "string",
                "description": "A summary of the parent agent's context to pass to the sub-agent."
            },
            "request_id": {
                "type": "string",
                "description": "Check status of a background sub-agent. Provide request_id without task to retrieve the result."
            },
            "name": {
                "type": "string",
                "description": "Optional name for the sub-agent. Named agents are addressable and can communicate via team messages."
            },
            "team_name": {
                "type": "string",
                "description": "Team name for shared tasks and messages. Requires name."
            },
            "to": {
                "type": "string",
                "description": "Recipient agent name (for send_message action)."
            },
            "text": {
                "type": "string",
                "description": "Message text (for send_message action)."
            }
        },
        "required": []
    })
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore subagent`
Expected: All tests pass, including new ones.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/subagent_tool.rs
git commit -m "feat(subagent): extend SubagentAction with name, team_name, send_message, read_inbox"
```

---

## Task 4: Implement Teammate Spawn in `execute()`

**Files:**
- Modify: `src/agent_loop/subagent_tool.rs`

This task wires `TeammateManager` into `SubagentTool` and implements the `SendMessage`/`ReadInbox` action handlers in `execute()`.

- [ ] **Step 1: Add `TeammateManager` and messaging dependencies to `SubagentTool`**

Add new fields to the `SubagentTool` struct:

```rust
use super::subagent_teammates::TeammateManager;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::store::MessageStore;
use crate::teams::messages::types::MessageType;

pub struct SubagentTool {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    chain: super::chain_context::ChainContext,
    agent_registry: Arc<AgentRegistry>,
    background_tracker: Arc<BackgroundAgentTracker>,
    /// Optional teammate manager — None if team store not configured.
    teammate_manager: Option<Arc<TeammateManager>>,
    /// Optional message store for send_message/read_inbox actions.
    message_store: Option<Arc<dyn MessageStore>>,
    /// The calling agent's own ID (for send_message "from" field).
    parent_agent_id: String,
}
```

Update the `new()` constructor to accept new optional params:

```rust
impl SubagentTool {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
        chain: super::chain_context::ChainContext,
        agent_registry: Arc<AgentRegistry>,
        background_tracker: Arc<BackgroundAgentTracker>,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
            chain,
            agent_registry,
            background_tracker,
            teammate_manager: None,
            message_store: None,
            parent_agent_id: "primary".to_string(),
        }
    }

    /// Enable teammate capabilities (requires a TeamStore).
    pub fn with_teammate_manager(mut self, mgr: Arc<TeammateManager>) -> Self {
        self.teammate_manager = Some(mgr);
        self
    }

    /// Enable messaging for send_message/read_inbox actions.
    pub fn with_message_store(mut self, store: Arc<dyn MessageStore>) -> Self {
        self.message_store = Some(store);
        self
    }

    /// Set the parent agent's ID for message attribution.
    pub fn with_parent_agent_id(mut self, id: impl Into<String>) -> Self {
        self.parent_agent_id = id.into();
        self
    }
}
```

- [ ] **Step 2: Implement `SendMessage` and `ReadInbox` handlers in `execute()`**

In the `execute()` method, add handlers for the new actions before the existing `CheckStatus` handler:

```rust
async fn execute(&self, input: Value) -> ToolResult {
    let action = match parse_args(&input) {
        Ok(a) => a,
        Err(e) => {
            return ToolResult::Error {
                error: e,
                retryable: false,
            }
        }
    };

    match action {
        SubagentAction::SendMessage { to, text } => {
            let store = match &self.message_store {
                Some(s) => s,
                None => {
                    return ToolResult::Error {
                        error: "Messaging not available — no team store configured".into(),
                        retryable: false,
                    }
                }
            };

            // Find the team that both sender and recipient belong to
            // For now, use direct message via the store
            let msg = crate::teams::messages::types::NewMessage {
                team_id: String::new(), // will be resolved by teammate context
                from_agent: self.parent_agent_id.clone(),
                msg_type: MessageType::Message,
                subject: format!("Message to {}", to),
                content: text,
                recipients: vec![crate::teams::messages::types::Recipient {
                    agent_id: to.clone(),
                    role: crate::teams::messages::types::RecipientRole::To,
                }],
                reply_to: None,
                attachments: vec![],
            };

            match store.send_message(msg).await {
                Ok(sent) => ToolResult::Success {
                    output: json!({
                        "status": "sent",
                        "message_id": sent.id,
                        "to": to,
                    }),
                },
                Err(e) => ToolResult::Error {
                    error: format!("Failed to send message: {}", e),
                    retryable: true,
                },
            }
        }

        SubagentAction::ReadInbox { team_name } => {
            let store = match &self.message_store {
                Some(s) => s,
                None => {
                    return ToolResult::Error {
                        error: "Messaging not available — no team store configured".into(),
                        retryable: false,
                    }
                }
            };

            match store
                .read_inbox(&self.parent_agent_id, &team_name, None)
                .await
            {
                Ok(messages) => {
                    let summaries: Vec<Value> = messages
                        .iter()
                        .map(|m| {
                            json!({
                                "id": m.id,
                                "from": m.from_agent,
                                "subject": m.subject,
                                "content": m.content,
                                "type": m.msg_type.as_str(),
                            })
                        })
                        .collect();

                    ToolResult::Success {
                        output: json!({
                            "messages": summaries,
                            "count": summaries.len(),
                        }),
                    }
                }
                Err(e) => ToolResult::Error {
                    error: format!("Failed to read inbox: {}", e),
                    retryable: true,
                },
            }
        }

        SubagentAction::CheckStatus(request_id) => {
            // ... existing CheckStatus handler unchanged ...
        }

        SubagentAction::Run(args) => {
            // ... existing Run handler, with teammate registration added (next step) ...
        }
    }
}
```

- [ ] **Step 3: Add teammate registration to the `Run` handler**

Inside the `SubagentAction::Run(args)` arm, after resolving `agent_def` and before the background/foreground execution branch, add teammate registration:

```rust
SubagentAction::Run(args) => {
    tracing::info!(
        task = %args.task,
        agent_type = ?args.agent_type,
        name = ?args.name,
        team_name = ?args.team_name,
        "subagent: starting sub-task"
    );

    // ... existing agent_def resolution and chain depth check ...

    // Teammate registration (if name + team_name provided)
    let mut team_id = None;
    if let (Some(ref name), Some(ref tname)) = (&args.name, &args.team_name) {
        if let Some(ref mgr) = self.teammate_manager {
            match mgr.ensure_team(tname, &self.parent_agent_id).await {
                Ok(tid) => {
                    if let Err(e) = mgr.register_teammate(&tid, name, "worker").await {
                        tracing::warn!(error = %e, "Failed to register teammate");
                    }
                    team_id = Some(tid);
                }
                Err(e) => {
                    return ToolResult::Error {
                        error: format!("Failed to create team '{}': {}", tname, e),
                        retryable: false,
                    };
                }
            }
        }
    }

    // Named teammates always run in background
    let run_in_background = args.run_in_background || args.name.is_some();

    // ... rest of execution (foreground/background) uses run_in_background ...
}
```

- [ ] **Step 4: Update existing test helper `make_tool()` to set new fields to None**

The existing `make_tool()` already works since the new fields default to `None` via the `new()` constructor. Verify:

Run: `cargo test -p alephcore subagent`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/subagent_tool.rs
git commit -m "feat(subagent): wire TeammateManager and messaging into SubagentTool execute()"
```

---

## Task 5: Wire `inject_task_context()` in ContextInjector (Phase 3)

**Files:**
- Modify: `src/agents/swarm/context_injector.rs`

- [ ] **Step 1: Write the failing test**

Add to existing tests in `context_injector.rs`:

```rust
#[tokio::test]
async fn test_inject_task_context_returns_formatted_list() {
    use crate::agents::swarm::tasks::{
        CoordTask, CoordTaskFilter, CoordTaskId, CoordTaskStatus, CoordTaskUpdate,
        NewCoordTask, Priority,
    };

    /// In-memory mock for testing
    struct MockTaskStore {
        tasks: tokio::sync::Mutex<Vec<CoordTask>>,
    }

    impl MockTaskStore {
        fn new(tasks: Vec<CoordTask>) -> Self {
            Self {
                tasks: tokio::sync::Mutex::new(tasks),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::agents::swarm::tasks::CoordTaskStore for MockTaskStore {
        async fn create_task(&self, _: NewCoordTask) -> crate::error::Result<CoordTask> {
            unimplemented!()
        }
        async fn get_task(&self, _: &str) -> crate::error::Result<Option<CoordTask>> {
            unimplemented!()
        }
        async fn update_task(&self, _: &str, _: CoordTaskUpdate) -> crate::error::Result<CoordTask> {
            unimplemented!()
        }
        async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>> {
            let tasks = self.tasks.lock().await;
            Ok(tasks
                .iter()
                .filter(|t| {
                    filter.owner.as_ref().map_or(true, |o| {
                        t.owner.as_ref() == Some(o)
                    })
                })
                .cloned()
                .collect())
        }
        async fn get_dependencies(&self, _: &str) -> crate::error::Result<Vec<CoordTaskId>> {
            Ok(vec![])
        }
        async fn get_dependents(&self, _: &str) -> crate::error::Result<Vec<CoordTaskId>> {
            Ok(vec![])
        }
        async fn get_newly_unblocked(&self, _: &str) -> crate::error::Result<Vec<CoordTask>> {
            Ok(vec![])
        }
    }

    let bus = Arc::new(AgentMessageBus::new());
    let store = Arc::new(MockTaskStore::new(vec![
        CoordTask {
            id: "task-1".into(),
            team_id: None,
            subject: "Research auth module".into(),
            description: "Deep dive into auth".into(),
            status: CoordTaskStatus::InProgress,
            owner: Some("agent-1".into()),
            priority: Priority::High,
            result: None,
            metadata: serde_json::json!({}),
            dependencies: vec![],
            created_at: 0,
            started_at: Some(0),
            completed_at: None,
        },
    ]));

    let injector = ContextInjector::new(bus).with_task_store(store);
    let ctx = injector.inject_task_context("agent-1").await;

    assert!(ctx.is_some());
    let text = ctx.unwrap();
    assert!(text.contains("Your Tasks"));
    assert!(text.contains("Research auth module"));
    assert!(text.contains("InProgress"));
    assert!(text.contains("High"));
}

#[tokio::test]
async fn test_inject_task_context_empty_returns_none() {
    // Same MockTaskStore but with no tasks
    // (use the same mock from above with empty vec)
    let bus = Arc::new(AgentMessageBus::new());
    let injector = ContextInjector::new(bus);
    let ctx = injector.inject_task_context("agent-1").await;
    assert!(ctx.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore test_inject_task_context`
Expected: FAIL — `inject_task_context` currently returns `None`.

- [ ] **Step 3: Implement `inject_task_context()`**

In `context_injector.rs`, replace the `inject_task_context()` method (around line 203):

```rust
/// Inject task coordination context for an agent.
pub async fn inject_task_context(&self, agent_id: &str) -> Option<String> {
    use crate::agents::swarm::tasks::CoordTaskFilter;

    let store = self.task_store.as_ref()?;

    let filter = CoordTaskFilter {
        owner: Some(agent_id.to_string()),
        ..Default::default()
    };

    let tasks = store.list_tasks(filter).await.ok()?;

    if tasks.is_empty() {
        return None;
    }

    let mut ctx = String::from("## Your Tasks\n");
    for task in &tasks {
        ctx.push_str(&format!(
            "- [{}] {} (priority: {:?})\n",
            task.status.as_str(),
            task.subject,
            task.priority
        ));
        if let Some(ref deps) = Some(&task.dependencies) {
            if !deps.is_empty() {
                ctx.push_str(&format!("  blocked by: {}\n", deps.join(", ")));
            }
        }
    }
    Some(ctx)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore test_inject_task_context`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agents/swarm/context_injector.rs
git commit -m "feat(swarm): implement inject_task_context with CoordTaskStore integration"
```

---

## Task 6: Wire `InboxContextProvider` into `SwarmCoordinator`

**Files:**
- Modify: `src/agents/swarm/coordinator.rs`

- [ ] **Step 1: Add `with_inbox_provider` method to `SwarmCoordinator`**

Add after the existing `with_task_store()` method (around line 165):

```rust
/// Attach an inbox context provider for team message awareness.
///
/// Must be called before `start()` (i.e. before the injector Arc is shared).
pub fn with_inbox_provider(
    mut self,
    provider: Arc<dyn crate::teams::context::InboxContextProvider>,
) -> Result<Self> {
    let inner = Arc::try_unwrap(self.injector).map_err(|_| {
        crate::error::AlephError::config(
            "with_inbox_provider must be called before start() — injector Arc already shared",
        )
    })?;
    self.injector = Arc::new(inner.with_inbox_provider(provider));
    Ok(self)
}
```

- [ ] **Step 2: Add test**

Add to existing tests in `coordinator.rs`:

```rust
#[tokio::test]
async fn test_coordinator_with_task_store_and_inbox() {
    use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;

    let coordinator = SwarmCoordinator::new().await.unwrap();

    let task_store = Arc::new(
        SqliteCoordTaskStore::new_in_memory().await.unwrap(),
    );

    let coordinator = coordinator.with_task_store(task_store).unwrap();

    // Verify coordinator still works after wiring
    let stats = coordinator.statistics().await;
    assert_eq!(stats.context_window_size, 0);
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore test_coordinator_with`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/agents/swarm/coordinator.rs
git commit -m "feat(swarm): add with_inbox_provider to SwarmCoordinator"
```

---

## Task 7: Implement Critical Event Interrupt Mechanism (Phase 4)

**Files:**
- Modify: `src/agents/swarm/context_injector.rs`

- [ ] **Step 1: Write the failing test**

Add to tests in `context_injector.rs`:

```rust
#[tokio::test]
async fn test_interrupt_stores_pending_feedback() {
    let bus = Arc::new(AgentMessageBus::new());
    let injector = ContextInjector::new(bus);

    // Register an agent token
    let token = CancellationToken::new();
    injector.register_agent_token("agent-1", token.clone());

    // Handle critical event
    let event = CriticalEvent::ErrorDetected {
        agent_id: "agent-2".into(),
        error_message: "Database connection lost".into(),
        timestamp: 0,
    };

    let feedback = injector
        .handle_critical_event(&event, "agent-1")
        .await
        .unwrap();

    assert!(feedback.contains("CRITICAL INTERRUPT"));
    assert!(feedback.contains("Database connection lost"));

    // Token should be cancelled
    assert!(token.is_cancelled());

    // Pending interrupt should be retrievable
    let pending = injector.take_pending_interrupt("agent-1");
    assert!(pending.is_some());
    assert!(pending.unwrap().contains("Database connection lost"));

    // Second take returns None
    assert!(injector.take_pending_interrupt("agent-1").is_none());
}

#[tokio::test]
async fn test_interrupt_no_token_still_formats() {
    let bus = Arc::new(AgentMessageBus::new());
    let injector = ContextInjector::new(bus);

    let event = CriticalEvent::GlobalFailure {
        error: "Out of memory".into(),
        timestamp: 0,
    };

    // No token registered — should still format but not cancel
    let feedback = injector
        .handle_critical_event(&event, "unknown-agent")
        .await
        .unwrap();

    assert!(feedback.contains("CRITICAL INTERRUPT"));
    assert!(feedback.contains("Out of memory"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore test_interrupt`
Expected: FAIL — `register_agent_token` and `take_pending_interrupt` don't exist.

- [ ] **Step 3: Add `DashMap` fields and methods to `ContextInjector`**

Add imports at the top of `context_injector.rs`:

```rust
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;
```

Add new fields to `ContextInjector`:

```rust
pub struct ContextInjector {
    bus: Arc<AgentMessageBus>,
    context_window: Arc<RwLock<ContextWindow>>,
    task_store: Option<Arc<dyn CoordTaskStore>>,
    inbox_provider: Option<Arc<dyn InboxContextProvider>>,
    /// agent_id → CancellationToken for interrupting current LLM call
    agent_tokens: DashMap<String, CancellationToken>,
    /// agent_id → pending interrupt feedback to inject
    pending_interrupts: DashMap<String, String>,
}
```

Update all constructors (`new`, `with_window_size`) to initialize the new fields:

```rust
agent_tokens: DashMap::new(),
pending_interrupts: DashMap::new(),
```

Add public methods:

```rust
/// Register a CancellationToken for an agent (called at sub-agent spawn).
pub fn register_agent_token(&self, agent_id: &str, token: CancellationToken) {
    self.agent_tokens.insert(agent_id.to_string(), token);
}

/// Remove an agent's token (called when sub-agent completes).
pub fn unregister_agent_token(&self, agent_id: &str) {
    self.agent_tokens.remove(agent_id);
}

/// Take (consume) a pending interrupt message for an agent.
/// Returns `None` if no pending interrupt exists.
pub fn take_pending_interrupt(&self, agent_id: &str) -> Option<String> {
    self.pending_interrupts.remove(agent_id).map(|(_, v)| v)
}
```

- [ ] **Step 4: Update `handle_critical_event()` to actually interrupt**

Replace the existing `handle_critical_event()`:

```rust
/// Handle critical event (Tier 1: Interrupt-Driven).
///
/// Cancels the target agent's in-flight LLM call and stores
/// the interrupt feedback for injection in the next Think phase.
pub async fn handle_critical_event(
    &self,
    event: &CriticalEvent,
    agent_id: &str,
) -> Result<String> {
    let feedback = format!("[CRITICAL INTERRUPT] {}", self.format_critical_event(event));

    info!("Critical event for agent {}: {}", agent_id, feedback);

    // Cancel the agent's current LLM call if token is registered
    if let Some(token) = self.agent_tokens.get(agent_id) {
        token.cancel();
        tracing::info!(
            agent_id = agent_id,
            "Cancelled agent's CancellationToken due to critical event"
        );
    }

    // Store pending interrupt for injection in next Think phase
    self.pending_interrupts
        .insert(agent_id.to_string(), feedback.clone());

    Ok(feedback)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore test_interrupt`
Expected: PASS.

- [ ] **Step 6: Run all context_injector tests**

Run: `cargo test -p alephcore context_injector`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/agents/swarm/context_injector.rs
git commit -m "feat(swarm): implement critical event interrupt with CancellationToken + pending feedback"
```

---

## Task 8: Integrate LLM into `IntelligenceLayer` (Phase 5)

**Files:**
- Modify: `src/agents/swarm/aggregator.rs`

- [ ] **Step 1: Write the failing test**

Add to existing tests in `aggregator.rs`:

```rust
#[tokio::test]
async fn test_intelligence_layer_with_provider() {
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use std::future::Future;
    use std::pin::Pin;

    struct MockProvider;

    impl AiProvider for MockProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(ProviderResponse::text_only(
                    "3 agents actively analyzing auth module, 2 file reads completed".to_string(),
                ))
            })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000"
        }
    }

    let layer = IntelligenceLayer::new(Duration::from_secs(5), 1)
        .with_provider(Arc::new(MockProvider));

    let events = vec![
        InfoEvent::FileAccessed {
            agent_id: "agent-1".into(),
            path: "/auth/login.rs".into(),
            operation: FileOperation::Read,
            timestamp: 1,
        },
        InfoEvent::ToolExecuted {
            agent_id: "agent-2".into(),
            tool_name: "grep".into(),
            timestamp: 2,
        },
    ];

    let summary = layer.summarize_swarm_behavior(&events).await;
    assert!(summary.is_some());
    let text = summary.unwrap();
    // With provider, should get LLM-generated summary
    assert!(text.contains("auth module") || text.contains("agent"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore test_intelligence_layer_with_provider`
Expected: FAIL — `with_provider` method doesn't exist.

- [ ] **Step 3: Add `AiProvider` to `IntelligenceLayer`**

Update the struct:

```rust
use crate::providers::AiProvider;

pub struct IntelligenceLayer {
    summary_interval: Duration,
    min_events: usize,
    /// Optional AI provider for LLM-powered summarization.
    provider: Option<Arc<dyn AiProvider>>,
    /// Model hint for cost control (prefer cheap models).
    model_hint: Option<String>,
}

impl IntelligenceLayer {
    pub fn new(summary_interval: Duration, min_events: usize) -> Self {
        Self {
            summary_interval,
            min_events,
            provider: None,
            model_hint: None,
        }
    }

    /// Enable LLM-powered summarization with the given provider.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set model hint for cost control (e.g., "haiku" for cheap summarization).
    pub fn with_model_hint(mut self, hint: impl Into<String>) -> Self {
        self.model_hint = Some(hint.into());
        self
    }
}
```

- [ ] **Step 4: Update `summarize_swarm_behavior()` to use LLM when available**

```rust
async fn summarize_swarm_behavior(&self, events: &[InfoEvent]) -> Option<String> {
    if events.is_empty() {
        return None;
    }

    // If provider is available, use LLM summarization
    if let Some(ref provider) = self.provider {
        let event_descriptions: Vec<String> = events
            .iter()
            .map(|e| match e {
                InfoEvent::FileAccessed {
                    agent_id, path, operation, ..
                } => format!("Agent {} {:?} file {}", agent_id, operation, path),
                InfoEvent::SymbolSearched {
                    agent_id, symbol, ..
                } => format!("Agent {} searched for symbol '{}'", agent_id, symbol),
                InfoEvent::ToolExecuted {
                    agent_id, tool_name, ..
                } => format!("Agent {} executed tool '{}'", agent_id, tool_name),
                InfoEvent::ActionStarted {
                    agent_id, action_type, target, ..
                } => format!(
                    "Agent {} started {} on {}",
                    agent_id,
                    action_type,
                    target.as_deref().unwrap_or("unknown")
                ),
                InfoEvent::InsightCaptured {
                    agent_id, insight, ..
                } => format!("Agent {} insight: {}", agent_id, insight),
            })
            .collect();

        let prompt = format!(
            "Summarize the following agent activity in 2-3 sentences. \
             Focus on what agents are doing and any patterns:\n\n{}",
            event_descriptions.join("\n")
        );

        use crate::providers::adapter::RequestPayload;

        let payload = RequestPayload {
            system_prompt: "You are a concise activity summarizer.",
            messages: &[crate::providers::adapter::Message {
                role: "user",
                content: &prompt,
            }],
            tools: &[],
            model_hint: self.model_hint.as_deref(),
            max_tokens: Some(150),
            temperature: Some(0.3),
        };

        match provider.process(payload).await {
            Ok(response) => return Some(response.text_content()),
            Err(e) => {
                warn!("LLM summarization failed, falling back to stats: {}", e);
                // Fall through to statistical summary
            }
        }
    }

    // Fallback: simple statistical summary
    let tool_count = events
        .iter()
        .filter(|e| matches!(e, InfoEvent::ToolExecuted { .. }))
        .count();

    let file_count = events
        .iter()
        .filter(|e| matches!(e, InfoEvent::FileAccessed { .. }))
        .count();

    let search_count = events
        .iter()
        .filter(|e| matches!(e, InfoEvent::SymbolSearched { .. }))
        .count();

    Some(format!(
        "Swarm activity: {} tool executions, {} file accesses, {} symbol searches in last {} seconds",
        tool_count,
        file_count,
        search_count,
        self.summary_interval.as_secs()
    ))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore aggregator`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/agents/swarm/aggregator.rs
git commit -m "feat(swarm): integrate AiProvider into IntelligenceLayer for LLM summarization"
```

---

## Task 9: Final Compilation Check and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Full build check**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 2: Run all tests**

Run: `cargo test -p alephcore`
Expected: All tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings.

- [ ] **Step 4: Verify file sizes**

Check that refactored files meet size targets:

Run: `wc -l src/agent_loop/subagent_tool.rs src/agent_loop/subagent_runner.rs src/agent_loop/subagent_teammates.rs`

Expected: Each file under 500 lines.

- [ ] **Step 5: Commit any cleanup**

```bash
git add -A
git commit -m "chore(agent_loop): cleanup unused imports and fix clippy warnings"
```

---

## Implementation Notes

### Dependency on `RequestPayload` shape (Task 8)

The `RequestPayload` struct in `src/providers/adapter.rs` may have a different signature than shown in Task 8. Before implementing, read the actual struct definition and adjust the LLM call accordingly. The key requirement is: send a simple text prompt and get text back.

### `CoordTaskStatus::as_str()` (Task 5)

The `CoordTaskStatus` enum needs an `as_str()` method for the task context injection. If it doesn't exist, implement `Display` or `as_str()` on the enum. Check the actual definition in `src/agents/swarm/tasks/mod.rs`.

### `SqliteTeamStore::new_in_memory()` (Task 2)

The tests assume `SqliteTeamStore::new_in_memory()` exists. If it doesn't, create an in-memory SQLite connection and call `SqliteTeamStore::new(conn)` followed by `store.migrate().await`. Check `src/teams/store.rs` for the actual constructor pattern.

### `InfoEvent::ToolExecuted` variant (Task 8)

The `InfoEvent` enum may not have a `ToolExecuted` variant with the exact fields shown. Check `src/agents/swarm/events.rs` for the actual variant names and fields, and adjust the pattern matching accordingly.
