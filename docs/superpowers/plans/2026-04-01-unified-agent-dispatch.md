# Unified Agent Dispatch Chain Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify three agent dispatch mechanisms into a single SubagentTool with role-based agent selection, context summary injection, and background execution.

**Architecture:** Enhance SubagentTool to query AgentRegistry for AgentDef (role, system prompt, tool whitelist), build a customized AgentLoop per invocation. Add BackgroundAgentTracker for async execution with event bus notification. Then delete 11 legacy files (TaskTool, SubAgentDispatcher, DelegateTool, Coordinator, etc.).

**Tech Stack:** Rust, tokio, serde_json, uuid

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/agents/types.rs` | Modify | Add `ContextMode`, `token_budget`, `model_hint`, `context_mode` to `AgentDef` |
| `src/agents/registry.rs` | Modify | Add `plan` and `verify` built-in agents, add user-defined agent loading |
| `src/agents/prompts/default.md` | Create | System prompt for default sub-agent |
| `src/agents/prompts/plan.md` | Create | System prompt for plan agent |
| `src/agents/prompts/verify.md` | Create | System prompt for verify agent |
| `src/agent_loop/tool.rs` | Modify | Add `retain()` method to LoopToolRegistry |
| `src/agent_loop/subagent_tool.rs` | Modify | Full rewrite: agent_type, model, background, context_summary |
| `src/agent_loop/background_tracker.rs` | Create | BackgroundAgentTracker struct |
| `src/agent_loop/mod.rs` | Modify | Add `pub mod background_tracker;` |
| `src/gateway/execution_engine/run_loop.rs` | Modify | Wire AgentRegistry + ProviderRegistry into SubagentTool |
| `src/agents/mod.rs` | Modify | Remove old re-exports, update module declarations |
| `src/agents/sub_agents/mod.rs` | Modify → Delete | Remove module declarations progressively |
| 11 legacy files under `agents/sub_agents/` + `agents/task_tool.rs` | Delete | Old dispatch paths |

---

### Task 1: Extend AgentDef with new fields

**Files:**
- Modify: `src/agents/types.rs`

- [ ] **Step 1: Write test for ContextMode and new AgentDef fields**

Add to the existing `#[cfg(test)] mod tests` in `src/agents/types.rs`:

```rust
#[test]
fn test_context_mode_default() {
    let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt");
    assert_eq!(agent.context_mode, ContextMode::Fresh);
    assert!(agent.token_budget.is_none());
    assert!(agent.model_hint.is_none());
}

#[test]
fn test_with_context_mode() {
    let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt")
        .with_context_mode(ContextMode::Summary);
    assert_eq!(agent.context_mode, ContextMode::Summary);
}

#[test]
fn test_with_token_budget() {
    let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt")
        .with_token_budget(50_000);
    assert_eq!(agent.token_budget, Some(50_000));
}

#[test]
fn test_with_model_hint() {
    let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt")
        .with_model_hint("fast");
    assert_eq!(agent.model_hint.as_deref(), Some("fast"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agents::types::tests -- --nocapture 2>&1 | head -30`
Expected: Compilation error — `ContextMode` not defined, fields missing.

- [ ] **Step 3: Implement ContextMode enum and AgentDef extensions**

In `src/agents/types.rs`, add the enum before `AgentDef`:

```rust
/// How a sub-agent receives context from its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextMode {
    /// No parent context — sub-agent starts fresh (default).
    Fresh,
    /// Receives a parent-provided summary in its system prompt.
    Summary,
}

impl Default for ContextMode {
    fn default() -> Self {
        Self::Fresh
    }
}
```

Add three fields to `AgentDef`:

```rust
pub struct AgentDef {
    // ... existing fields ...
    /// Token budget override for the agent loop.
    pub token_budget: Option<u32>,
    /// Suggested model (can be overridden at call time).
    pub model_hint: Option<String>,
    /// How context is passed from parent agent.
    pub context_mode: ContextMode,
}
```

Update `AgentDef::new()` to initialize new fields:

```rust
pub fn new(id: impl Into<String>, mode: AgentMode, system_prompt: impl Into<String>) -> Self {
    Self {
        id: id.into(),
        mode,
        system_prompt: system_prompt.into(),
        allowed_tools: vec!["*".into()],
        denied_tools: vec![],
        max_iterations: None,
        token_budget: None,
        model_hint: None,
        context_mode: ContextMode::default(),
    }
}
```

Add builder methods:

```rust
pub fn with_context_mode(mut self, mode: ContextMode) -> Self {
    self.context_mode = mode;
    self
}

pub fn with_token_budget(mut self, budget: u32) -> Self {
    self.token_budget = Some(budget);
    self
}

pub fn with_model_hint(mut self, hint: impl Into<String>) -> Self {
    self.model_hint = Some(hint.into());
    self
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agents::types::tests -- --nocapture`
Expected: All tests pass including new ones.

- [ ] **Step 5: Commit**

```bash
git add src/agents/types.rs
git commit -m "feat(agents): add ContextMode, token_budget, model_hint to AgentDef"
```

---

### Task 2: Add LoopToolRegistry::retain() method

**Files:**
- Modify: `src/agent_loop/tool.rs`

The `run_subagent()` function needs to filter tools by AgentDef's allowlist. `LoopToolRegistry` currently has no filtering method.

- [ ] **Step 1: Write test for retain**

Add to the `#[cfg(test)]` module in `src/agent_loop/tool.rs`:

```rust
#[tokio::test]
async fn test_retain_filters_tools() {
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(DummyTool("alpha")));
    registry.register(Box::new(DummyTool("beta")));
    registry.register(Box::new(DummyTool("gamma")));

    registry.retain(|name| name == "alpha" || name == "gamma");

    assert_eq!(registry.len(), 2);
    assert!(registry.get("alpha").is_some());
    assert!(registry.get("beta").is_none());
    assert!(registry.get("gamma").is_some());
}
```

(If no `DummyTool` exists in tests, create a minimal one implementing `LoopTool`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::tool::tests::test_retain -- --nocapture 2>&1 | head -20`
Expected: Compilation error — `retain` not defined.

- [ ] **Step 3: Implement retain**

Add to `impl LoopToolRegistry` in `src/agent_loop/tool.rs`:

```rust
/// Remove tools whose names do not satisfy the predicate.
pub fn retain(&mut self, f: impl Fn(&str) -> bool) {
    self.tools.retain(|name, _| f(name));
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_loop::tool::tests::test_retain -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/tool.rs
git commit -m "feat(agent_loop): add LoopToolRegistry::retain() for tool filtering"
```

---

### Task 3: Add default, plan, and verify built-in agents

**Files:**
- Create: `src/agents/prompts/default.md`
- Create: `src/agents/prompts/plan.md`
- Create: `src/agents/prompts/verify.md`
- Modify: `src/agents/registry.rs`

- [ ] **Step 1: Write test for new built-in agents**

Add to existing tests in `src/agents/registry.rs`:

```rust
#[test]
fn test_plan_agent_config() {
    let registry = AgentRegistry::with_builtins();
    let plan = registry.get("plan").unwrap();

    assert_eq!(plan.mode, AgentMode::SubAgent);
    assert!(plan.is_tool_allowed("glob"));
    assert!(plan.is_tool_allowed("grep"));
    assert!(plan.is_tool_allowed("read_file"));
    assert!(!plan.is_tool_allowed("write_file"));
    assert!(!plan.is_tool_allowed("edit_file"));
    assert_eq!(plan.context_mode, ContextMode::Summary);
}

#[test]
fn test_verify_agent_config() {
    let registry = AgentRegistry::with_builtins();
    let verify = registry.get("verify").unwrap();

    assert_eq!(verify.mode, AgentMode::SubAgent);
    assert!(verify.is_tool_allowed("glob"));
    assert!(verify.is_tool_allowed("bash"));
    assert!(!verify.is_tool_allowed("write_file"));
    assert_eq!(verify.context_mode, ContextMode::Summary);
}

#[test]
fn test_builtin_agents_count_updated() {
    let agents = builtin_agents();
    assert_eq!(agents.len(), 7); // main, default, explore, coder, researcher, plan, verify
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agents::registry::tests -- --nocapture 2>&1 | head -30`
Expected: FAIL — `plan` and `verify` agents not found, count is 4 not 6.

- [ ] **Step 3: Create default.md prompt**

Create `src/agents/prompts/default.md`:

```markdown
You are a focused sub-agent executing a specific task delegated by a parent agent.
Complete the task thoroughly and return a clear, concise result.
Do not ask clarifying questions — work with what you have.
If you cannot complete the task, explain exactly what blocked you.
```

- [ ] **Step 5: Create plan.md prompt**

Create `src/agents/prompts/plan.md`:

```markdown
You are a planning agent. Your job is to analyze a task and produce a clear, step-by-step implementation plan.

## Constraints

- You are READ-ONLY. You must NOT create, modify, or delete any files.
- Use glob, grep, and read_file to explore the codebase.
- Bash is allowed only for read operations: ls, git status, git log, git diff, find, cat, head, tail.

## Output Format

1. Analyze the requirements and current codebase
2. Identify affected files and dependencies
3. Output a numbered step-by-step plan with:
   - What to change in each file
   - Why the change is needed
   - Risk assessment for each step
4. List critical files for implementation
```

- [ ] **Step 6: Create verify.md prompt**

Create `src/agents/prompts/verify.md`:

```markdown
You are a verification agent. Your job is to try to break the implementation — not to confirm it works.

## Failure Modes to Avoid

1. Verification avoidance: only reading code without running checks
2. Being fooled by the first 80%: UI looks fine, tests pass, so you skip edge cases

## Mandatory Checks

For every verification task:
1. Run the build: does it compile without warnings?
2. Run the test suite: do all tests pass?
3. Run lints: cargo clippy clean?
4. Adversarial probes: try inputs that should fail, boundary conditions, empty/null cases

## Output Format

For each check, report:
- Command run
- Output observed
- PASS / FAIL

End with: `VERDICT: PASS` or `VERDICT: FAIL` with reasons.

## Constraints

- You must NOT modify source files. Only read, run tests, and run verification commands.
- Use bash freely for running builds, tests, and diagnostic commands.
```

- [ ] **Step 7: Add default, plan, and verify to builtin_agents()**

In `src/agents/registry.rs`, add to the `builtin_agents()` vec after the researcher entry:

```rust
// Default agent - general-purpose sub-agent
AgentDef::new(
    "default",
    AgentMode::SubAgent,
    include_str!("prompts/default.md"),
)
.with_context_mode(ContextMode::Summary),

// Plan agent - read-only planner
AgentDef::new(
    "plan",
    AgentMode::SubAgent,
    include_str!("prompts/plan.md"),
)
.with_allowed_tools(vec![
    "glob".into(),
    "grep".into(),
    "read_file".into(),
    "bash".into(),
])
.with_denied_tools(vec!["write_file".into(), "edit_file".into()])
.with_max_iterations(20)
.with_context_mode(ContextMode::Summary),

// Verify agent - adversarial verifier
AgentDef::new(
    "verify",
    AgentMode::SubAgent,
    include_str!("prompts/verify.md"),
)
.with_allowed_tools(vec![
    "glob".into(),
    "grep".into(),
    "read_file".into(),
    "bash".into(),
])
.with_denied_tools(vec!["write_file".into(), "edit_file".into()])
.with_max_iterations(25)
.with_context_mode(ContextMode::Summary),
```

Also add the import at the top of `registry.rs`:

```rust
use crate::agents::types::{AgentDef, AgentMode, ContextMode};
```

- [ ] **Step 8: Update existing built-in agents with ContextMode**

In `builtin_agents()`, update the existing `coder` agent to use `ContextMode::Summary`:

```rust
// Coder agent - file operations
AgentDef::new(
    "coder",
    AgentMode::SubAgent,
    include_str!("prompts/coder.md"),
)
.with_allowed_tools(vec![...]) // unchanged
.with_max_iterations(30)
.with_context_mode(ContextMode::Summary),
```

Leave `main`, `explore`, and `researcher` as `ContextMode::Fresh` (the default).

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::registry::tests -- --nocapture`
Expected: All tests pass including new ones. Count = 7.

- [ ] **Step 10: Commit**

```bash
git add src/agents/prompts/default.md src/agents/prompts/plan.md src/agents/prompts/verify.md src/agents/registry.rs
git commit -m "feat(agents): add default, plan, and verify built-in agent roles"
```

---

### Task 4: Create BackgroundAgentTracker

**Files:**
- Create: `src/agent_loop/background_tracker.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Write tests for BackgroundAgentTracker**

Create `src/agent_loop/background_tracker.rs` with tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_list() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());

        let running = tracker.list_running();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].0, "req-1");
    }

    #[test]
    fn mark_completed_moves_from_running() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());

        tracker.mark_completed("req-1", Ok("done".to_string()));

        assert!(tracker.list_running().is_empty());
        let result = tracker.take_result("req-1");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unwrap(), "done");
    }

    #[test]
    fn cancel_cancels_token() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        let token_clone = token.clone();
        tracker.register("req-1".to_string(), token, "test task".to_string());

        tracker.cancel("req-1");
        assert!(token_clone.is_cancelled());
    }

    #[test]
    fn take_result_returns_none_for_unknown() {
        let tracker = BackgroundAgentTracker::new();
        assert!(tracker.take_result("unknown").is_none());
    }

    #[test]
    fn cleanup_removes_old_entries() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("old", Ok("old result".to_string()));

        // Cleanup with 0 TTL should remove everything
        tracker.cleanup(std::time::Duration::ZERO);
        assert!(tracker.take_result("old").is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::background_tracker 2>&1 | head -20`
Expected: Compilation error — module and struct not defined.

- [ ] **Step 3: Implement BackgroundAgentTracker**

Write the implementation above the tests in `src/agent_loop/background_tracker.rs`:

```rust
//! BackgroundAgentTracker — tracks sub-agents running in background tokio tasks.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::sync_primitives::RwLock;
use tokio_util::sync::CancellationToken;

/// Tracks background sub-agent executions.
pub struct BackgroundAgentTracker {
    running: RwLock<HashMap<String, RunningAgent>>,
    completed: RwLock<HashMap<String, CompletedAgent>>,
}

struct RunningAgent {
    cancel_token: CancellationToken,
    task_description: String,
    started_at: Instant,
}

struct CompletedAgent {
    result: Result<String, String>,
    completed_at: Instant,
}

impl BackgroundAgentTracker {
    pub fn new() -> Self {
        Self {
            running: RwLock::new(HashMap::new()),
            completed: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new background agent.
    pub fn register(
        &self,
        request_id: String,
        cancel_token: CancellationToken,
        task_description: String,
    ) {
        let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
        running.insert(
            request_id,
            RunningAgent {
                cancel_token,
                task_description,
                started_at: Instant::now(),
            },
        );
    }

    /// Mark a background agent as completed and store its result.
    pub fn mark_completed(&self, request_id: &str, result: Result<String, String>) {
        {
            let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
            running.remove(request_id);
        }
        {
            let mut completed = self.completed.write().unwrap_or_else(|e| e.into_inner());
            completed.insert(
                request_id.to_string(),
                CompletedAgent {
                    result,
                    completed_at: Instant::now(),
                },
            );
        }
    }

    /// Cancel a running background agent.
    pub fn cancel(&self, request_id: &str) {
        let running = self.running.read().unwrap_or_else(|e| e.into_inner());
        if let Some(agent) = running.get(request_id) {
            agent.cancel_token.cancel();
        }
    }

    /// Take (consume) a completed result. Returns None if not found.
    pub fn take_result(&self, request_id: &str) -> Option<Result<String, String>> {
        let mut completed = self.completed.write().unwrap_or_else(|e| e.into_inner());
        completed.remove(request_id).map(|c| c.result)
    }

    /// List running agents as (request_id, task_description, elapsed_secs).
    pub fn list_running(&self) -> Vec<(String, String, u64)> {
        let running = self.running.read().unwrap_or_else(|e| e.into_inner());
        running
            .iter()
            .map(|(id, agent)| {
                (
                    id.clone(),
                    agent.task_description.clone(),
                    agent.started_at.elapsed().as_secs(),
                )
            })
            .collect()
    }

    /// Remove completed entries older than `ttl`.
    pub fn cleanup(&self, ttl: Duration) {
        let mut completed = self.completed.write().unwrap_or_else(|e| e.into_inner());
        completed.retain(|_, agent| agent.completed_at.elapsed() < ttl);
    }
}

impl Default for BackgroundAgentTracker {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Register the module in agent_loop/mod.rs**

Add `pub mod background_tracker;` to `src/agent_loop/mod.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agent_loop::background_tracker -- --nocapture`
Expected: All 5 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/background_tracker.rs src/agent_loop/mod.rs
git commit -m "feat(agent_loop): add BackgroundAgentTracker for async sub-agents"
```

---

### Task 5: Rewrite SubagentTool with role selection, context, and background support

**Files:**
- Modify: `src/agent_loop/subagent_tool.rs`

This is the core task. The SubagentTool gains: `agent_type`, `model`, `run_in_background`, `context_summary` parameters. It queries AgentRegistry for the AgentDef and constructs the AgentLoop accordingly.

- [ ] **Step 1: Write tests for new SubagentTool parameters**

Replace the existing test module in `src/agent_loop/subagent_tool.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    use crate::agents::{AgentDef, AgentMode, AgentRegistry};
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;

    struct MockAiProvider;

    impl AiProvider for MockAiProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("mock response".to_string())) })
        }
        fn name(&self) -> &str { "mock" }
        fn color(&self) -> &str { "#000000" }
    }

    fn make_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    fn make_tool() -> SubagentTool {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let factory: ToolRegistryFactory = Arc::new(|| LoopToolRegistry::new());
        let safety_factory: SafetyGuardFactory = Arc::new(|| SafetyGuard::default_guard());
        let chain = super::super::chain_context::ChainContext::new();
        let registry = make_registry();
        let tracker = Arc::new(super::super::background_tracker::BackgroundAgentTracker::new());
        SubagentTool::new(provider, factory, safety_factory, chain, registry, tracker)
    }

    #[test]
    fn test_parse_args_basic() {
        let (args, _) = parse_args(&json!({"task": "do something"})).unwrap();
        assert_eq!(args.task, "do something");
        assert_eq!(args.timeout_secs, 120);
        assert!(args.agent_type.is_none());
        assert!(args.model.is_none());
        assert!(!args.run_in_background);
        assert!(args.context_summary.is_none());
    }

    #[test]
    fn test_parse_args_full() {
        let (args, _) = parse_args(&json!({
            "task": "explore the codebase",
            "agent_type": "explore",
            "model": "fast",
            "timeout_secs": 60,
            "run_in_background": true,
            "context_summary": "We are working on auth module"
        })).unwrap();
        assert_eq!(args.task, "explore the codebase");
        assert_eq!(args.agent_type.as_deref(), Some("explore"));
        assert_eq!(args.model.as_deref(), Some("fast"));
        assert_eq!(args.timeout_secs, 60);
        assert!(args.run_in_background);
        assert_eq!(args.context_summary.as_deref(), Some("We are working on auth module"));
    }

    #[test]
    fn test_parse_args_empty_task() {
        let result = parse_args(&json!({"task": ""}));
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_includes_new_fields() {
        let tool = make_tool();
        let schema = tool.schema();
        assert!(schema["properties"]["agent_type"].is_object());
        assert!(schema["properties"]["model"].is_object());
        assert!(schema["properties"]["run_in_background"].is_object());
        assert!(schema["properties"]["context_summary"].is_object());
    }

    #[tokio::test]
    async fn test_execute_with_agent_type() {
        let tool = make_tool();
        let result = tool.execute(json!({
            "task": "explore files",
            "agent_type": "explore"
        })).await;

        match result {
            ToolResult::Success { output } => {
                assert!(output["result"].is_string());
            }
            ToolResult::Error { error, .. } => panic!("expected success: {}", error),
            _ => panic!("unexpected result"),
        }
    }

    #[tokio::test]
    async fn test_execute_unknown_agent_type() {
        let tool = make_tool();
        let result = tool.execute(json!({
            "task": "do something",
            "agent_type": "nonexistent_agent"
        })).await;

        match result {
            ToolResult::Error { error, .. } => {
                assert!(error.contains("Unknown agent type"));
            }
            _ => panic!("expected error for unknown agent type"),
        }
    }

    #[tokio::test]
    async fn test_execute_background() {
        let tool = make_tool();
        let result = tool.execute(json!({
            "task": "long running task",
            "run_in_background": true
        })).await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["status"], "running_in_background");
                assert!(output["request_id"].is_string());
            }
            ToolResult::Error { error, .. } => panic!("expected success: {}", error),
            _ => panic!("unexpected result"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib agent_loop::subagent_tool::tests 2>&1 | head -30`
Expected: Compilation errors — new fields, new constructor signature.

- [ ] **Step 3: Implement SubagentArgs and parse_args**

Replace the `parse_args` function and add `SubagentArgs`:

```rust
/// Parsed arguments for the subagent tool.
struct SubagentArgs {
    task: String,
    agent_type: Option<String>,
    model: Option<String>,
    timeout_secs: u64,
    run_in_background: bool,
    context_summary: Option<String>,
}

fn parse_args(input: &Value) -> Result<(SubagentArgs, ()), String> {
    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "missing required field: task".to_string())?;

    if task.trim().is_empty() {
        return Err("task must not be empty".to_string());
    }

    let args = SubagentArgs {
        task,
        agent_type: input.get("agent_type").and_then(|v| v.as_str()).map(String::from),
        model: input.get("model").and_then(|v| v.as_str()).map(String::from),
        timeout_secs: input.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(120),
        run_in_background: input.get("run_in_background").and_then(|v| v.as_bool()).unwrap_or(false),
        context_summary: input.get("context_summary").and_then(|v| v.as_str()).map(String::from),
    };

    Ok((args, ()))
}
```

- [ ] **Step 4: Rewrite SubagentTool struct and constructor**

```rust
use crate::agents::{AgentDef, AgentRegistry};
use crate::sync_primitives::Arc;

pub struct SubagentTool {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    chain: super::chain_context::ChainContext,
    agent_registry: Arc<AgentRegistry>,
    background_tracker: Arc<super::background_tracker::BackgroundAgentTracker>,
}

impl SubagentTool {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
        chain: super::chain_context::ChainContext,
        agent_registry: Arc<AgentRegistry>,
        background_tracker: Arc<super::background_tracker::BackgroundAgentTracker>,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
            chain,
            agent_registry,
            background_tracker,
        }
    }
}
```

- [ ] **Step 5: Update schema() to include new fields**

```rust
fn schema(&self) -> Value {
    json!({
        "type": "object",
        "properties": {
            "task": {
                "type": "string",
                "description": "A clear description of the task for the sub-agent to complete."
            },
            "agent_type": {
                "type": "string",
                "description": "Agent role: explore (read-only search), plan (architecture planning), verify (adversarial testing), coder (implementation), researcher (web search). Omit for general-purpose."
            },
            "model": {
                "type": "string",
                "description": "Override model for the sub-agent (e.g. 'fast'). Omit to use default."
            },
            "timeout_secs": {
                "type": "integer",
                "description": "Maximum time in seconds. Default: 120.",
                "default": 120
            },
            "run_in_background": {
                "type": "boolean",
                "description": "Run in background. Returns immediately with request_id. Default: false.",
                "default": false
            },
            "context_summary": {
                "type": "string",
                "description": "Brief summary of relevant context: what you've learned, key decisions, files examined. Only include what's relevant to the task."
            }
        },
        "required": ["task"]
    })
}
```

- [ ] **Step 6: Implement the run_subagent helper function**

Add a module-level async function:

```rust
/// Build and run a sub-agent loop with the given configuration.
async fn run_subagent(
    provider: Arc<dyn AiProvider>,
    agent_def: &AgentDef,
    task: &str,
    context_summary: Option<&str>,
    tool_registry_factory: &ToolRegistryFactory,
    safety_guard_factory: &SafetyGuardFactory,
    child_chain: super::chain_context::ChainContext,
    timeout_secs: u64,
) -> Result<super::loop_core::RunResult, String> {
    let bridge = AiProviderBridge::new(provider);

    // Build prompt with agent's system prompt + optional context
    let mut prompt_builder = PromptBuilder::new()
        .with_identity(&agent_def.system_prompt)
        .with_default_behavior_sections();

    if let Some(summary) = context_summary {
        use super::prompt_builder::{PromptSection, Stability};
        prompt_builder.register(PromptSection {
            name: "parent_context".to_string(),
            stability: Stability::Dynamic,
            priority: 500,
            protected: false,
            content: format!("## Context from parent agent\n\n{}", summary),
        });
    }

    // Build tool registry filtered by AgentDef
    let mut registry = (tool_registry_factory)();
    registry.retain(|name| agent_def.is_tool_allowed(name));

    let config = LoopConfig {
        max_iterations: agent_def.max_iterations.unwrap_or(25) as usize,
        token_budget: agent_def.token_budget.unwrap_or(100_000) as usize,
    };

    let cancel = CancellationToken::new();
    let mut agent_loop = AgentLoop::new(
        bridge,
        registry,
        prompt_builder,
        (safety_guard_factory)(),
        config,
        cancel,
    )
    .with_chain(child_chain);

    let mut callback = NoopCallback;
    let timeout = std::time::Duration::from_secs(timeout_secs);

    tokio::time::timeout(timeout, agent_loop.run(task, &mut callback))
        .await
        .map_err(|_| format!("Sub-agent timed out after {}s", timeout_secs))?
        .map_err(|e| format!("Sub-agent failed: {}", e))
}
```

- [ ] **Step 7: Implement execute() with foreground/background split**

```rust
#[async_trait]
impl LoopTool for SubagentTool {
    fn name(&self) -> &str { "subagent" }

    fn description(&self) -> &str {
        "Delegate a task to a specialized sub-agent. Choose an agent_type for \
         focused roles (explore, plan, verify, coder, researcher) or omit for \
         general-purpose. Use run_in_background for long tasks."
    }

    fn schema(&self) -> Value { /* as defined in Step 5 */ }

    async fn execute(&self, input: Value) -> ToolResult {
        let (args, _) = match parse_args(&input) {
            Ok(a) => a,
            Err(e) => return ToolResult::Error { error: e, retryable: false },
        };

        // Resolve agent definition
        let agent_def = if let Some(ref agent_type) = args.agent_type {
            match self.agent_registry.get(agent_type) {
                Some(def) => def,
                None => {
                    let available = self.agent_registry.list_subagents()
                        .iter()
                        .map(|a| a.id.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return ToolResult::Error {
                        error: format!("Unknown agent type '{}'. Available: {}", agent_type, available),
                        retryable: false,
                    };
                }
            }
        } else {
            // Default: use the registered "default" agent
            self.agent_registry.get("default").expect("default agent must be registered")
        };

        // Check nesting depth
        let child_chain = match self.chain.child() {
            Some(c) => c,
            None => {
                return ToolResult::Error {
                    error: format!("Maximum subagent nesting depth ({}) exceeded", self.chain.max_depth),
                    retryable: false,
                };
            }
        };

        // Resolve provider (model override)
        // Model override: if args.model specified, try to resolve from provider_registry
        // Otherwise use agent_def.model_hint, otherwise default provider
        let provider = self.provider.clone();

        tracing::info!(
            task = %args.task,
            agent_type = ?args.agent_type,
            background = args.run_in_background,
            "subagent: dispatching"
        );

        if args.run_in_background {
            // Background path
            let request_id = uuid::Uuid::new_v4().to_string();
            let cancel_token = CancellationToken::new();
            self.background_tracker.register(
                request_id.clone(),
                cancel_token.clone(),
                args.task.clone(),
            );

            let provider = provider.clone();
            let agent_def = agent_def.clone();
            let task = args.task.clone();
            let context_summary = args.context_summary.clone();
            let factory = self.tool_registry_factory.clone();
            let safety = self.safety_guard_factory.clone();
            let tracker = self.background_tracker.clone();
            let timeout_secs = args.timeout_secs;

            tokio::spawn(async move {
                let result = run_subagent(
                    provider, &agent_def, &task,
                    context_summary.as_deref(),
                    &factory, &safety, child_chain, timeout_secs,
                ).await;

                let summary = match &result {
                    Ok(r) => r.final_text.clone().unwrap_or_else(|| "(no output)".to_string()),
                    Err(e) => e.clone(),
                };

                tracker.mark_completed(
                    &request_id,
                    result.map(|r| r.final_text.unwrap_or_else(|| "(no output)".to_string())),
                );

                tracing::info!(request_id = %request_id, "subagent: background task completed");
            });

            ToolResult::Success {
                output: json!({
                    "status": "running_in_background",
                    "request_id": request_id,
                    "message": "Sub-agent started in background. You will be notified when it completes."
                }),
            }
        } else {
            // Foreground path
            match run_subagent(
                provider, &agent_def, &args.task,
                args.context_summary.as_deref(),
                &self.tool_registry_factory, &self.safety_guard_factory,
                child_chain, args.timeout_secs,
            ).await {
                Ok(result) => {
                    tracing::info!(
                        iterations = result.iterations,
                        tool_calls = result.tool_calls_made,
                        tokens = result.total_tokens,
                        "subagent: completed"
                    );
                    ToolResult::Success {
                        output: json!({
                            "result": result.final_text.unwrap_or_else(|| "(no output)".to_string()),
                            "iterations": result.iterations,
                            "tool_calls_made": result.tool_calls_made
                        }),
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "subagent: failed");
                    ToolResult::Error { error: e, retryable: false }
                }
            }
        }
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agent_loop::subagent_tool::tests -- --nocapture`
Expected: All tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/agent_loop/subagent_tool.rs
git commit -m "feat(agent_loop): rewrite SubagentTool with role selection, context, and background support"
```

---

### Task 6: Wire AgentRegistry and BackgroundAgentTracker into Gateway

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Update SubagentTool construction in the gateway**

In `src/gateway/execution_engine/run_loop.rs`, find the block at ~line 286 where `SubagentTool::new` is called. Update it to pass `AgentRegistry` and `BackgroundAgentTracker`:

```rust
// Register subagent tool
{
    use crate::agent_loop::subagent_tool::SubagentTool;
    use crate::agent_loop::background_tracker::BackgroundAgentTracker;
    use crate::agents::AgentRegistry;

    let sub_provider = self.provider_registry.default_provider();
    let agent_registry = Arc::new(AgentRegistry::with_builtins());
    let background_tracker = Arc::new(BackgroundAgentTracker::new());

    tool_registry.register(Box::new(SubagentTool::new(
        sub_provider,
        sub_tool_factory.clone(),
        sub_safety_factory.clone(),
        run_chain.clone(),
        agent_registry,
        background_tracker,
    )));
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles successfully.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -20`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "feat(gateway): wire AgentRegistry and BackgroundAgentTracker into SubagentTool"
```

---

### Task 7: Remove legacy TaskTool

**Files:**
- Delete: `src/agents/task_tool.rs`
- Modify: `src/agents/mod.rs`
- Modify: `src/agents/integration_test.rs`

- [ ] **Step 1: Find all references to TaskTool**

Run: `cargo check -p alephcore 2>&1 | grep -i "task_tool\|TaskTool" | head -20`

This will confirm what breaks when we remove it.

- [ ] **Step 2: Remove TaskTool from mod.rs**

In `src/agents/mod.rs`:
- Remove `mod task_tool;` line
- Remove `pub use task_tool::{TaskTool, TaskToolError, TaskToolResult};` line

- [ ] **Step 3: Update integration_test.rs**

In `src/agents/integration_test.rs`, remove or rewrite tests that reference `TaskTool`. If the entire file is TaskTool-specific, delete it.

- [ ] **Step 4: Delete task_tool.rs**

```bash
rm src/agents/task_tool.rs
```

- [ ] **Step 5: Fix any remaining compilation errors**

Run: `cargo check -p alephcore 2>&1 | head -40`
Fix any references in other files (likely `builder/agent_init.rs` or similar). Replace usages with the new SubagentTool-based approach or remove dead code paths.

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -20`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "refactor(agents): remove legacy TaskTool"
```

---

### Task 8: Remove SubAgent delegation framework

**Files:**
- Delete: `src/agents/sub_agents/delegate_tool.rs`
- Delete: `src/agents/sub_agents/dispatcher.rs`
- Delete: `src/agents/sub_agents/coordinator.rs`
- Delete: `src/agents/sub_agents/result_collector.rs`
- Delete: `src/agents/sub_agents/result_merger.rs`
- Delete: `src/agents/sub_agents/mcp_agent.rs`
- Delete: `src/agents/sub_agents/skill_agent.rs`
- Delete: `src/agents/sub_agents/run.rs`
- Delete: `src/agents/sub_agents/traits.rs`
- Delete: `src/agents/sub_agents/registry.rs`
- Evaluate: `src/agents/sub_agents/persistence.rs`
- Modify: `src/agents/sub_agents/mod.rs`
- Modify: `src/agents/mod.rs`

This is a large deletion. Work incrementally — remove modules one at a time, fixing compilation after each group.

- [ ] **Step 1: Find all external references to sub_agents types**

```bash
cargo check -p alephcore 2>&1 | grep "sub_agents" | head -40
```

Also check the server binary:

```bash
grep -rn "sub_agents\|SubAgentDispatcher\|DelegateTool\|SubAgent\b\|McpSubAgent\|SkillSubAgent\|ResultCollector\|ExecutionCoordinator\|SubAgentRequest\|SubAgentResult" src/bin/ src/gateway/ src/executor/ --include="*.rs" | grep -v "test" | head -40
```

- [ ] **Step 2: Remove re-exports from agents/mod.rs**

In `src/agents/mod.rs`, remove:

```rust
// Remove these lines:
pub use sub_agents::{
    DelegateTool, McpSubAgent, SkillSubAgent, SubAgent, SubAgentCapability, SubAgentDispatcher,
    SubAgentRequest, SubAgentResult, SubAgentType,
};
```

Keep `pub mod sub_agents;` for now (persistence.rs may still be needed).

- [ ] **Step 3: Fix compilation errors in gateway/executor/server**

For each file that referenced the removed types:
- If it's DelegateTool registration in `builder/agent_init.rs` — remove the registration code
- If it's SubAgentDispatcher creation — remove the creation code
- If it's type references in executor registry — remove the fields and registration

Run `cargo check -p alephcore` after each fix until clean.

- [ ] **Step 4: Delete the sub_agents source files**

```bash
rm src/agents/sub_agents/delegate_tool.rs
rm src/agents/sub_agents/dispatcher.rs
rm src/agents/sub_agents/coordinator.rs
rm src/agents/sub_agents/result_collector.rs
rm src/agents/sub_agents/result_merger.rs
rm src/agents/sub_agents/mcp_agent.rs
rm src/agents/sub_agents/skill_agent.rs
rm src/agents/sub_agents/run.rs
rm src/agents/sub_agents/traits.rs
rm src/agents/sub_agents/registry.rs
```

- [ ] **Step 5: Update sub_agents/mod.rs**

Reduce `src/agents/sub_agents/mod.rs` to only what remains:

```rust
//! Sub-Agent persistence (legacy run tracking).

mod persistence;

pub use persistence::SubAgentRunFact;
```

If `persistence.rs` has no external consumers, delete it too and remove the entire `sub_agents` module.

- [ ] **Step 6: Verify compilation and tests**

```bash
cargo check -p alephcore && cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -20
```

Expected: Clean compilation, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "refactor(agents): remove SubAgent delegation framework (11 files)"
```

---

### Task 9: Final verification

**Files:** None (verification only)

- [ ] **Step 1: Full build**

```bash
cargo build -p alephcore
```

Expected: Clean build, no warnings.

- [ ] **Step 2: Full test suite**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass.

- [ ] **Step 3: Clippy**

```bash
cargo clippy -p alephcore -- -D warnings
```

Expected: No warnings.

- [ ] **Step 4: Verify the new SubagentTool works end-to-end**

Run the server and test a subagent call:

```bash
# Build and start
cargo build --bin aleph-server
# In a separate terminal, send a test message that should trigger subagent use
```

- [ ] **Step 5: Verify code metrics**

Count lines removed vs added:

```bash
git diff --stat HEAD~8..HEAD
```

Expected: Net negative line count (more deleted than added).

- [ ] **Step 6: Commit any final fixes**

```bash
git add -u
git commit -m "chore: final cleanup after unified agent dispatch refactor"
```
