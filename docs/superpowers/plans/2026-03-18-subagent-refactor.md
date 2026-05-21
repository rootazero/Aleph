# Subagent System Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the fake subagent system (spawn/steer/kill + McpSubAgent/SkillSubAgent) with a single `subagent` LoopTool that runs a real AgentLoop.

**Architecture:** SubagentTool is a LoopTool registered directly into the LoopToolRegistry during `run_agent_loop`. It holds the ingredients to build a sub-agent's AgentLoop (provider, tool list, safety guard). When called, it constructs a temporary AgentLoop, runs the task, and returns the result. No dispatcher, no registry, no lifecycle management.

**Tech Stack:** Rust, existing agent_loop module, existing LoopTool trait

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/agent_loop/subagent_tool.rs` | SubagentTool implementing LoopTool — creates and runs a temporary AgentLoop |
| Modify | `src/agent_loop/mod.rs` | Add `pub mod subagent_tool;` |
| Modify | `src/gateway/execution_engine/run_loop.rs:60-77` | Create SubagentTool, register it in the LoopToolRegistry |
| Modify | `src/executor/builtin_registry/config.rs:26,44` | Remove `sub_agent_dispatcher`, `sub_agent_registry` fields |
| Modify | `src/executor/builtin_registry/builder.rs:172-220` | Remove subagent_spawn/steer/kill registration block |
| Modify | `src/executor/builtin_registry/registry.rs:84-86,311-329` | Remove struct fields and match arms |
| Modify | `src/builtin_tools/mod.rs:68,120-123` | Remove `subagent_manage` module and re-exports |
| Modify | `src/agents/mod.rs:38,57-61` | Remove `sub_agents` module and re-exports |
| Modify | `src/agents/sub_agents/mod.rs` | Delete entire file |
| Modify | `src/bin/aleph/commands/start/builder/agent_init.rs:253-264,282` | Remove SubAgentDispatcher initialization |
| Delete | `src/builtin_tools/subagent_manage/` | Entire directory (spawn.rs, steer.rs, kill.rs, mod.rs) |
| Delete | `src/agents/sub_agents/` | Entire directory (10 files) |

---

### Task 1: Create SubagentTool

**Files:**
- Create: `src/agent_loop/subagent_tool.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Write the SubagentTool with tests**

Create `src/agent_loop/subagent_tool.rs`:

```rust
//! SubagentTool — delegate a task to a temporary, autonomous sub-agent.
//!
//! The sub-agent runs a full AgentLoop (think → act) with the same
//! tool set as the parent (minus the "subagent" tool itself to prevent
//! recursion). It completes the task independently and returns the result.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::loop_core::{AgentLoop, LoopConfig, NoopCallback};
use super::prompt_builder::PromptBuilder;
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::{LoopTool, LoopToolRegistry, ToolResult};

/// Sub-agent system prompt — focused on task completion.
const SUBAGENT_SYSTEM_PROMPT: &str = "\
You are a focused sub-agent executing a specific task. \
Complete the task using available tools, then return a clear summary of results. \
Be concise and direct. Do not ask clarifying questions — work with what you have.";

/// Parsed arguments from LLM tool call.
#[derive(Debug, Deserialize)]
struct SubagentArgs {
    task: String,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
}

fn default_timeout() -> u64 {
    120
}

/// Factory function type for building the sub-agent's LoopToolRegistry.
///
/// This avoids SubagentTool needing to know about UnifiedTool, ToolRegistry,
/// or the gateway layer. The factory is created in run_loop.rs where those
/// types are available.
pub type ToolRegistryFactory = Arc<dyn Fn() -> LoopToolRegistry + Send + Sync>;

/// A LoopTool that delegates a task to a temporary AgentLoop.
pub struct SubagentTool {
    /// LLM provider (shared with parent)
    provider: Arc<dyn AiProvider>,
    /// Factory to build the sub-agent's tool registry (parent tools minus "subagent")
    tool_registry_factory: ToolRegistryFactory,
    /// Safety guard (inherited from parent)
    safety_guard: SafetyGuard,
}

impl SubagentTool {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard: SafetyGuard,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard,
        }
    }
}

#[async_trait]
impl LoopTool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Delegate a task to an autonomous sub-agent that works independently \
         and returns the result. The sub-agent has access to all your tools \
         and can make multiple tool calls to complete the task. Use this for \
         tasks that can be done in parallel or require focused independent work."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Clear description of what the sub-agent should accomplish"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Max seconds to wait (default: 120)",
                    "default": 120
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let args: SubagentArgs = match serde_json::from_value(input) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::Error {
                    error: format!("Invalid subagent arguments: {}", e),
                    retryable: false,
                };
            }
        };

        info!(task = %args.task, timeout = args.timeout_secs, "Subagent starting");

        // Build sub-agent components
        let bridge = AiProviderBridge::new(Arc::clone(&self.provider));
        let tool_registry = (self.tool_registry_factory)();
        let prompt_builder = PromptBuilder::new()
            .with_soul_identity(SUBAGENT_SYSTEM_PROMPT);
        let config = LoopConfig {
            max_iterations: 25,
            token_budget: 100_000,
            timeout_secs: args.timeout_secs,
        };

        let agent_loop = AgentLoop::new(
            bridge,
            tool_registry,
            prompt_builder,
            self.safety_guard.clone(),
            config,
        );

        let mut callback = NoopCallback;
        match agent_loop.run(&args.task, &mut callback).await {
            Ok(result) => {
                info!(
                    iterations = result.iterations,
                    tool_calls = result.tool_calls_made,
                    hit_limit = result.hit_limit,
                    "Subagent completed"
                );
                let text = result.final_text.unwrap_or_else(|| {
                    if result.hit_limit {
                        format!(
                            "Sub-agent hit limits ({} iterations, {} tool calls) without producing a final answer.",
                            result.iterations, result.tool_calls_made
                        )
                    } else {
                        "(No response from sub-agent)".to_string()
                    }
                });
                ToolResult::Success {
                    output: json!({
                        "result": text,
                        "iterations": result.iterations,
                        "tool_calls_made": result.tool_calls_made,
                    }),
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Subagent failed");
                ToolResult::Error {
                    error: format!("Sub-agent execution failed: {}", e),
                    retryable: true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::loop_core::LoopProvider;
    use crate::providers::adapter::{ProviderResponse, StopReason};
    use crate::providers::message::UnifiedMessage;
    use super::super::tool::ToolDefinition;

    // Mock provider that returns a text response
    struct MockProvider;

    #[async_trait]
    impl LoopProvider for MockProvider {
        async fn call(
            &self,
            _messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<ProviderResponse> {
            Ok(ProviderResponse::text_only("Sub-agent result: task completed.".to_string()))
        }
    }

    // We can't easily test with a real AiProvider, so test the schema and arg parsing
    #[test]
    fn test_subagent_tool_schema() {
        let tool = SubagentTool::new(
            Arc::new(crate::providers::adapter::NoopProvider) as Arc<dyn AiProvider>,
            Arc::new(|| LoopToolRegistry::new()),
            SafetyGuard::default_guard(),
        );
        assert_eq!(tool.name(), "subagent");
        let schema = tool.schema();
        assert_eq!(schema["required"], json!(["task"]));
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["timeout_secs"].is_object());
    }

    #[test]
    fn test_subagent_args_parsing() {
        let args: SubagentArgs = serde_json::from_value(json!({
            "task": "Search for Rust tutorials"
        })).unwrap();
        assert_eq!(args.task, "Search for Rust tutorials");
        assert_eq!(args.timeout_secs, 120);

        let args: SubagentArgs = serde_json::from_value(json!({
            "task": "Do something",
            "timeout_secs": 60
        })).unwrap();
        assert_eq!(args.timeout_secs, 60);
    }

    #[tokio::test]
    async fn test_subagent_invalid_args() {
        let tool = SubagentTool::new(
            Arc::new(crate::providers::adapter::NoopProvider) as Arc<dyn AiProvider>,
            Arc::new(|| LoopToolRegistry::new()),
            SafetyGuard::default_guard(),
        );
        let result = tool.execute(json!({})).await;
        assert!(matches!(result, ToolResult::Error { retryable: false, .. }));
    }
}
```

Note: This code references `crate::providers::adapter::NoopProvider` — check if it exists. If not, the test can use a simple stub. The actual execution test is covered by Task 2's integration via run_loop.

- [ ] **Step 2: Add module declaration**

In `src/agent_loop/mod.rs`, add:
```rust
pub mod subagent_tool;
```

- [ ] **Step 3: Run compile check**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: May fail if `NoopProvider` doesn't exist — adjust test to use a local mock.

- [ ] **Step 4: Fix any compilation issues, run tests**

Run: `cargo test -p alephcore --lib agent_loop::subagent_tool -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/subagent_tool.rs src/agent_loop/mod.rs
git commit -m "agent_loop: add SubagentTool — real LLM-powered sub-agent"
```

---

### Task 2: Wire SubagentTool into run_agent_loop

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs:60-77`

- [ ] **Step 1: Register SubagentTool in run_agent_loop**

In `run_loop.rs`, after `let tool_registry = build_registry_from_tools(...)` (line 76), add:

```rust
// Register subagent tool — runs a sub-AgentLoop with same tools minus "subagent"
{
    use crate::agent_loop::subagent_tool::SubagentTool;

    let sub_provider = provider.clone();
    let sub_tool_registry = self.tool_registry.clone();
    let sub_allowed_tools: Vec<_> = allowed_tools
        .iter()
        .filter(|t| t.name != "subagent")
        .cloned()
        .collect();
    let sub_working_dir = default_working_dir.clone();

    let factory: crate::agent_loop::subagent_tool::ToolRegistryFactory =
        Arc::new(move || {
            build_registry_from_tools(
                sub_tool_registry.clone(),
                &sub_allowed_tools,
                sub_working_dir.clone(),
            )
        });

    let subagent = SubagentTool::new(sub_provider, factory, safety.clone());
    tool_registry.register(Box::new(subagent));
}
```

Note: `tool_registry` is currently immutable after construction. May need to change `let tool_registry =` to `let mut tool_registry =` on line 72.

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: PASS (or minor fixes needed for mutability)

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p alephcore --lib agent_loop -- --nocapture`
Expected: All existing tests still PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "gateway: wire SubagentTool into agent loop with parent's tools"
```

---

### Task 3: Remove old subagent_manage tools

**Files:**
- Delete: `src/builtin_tools/subagent_manage/` (entire directory)
- Modify: `src/builtin_tools/mod.rs` — remove module declaration and re-exports
- Modify: `src/executor/builtin_registry/config.rs` — remove `sub_agent_dispatcher`, `sub_agent_registry` fields
- Modify: `src/executor/builtin_registry/builder.rs` — remove subagent registration block (lines 172-220)
- Modify: `src/executor/builtin_registry/registry.rs` — remove struct fields (lines 84-86) and match arms (lines 311-329)
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs` — remove SubAgentDispatcher initialization

- [ ] **Step 1: Delete subagent_manage directory**

```bash
rm -rf src/builtin_tools/subagent_manage/
```

- [ ] **Step 2: Remove module declaration and re-exports from builtin_tools/mod.rs**

Remove line 68: `pub mod subagent_manage;`
Remove lines 120-123: the `pub use subagent_manage::{...};` block

- [ ] **Step 3: Remove config fields**

In `config.rs`, remove:
- Line 8: `use crate::agents::sub_agents::{SubAgentDispatcher, SubAgentRegistry};`
- Line 26: `pub sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>,`
- Line 44: `pub sub_agent_registry: Option<Arc<SubAgentRegistry>>,`

- [ ] **Step 4: Remove builder registration block**

In `builder.rs`, remove the entire block from line 172 (`// Add subagent management tools`) through line 220 (`(None, None, None)`).

Remove the corresponding struct field assignments wherever the builder stores `subagent_spawn_tool`, `subagent_steer_tool`, `subagent_kill_tool`.

- [ ] **Step 5: Remove registry struct fields and match arms**

In `registry.rs`:
- Remove lines 84-86 (the three struct fields)
- Remove lines 311-329 (the three match arms for "subagent_spawn"/"subagent_steer"/"subagent_kill")

- [ ] **Step 6: Remove SubAgentDispatcher from agent_init.rs**

In `agent_init.rs`:
- Remove the `sub_agent_disp` variable declaration (line 109)
- Remove the SubAgentDispatcher creation block (lines 253-264)
- Remove `sub_agent_dispatcher` from tool_config struct literal (line 282)
- Remove `sub_agent_dispatcher` from the returned InitResult struct (line 748)
- Remove the field from the InitResult struct definition (line 76)

- [ ] **Step 7: Compile check and fix cascading errors**

Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: May have errors from other files importing SubAgentDispatcher. Fix each one.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "cleanup: remove subagent_manage tools and BuiltinToolConfig references"
```

---

### Task 4: Remove agents/sub_agents module

**Files:**
- Delete: `src/agents/sub_agents/` (entire directory — 10 files)
- Modify: `src/agents/mod.rs` — remove module declaration and re-exports

- [ ] **Step 1: Check for external references**

Run: `cargo check -p alephcore 2>&1 | grep "sub_agents"` to see if anything still depends on the module.

- [ ] **Step 2: Delete the directory**

```bash
rm -rf src/agents/sub_agents/
```

- [ ] **Step 3: Update agents/mod.rs**

Remove line 38: `pub mod sub_agents;`
Remove lines 57-61: the `pub use sub_agents::{...};` block
Update the module doc comment (lines 17-22) to remove sub_agents references.

- [ ] **Step 4: Fix all compilation errors**

Run: `cargo check -p alephcore 2>&1 | head -40`
Fix any remaining imports of `sub_agents` types across the codebase.

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -10`
Expected: All tests pass (except pre-existing browser_tools failures)

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -20`
Expected: No new warnings from our changes

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "cleanup: remove agents/sub_agents module (replaced by SubagentTool)"
```

---

### Task 5: Final verification

- [ ] **Step 1: Full compile**

Run: `cargo build --release --bin aleph 2>&1 | tail -5`
Expected: Build succeeds

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -10`
Expected: All tests pass

- [ ] **Step 3: Verify subagent tool is registered**

Start Aleph and check logs for tool registration:
```bash
pkill -f "target/release/aleph" 2>/dev/null; sleep 2
target/release/aleph start 2>&1 | grep -i "subagent\|tool_count" | head -5
```
Expected: Log shows the subagent tool is registered in the loop's tool registry

- [ ] **Step 4: Send a delegation test**

Send a task via WebSocket that should trigger subagent usage:
"帮我同时完成两件事：1) 搜索最新的 Rust 2024 新特性 2) 告诉我今天星期几"

Verify the agent uses the `subagent` tool to delegate at least one sub-task.
