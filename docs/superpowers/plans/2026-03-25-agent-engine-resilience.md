# Agent Engine Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify agent timeout to 48h default with cascade resolution, isolate compression from timeout, make truncation pair-aware, add empty session early return, and provide Panel UI for execution config.

**Architecture:** Five orthogonal changes to the agent execution engine. Timeout control moves from the Loop to the Engine exclusively. Compression extends the Engine deadline dynamically. Truncation operates on conversation rounds (not individual messages). Panel gets a new Execution settings page.

**Tech Stack:** Rust (core), Leptos/WASM (Panel), tokio (async runtime)

**Spec:** `docs/superpowers/specs/2026-03-25-agent-engine-resilience-design.md`

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `src/config/types/execution.rs` | `ExecutionConfig` struct (serde + JsonSchema) |
| `src/gateway/handlers/execution_config.rs` | RPC handler: `execution_config.get` / `.update` |
| `interfaces/webchat/src/views/settings/execution.rs` | Panel Execution settings page |

### Modified Files
| File | Change |
|------|--------|
| `src/agent_loop/loop_core.rs` | Remove `timeout_secs` from `LoopConfig`, delete timeout check, rewrite `enforce_context_limit`, add `find_safe_cut_point` + `remove_oldest_complete_round` |
| `src/gateway/execution_engine/mod.rs` | Update `ExecutionEngineConfig` default to 172_800 |
| `src/gateway/execution_engine/engine.rs` | Resettable deadline, cascade timeout resolution |
| `src/gateway/execution_engine/run_loop.rs` | Accept deadline param, wrap compression, delete duplicate timeout resolution |
| `src/config/types/orchestrator.rs` | Update default timeout to 172_800 |
| `src/config/types/mod.rs` | Add `pub mod execution;` + `pub use execution::*;` |
| `src/config/structs.rs` | Add `execution: ExecutionConfig` field to `Config` |
| `src/memory/session_compactor/mod.rs` | Early return in `prepare_history` |
| `src/gateway/agent_instance.rs` | Add `timeout_secs: Option<u64>` to `AgentInstanceConfig` |
| `src/gateway/handlers/mod.rs` | Add `pub mod execution_config;` |
| `src/bin/aleph-server/commands/start/builder/handlers.rs` | Register `execution_config` RPC handlers |
| `interfaces/webchat/src/views/settings/mod.rs` | Add `pub mod execution;` + `pub use execution::ExecutionView;` |
| `interfaces/webchat/src/components/settings_sidebar.rs` | Add `Execution` tab to `SettingsTab` enum + Advanced group |
| `interfaces/webchat/src/app.rs` | Add `/settings/execution` route |

---

### Task 1: ExecutionConfig type + Config integration

**Files:**
- Create: `src/config/types/execution.rs`
- Modify: `src/config/types/mod.rs:20-71`
- Modify: `src/config/structs.rs:148-152`

- [ ] **Step 1: Create `execution.rs` config type**

```rust
// src/config/types/execution.rs
//! Execution engine configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Execution engine settings (agent timeout, iteration limits)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionConfig {
    /// Default agent timeout in seconds (default: 172800 = 48 hours)
    #[serde(default = "default_timeout_secs")]
    pub default_timeout_secs: u64,

    /// Maximum iterations per agent run (default: 200)
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_timeout_secs() -> u64 {
    172_800
}

fn default_max_iterations() -> usize {
    200
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: default_timeout_secs(),
            max_iterations: default_max_iterations(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = ExecutionConfig::default();
        assert_eq!(config.default_timeout_secs, 172_800);
        assert_eq!(config.max_iterations, 200);
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = ExecutionConfig::default();
        let toml = toml::to_string(&config).unwrap();
        let parsed: ExecutionConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.default_timeout_secs, 172_800);
        assert_eq!(parsed.max_iterations, 200);
    }

    #[test]
    fn test_serde_with_missing_fields() {
        let parsed: ExecutionConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.default_timeout_secs, 172_800);
        assert_eq!(parsed.max_iterations, 200);
    }
}
```

- [ ] **Step 2: Register module in `config/types/mod.rs`**

Add after `pub mod evolution;` (line 24):
```rust
pub mod execution;
```

Add after `pub use evolution::*;` (line 53):
```rust
pub use execution::*;
```

- [ ] **Step 3: Add field to `Config` struct in `structs.rs`**

Add after the `pub acp: AcpConfig,` field (around line 148):
```rust
    /// Execution engine configuration (timeout, iteration limits)
    #[serde(default)]
    pub execution: ExecutionConfig,
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib config::types::execution`
Expected: 3 tests PASS

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 6: Commit**

```bash
git add src/config/types/execution.rs src/config/types/mod.rs src/config/structs.rs
git commit -m "config: add ExecutionConfig type (48h default timeout, 200 max iterations)"
```

---

### Task 2: Timeout unification — remove LoopConfig.timeout_secs

**Files:**
- Modify: `src/agent_loop/loop_core.rs:163-177` (LoopConfig), `:337-342` (timeout check)
- Modify: `src/gateway/execution_engine/mod.rs:41-48` (default)
- Modify: `src/gateway/execution_engine/run_loop.rs:150-158` (LoopConfig construction)
- Modify: `src/config/types/orchestrator.rs:47-49` (default)

- [ ] **Step 1: Remove `timeout_secs` from `LoopConfig` and update default**

In `src/agent_loop/loop_core.rs`, change `LoopConfig` (lines 163-177):

```rust
/// Loop configuration — guards against runaway loops.
pub struct LoopConfig {
    pub max_iterations: usize,
    pub token_budget: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 200,
            token_budget: 100_000,
        }
    }
}
```

- [ ] **Step 2: Delete timeout check in the loop**

In `src/agent_loop/loop_core.rs`, remove lines 338-342 (inside `run_with_history_messages`):

Delete:
```rust
            // Check timeout
            if start.elapsed().as_secs() >= self.config.timeout_secs {
                hit_limit = true;
                break;
            }
```

Also delete the `let start = Instant::now();` line (334) and the `use std::time::Instant;` import if no longer needed (check if `Instant` is used elsewhere in the file).

- [ ] **Step 3: Update `ExecutionEngineConfig` default**

In `src/gateway/execution_engine/mod.rs`, change line 45:

```rust
            default_timeout_secs: 172_800,
```

- [ ] **Step 4: Update `run_loop.rs` — remove duplicate timeout and `timeout_secs` from LoopConfig construction**

In `src/gateway/execution_engine/run_loop.rs`, change lines 150-159:

Replace:
```rust
        // Config from agent
        let max_loops = agent.config().max_loops as usize;
        let timeout_secs = request
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);
        let token_budget = agent.config().max_tokens.unwrap_or(500_000);
        let loop_config = LoopConfig {
            max_iterations: max_loops,
            token_budget,
            timeout_secs,
        };
```

With:
```rust
        // Config from agent
        let max_loops = agent.config().max_loops as usize;
        let token_budget = agent.config().max_tokens.unwrap_or(500_000);
        let loop_config = LoopConfig {
            max_iterations: max_loops,
            token_budget,
        };
```

- [ ] **Step 5: Wire `Config.execution` into `ExecutionEngineConfig` at startup**

In `src/bin/aleph-server/commands/start/builder/agent_init.rs`, replace line 531:

Replace:
```rust
        let engine_config = ExecutionEngineConfig::default();
```

With:
```rust
        let engine_config = ExecutionEngineConfig {
            default_timeout_secs: app_config.execution.default_timeout_secs,
            ..Default::default()
        };
```

Also update `run_loop.rs` to use config-driven `max_iterations` fallback. In the `LoopConfig` construction (around line 155 after Task 2 changes), change:

```rust
        let max_loops = agent.config().max_loops as usize;
```

to:

```rust
        let max_loops = if agent.config().max_loops > 0 {
            agent.config().max_loops as usize
        } else {
            self.config.max_iterations_default.unwrap_or(200)
        };
```

**Note**: This is optional — the current `agent.config().max_loops` defaults to 100 which is already a per-agent override. The global `Config.execution.max_iterations` will only be used when constructing agents without explicit `max_loops`. Since `AgentInstanceConfig::default().max_loops = 100`, existing agents are unaffected. The Panel value is available for future use.

- [ ] **Step 6: Update `OrchestratorGuards` default**

In `src/config/types/orchestrator.rs`, change `default_timeout_seconds` (line 47-49):

```rust
fn default_timeout_seconds() -> u64 {
    172_800
}
```

Also update the test assertion on line 99:
```rust
        assert_eq!(config.guards.timeout_seconds, 172_800);
```

- [ ] **Step 7: Update all LoopConfig constructions in tests**

In `src/agent_loop/loop_core.rs` — approximately 14 test instances that construct `LoopConfig { max_iterations: N, token_budget: M, timeout_secs: T }`. Remove the `timeout_secs` field from each.

In `src/agent_loop/integration_probe.rs` — line 168: remove `timeout_secs` field.

In `src/agent_loop/subagent_tool.rs` — line 132: remove `timeout_secs` field.

In `src/agent_loop/factory.rs` — lines 135, 158, 187, 214: remove `timeout_secs` field from each.

In `src/gateway/execution_engine/tests.rs` — update any `LoopConfig` construction.

- [ ] **Step 8: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: All existing tests pass (timeout-related behavior now managed by Engine, not Loop)

- [ ] **Step 9: Commit**

```bash
git add src/agent_loop/ src/gateway/execution_engine/ src/config/types/orchestrator.rs
git commit -m "engine: unify timeout to Engine layer, remove LoopConfig.timeout_secs, default 48h"
```

---

### Task 3: Cascade timeout resolution + agent-level override

**Files:**
- Modify: `src/gateway/agent_instance.rs:17-44` (AgentInstanceConfig)
- Modify: `src/gateway/execution_engine/engine.rs:349-351` (timeout resolution)

- [ ] **Step 1: Add `timeout_secs` to `AgentInstanceConfig`**

In `src/gateway/agent_instance.rs`, add after `tool_permissions` field (line 43):

```rust
    /// Optional per-agent timeout override (seconds). None = use global default.
    pub timeout_secs: Option<u64>,
```

Update the `Default` impl (add after `tool_permissions: None,` on line 65):
```rust
            timeout_secs: None,
```

- [ ] **Step 2: Add config accessor**

In `AgentInstanceConfig` impl block (after `tool_permissions()` method, around line 76):

```rust
    /// Return the agent's timeout override, if set.
    pub fn timeout_secs(&self) -> Option<u64> {
        self.timeout_secs
    }
```

- [ ] **Step 3: Update cascade resolution in `engine.rs`**

In `src/gateway/execution_engine/engine.rs`, replace lines 349-351:

Replace:
```rust
        let timeout_secs = request
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);
```

With:
```rust
        let timeout_secs = request
            .timeout_secs
            .or(agent.config().timeout_secs())
            .unwrap_or(self.config.default_timeout_secs);
```

- [ ] **Step 4: Update any `AgentInstanceConfig` construction sites that use struct literal syntax**

Search for `AgentInstanceConfig {` across the codebase and add `timeout_secs: None,` where needed. Key locations:
- `src/gateway/agent_instance.rs` — `from_resolved_agent()` method
- `src/bin/aleph-server/commands/start/builder/agent_init.rs`
- Any test files constructing `AgentInstanceConfig`

Run: `cargo check -p alephcore` to find any missing fields.

- [ ] **Step 5: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/gateway/agent_instance.rs src/gateway/execution_engine/engine.rs src/bin/aleph-server/
git commit -m "engine: add agent-level timeout override, three-layer cascade resolution"
```

---

### Task 4: Resettable deadline (compression isolation)

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs:348-370` (execute method)
- Modify: `src/gateway/execution_engine/run_loop.rs:34-40,191-201` (run_agent_loop signature + compression wrapping)

- [ ] **Step 1: Add `wait_for_deadline` helper in `engine.rs`**

Add at the bottom of `engine.rs`, before the closing `}` of the impl block (or as a free function outside the impl):

```rust
/// Wait until the resettable deadline expires.
///
/// The deadline can be extended by compression tasks. This function re-checks
/// after waking to handle extensions that occurred during sleep.
async fn wait_for_deadline(deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>) {
    loop {
        let dl = *deadline.lock().await;
        tokio::time::sleep_until(dl).await;
        // Re-check: deadline may have been extended while we slept.
        if tokio::time::Instant::now() >= *deadline.lock().await {
            break;
        }
        // Guard against theoretical busy-spin if deadline is in the past
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
```

Add necessary imports at the top of `engine.rs`:
```rust
use tokio::sync::Mutex as TokioMutex;
```

- [ ] **Step 2: Replace fixed sleep with resettable deadline in `execute()`**

In `engine.rs`, replace lines 348-370:

Replace:
```rust
        let timeout_secs = request
            .timeout_secs
            .or(agent.config().timeout_secs())
            .unwrap_or(self.config.default_timeout_secs);

        let result = tokio::select! {
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
            ) => result,

            _ = cancel_rx.recv() => {
                info!("Run {} cancelled", run_id);
                Err(ExecutionError::Cancelled)
            }

            _ = tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)) => {
                warn!("Run {} timed out after {}s", run_id, timeout_secs);
                Err(ExecutionError::Timeout)
            }
        };
```

With:
```rust
        let timeout_secs = request
            .timeout_secs
            .or(agent.config().timeout_secs())
            .unwrap_or(self.config.default_timeout_secs);

        let deadline = Arc::new(TokioMutex::new(
            tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs)
        ));

        let result = tokio::select! {
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
                deadline.clone(),
            ) => result,

            _ = cancel_rx.recv() => {
                info!("Run {} cancelled", run_id);
                Err(ExecutionError::Cancelled)
            }

            _ = wait_for_deadline(deadline.clone()) => {
                warn!("Run {} timed out after {}s effective time", run_id, timeout_secs);
                Err(ExecutionError::Timeout)
            }
        };
```

- [ ] **Step 3: Update `run_agent_loop` signature to accept deadline**

In `src/gateway/execution_engine/run_loop.rs`, change the signature (lines 34-40):

Replace:
```rust
    pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
```

With:
```rust
    pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
    ) -> Result<String, ExecutionError> {
```

- [ ] **Step 4: Wrap `prepare_history` with deadline extension**

In `run_loop.rs`, wrap the `prepare_history` call (lines 191-201):

Replace:
```rust
        // Load conversation history from session (for multi-turn context)
        let mut history = if let Some(ref sc) = self.session_compactor {
            sc.prepare_history(
                &agent,
                &request.session_key,
                &request.input,
                token_budget as u64,
            )
            .await
        } else {
            build_loop_history(&agent, &request.session_key, &request.input).await
        };
```

With:
```rust
        // Load conversation history from session (for multi-turn context)
        // Compression time is excluded from the agent's timeout budget.
        let before_compress = tokio::time::Instant::now();
        let mut history = if let Some(ref sc) = self.session_compactor {
            sc.prepare_history(
                &agent,
                &request.session_key,
                &request.input,
                token_budget as u64,
            )
            .await
        } else {
            build_loop_history(&agent, &request.session_key, &request.input).await
        };
        let compress_elapsed = before_compress.elapsed();
        if !compress_elapsed.is_zero() {
            *deadline.lock().await += compress_elapsed;
        }
```

- [ ] **Step 5: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/gateway/execution_engine/engine.rs src/gateway/execution_engine/run_loop.rs
git commit -m "engine: resettable deadline — compression time excluded from agent timeout"
```

---

### Task 5: Pair-aware truncation

**Files:**
- Modify: `src/agent_loop/loop_core.rs:37-116` (enforce_context_limit)

- [ ] **Step 1: Write tests for `find_safe_cut_point`**

In `src/agent_loop/loop_core.rs`, add to the existing `#[cfg(test)] mod tests` section:

```rust
    // --- find_safe_cut_point ---

    #[test]
    fn test_safe_cut_at_user_message() {
        let msgs = vec![
            UnifiedMessage::user("old"),
            UnifiedMessage::user("recent"),
            UnifiedMessage::assistant("reply"),
        ];
        assert_eq!(find_safe_cut_point(&msgs, 1), 1);
    }

    #[test]
    fn test_safe_cut_skips_tool_result() {
        let msgs = vec![
            UnifiedMessage::user("query"),
            UnifiedMessage::assistant_with_tool_calls("thinking", vec![tool_call("tc1", "search", json!({}))]),
            UnifiedMessage::tool_result("tc1", "search", "results", false),
            UnifiedMessage::assistant("done"),
            UnifiedMessage::user("followup"),
        ];
        // initial_cut = 2 lands on ToolResult → walk back to 1 (Assistant with tool calls) → break
        assert_eq!(find_safe_cut_point(&msgs, 2), 1);
    }

    #[test]
    fn test_safe_cut_at_plain_assistant() {
        let msgs = vec![
            UnifiedMessage::user("hi"),
            UnifiedMessage::assistant("hello"),
            UnifiedMessage::user("bye"),
        ];
        assert_eq!(find_safe_cut_point(&msgs, 2), 2);
    }

    #[test]
    fn test_safe_cut_at_zero() {
        let msgs = vec![
            UnifiedMessage::tool_result("tc1", "t", "o", false),
            UnifiedMessage::assistant("done"),
        ];
        assert_eq!(find_safe_cut_point(&msgs, 0), 0);
    }

    // --- remove_oldest_complete_round ---

    #[test]
    fn test_remove_round_user_message() {
        let mut msgs = vec![
            UnifiedMessage::user("[SYSTEM] Truncated"),
            UnifiedMessage::user("old question"),
            UnifiedMessage::assistant("answer"),
        ];
        remove_oldest_complete_round(&mut msgs);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[1].is_assistant());
    }

    #[test]
    fn test_remove_round_tool_group() {
        let mut msgs = vec![
            UnifiedMessage::user("[SYSTEM] Truncated"),
            UnifiedMessage::assistant_with_tool_calls("", vec![tool_call("tc1", "s", json!({}))]),
            UnifiedMessage::tool_result("tc1", "s", "out", false),
            UnifiedMessage::user("next"),
        ];
        remove_oldest_complete_round(&mut msgs);
        assert_eq!(msgs.len(), 2); // notice + user("next")
        assert!(msgs[1].text_content().contains("next"));
    }

    #[test]
    fn test_remove_round_preserves_minimum() {
        let mut msgs = vec![
            UnifiedMessage::user("[SYSTEM] Truncated"),
            UnifiedMessage::user("last"),
        ];
        remove_oldest_complete_round(&mut msgs);
        assert_eq!(msgs.len(), 2); // Should not remove below 2
    }
```

These tests require helper functions. Add them at the top of the `#[cfg(test)] mod tests` block:

```rust
    use serde_json::json;

    /// Test helper: create an Assistant message with tool calls
    fn assistant_with_tool_calls(text: &str, calls: Vec<(&str, &str, Value)>) -> UnifiedMessage {
        let mut content = vec![ContentBlock::Text { text: text.to_string() }];
        for (id, name, args) in calls {
            content.push(ContentBlock::ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: args,
            });
        }
        UnifiedMessage::Assistant { content }
    }
```

Then update test calls to use tuple syntax, e.g.:
`assistant_with_tool_calls("thinking", vec![("tc1", "search", json!({}))])`

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- find_safe_cut_point remove_oldest_complete_round`
Expected: FAIL (functions don't exist yet)

- [ ] **Step 3: Implement `find_safe_cut_point` and `remove_oldest_complete_round`**

In `src/agent_loop/loop_core.rs`, add before `enforce_context_limit`:

```rust
const TRUNCATION_NOTICE: &str =
    "[SYSTEM] Earlier conversation history and memory context were truncated \
     to fit the model's context window. Continue based on the remaining context.";

/// Find a safe cut point that doesn't split ToolCall/ToolResult pairs.
///
/// Walks backwards from `initial_cut` until landing on a clean boundary:
/// a User message or a plain Assistant (no tool calls).
fn find_safe_cut_point(messages: &[UnifiedMessage], initial_cut: usize) -> usize {
    let mut cut = initial_cut;
    while cut > 0 {
        if messages[cut].is_tool_result() {
            // ToolResult without its ToolCall at this boundary — walk back
            cut -= 1;
        } else if messages[cut].is_assistant() && messages[cut].has_tool_calls() {
            // This Assistant's ToolResults are after `cut`.
            // `drain(0..cut)` excludes index `cut`, so this
            // Assistant and its ToolResults are preserved.
            break;
        } else {
            break; // User or plain Assistant — clean boundary
        }
    }
    cut
}

/// Remove the oldest complete conversation round after the truncation notice.
///
/// Precondition: `messages[0]` is the truncation notice (User message).
fn remove_oldest_complete_round(messages: &mut Vec<UnifiedMessage>) {
    if messages.len() <= 2 {
        return;
    }

    if messages[1].is_assistant() && messages[1].has_tool_calls() {
        // Remove the Assistant + all consecutive ToolResults after it
        let mut end = 2;
        while end < messages.len() && messages[end].is_tool_result() {
            end += 1;
        }
        messages.drain(1..end);
    } else {
        messages.remove(1);
    }
}
```

- [ ] **Step 4: Rewrite `enforce_context_limit` to use pair-aware truncation**

Replace the existing `enforce_context_limit` function (lines 37-116) with:

```rust
/// Hard safety net: truncate message history if total estimated tokens exceed budget.
///
/// Truncation boundaries fall on complete "conversation rounds" — never splitting
/// ToolCall/ToolResult pairs. A round is: User msg, plain Assistant msg, or
/// Assistant(ToolCalls) + all corresponding ToolResult messages.
///
/// **Philosophy**: keep the agent running > preserve history.
fn enforce_context_limit(
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    tool_defs: &[ToolDefinition],
    token_budget: usize,
    fresh_tail_count: usize,
    ratio: f64,
) {
    use crate::memory::session_compactor::context_window::{
        estimate_tokens, estimate_total_tokens,
    };

    // Estimate overhead from system prompt + tool definitions
    let prompt_tokens = estimate_tokens(system_prompt, ratio);
    let tool_tokens: usize = tool_defs
        .iter()
        .map(|td| {
            estimate_tokens(&td.name, ratio)
                + estimate_tokens(&td.description, ratio)
                + estimate_tokens(&td.parameters.to_string(), ratio)
        })
        .sum();
    let overhead = prompt_tokens + tool_tokens;
    let msg_budget = token_budget.saturating_sub(overhead);
    let msg_tokens = estimate_total_tokens(messages, ratio);

    if msg_tokens <= msg_budget {
        return;
    }

    tracing::warn!(
        target: "agent_loop",
        msg_tokens, msg_budget, overhead,
        total = msg_tokens + overhead,
        budget = token_budget,
        "Context exceeds budget after compaction — enforcing hard limit"
    );

    // Phase 1: Find safe cut point at round boundary
    let tail_start = messages.len().saturating_sub(fresh_tail_count);
    let cut = find_safe_cut_point(messages, tail_start);

    if cut > 0 {
        messages.drain(0..cut);
        messages.insert(0, UnifiedMessage::user(TRUNCATION_NOTICE));
    }

    // Phase 2: If still over budget, remove oldest complete rounds one by one
    while messages.len() > 2 && estimate_total_tokens(messages, ratio) > msg_budget {
        remove_oldest_complete_round(messages);
    }

    let final_tokens = estimate_total_tokens(messages, ratio);
    tracing::warn!(
        target: "agent_loop",
        remaining_messages = messages.len(),
        final_tokens, msg_budget,
        "Context limit enforced (pair-aware)"
    );
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- find_safe_cut_point remove_oldest_complete_round enforce_context`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "loop: pair-aware truncation — never orphan ToolCall/ToolResult pairs"
```

---

### Task 6: Empty session early return

**Files:**
- Modify: `src/memory/session_compactor/mod.rs:170-289` (prepare_history)

- [ ] **Step 1: Add early return in `prepare_history`**

In `src/memory/session_compactor/mod.rs`, replace lines 176-187:

Replace:
```rust
        self.metrics.prepare_history_calls.fetch_add(1, Ordering::Relaxed);
        tracing::info!(target: "session_compactor", "prepare");

        if !self.config.enabled {
            // Disabled: return raw history from the agent (last N messages).
            let raw = agent.get_history(session_key, None).await;
            return raw
                .into_iter()
                .map(|m| session_message_to_unified(&m))
                .collect();
        }

        let session_id = session_key.to_key_string();
```

With:
```rust
        self.metrics.prepare_history_calls.fetch_add(1, Ordering::Relaxed);
        tracing::info!(target: "session_compactor", "prepare");

        if !self.config.enabled {
            let raw = agent.get_history(session_key, None).await;
            return raw
                .into_iter()
                .map(|m| session_message_to_unified(&m))
                .collect();
        }

        // Short session: skip LanceDB query when there's nothing to compress
        let raw_messages = agent.get_history(session_key, None).await;
        if raw_messages.len() <= self.config.fresh_tail_count {
            return raw_messages.iter().map(session_message_to_unified).collect();
        }

        let session_id = session_key.to_key_string();
```

Then delete the duplicate fetch at the original line 227. The line to remove is:

```rust
        // DELETE THIS LINE (was ~line 227, now moved above):
        let raw_messages = agent.get_history(session_key, None).await;
```

The `raw_messages` variable is already in scope from the early return block above. The code that follows (starting with `let pairs: Vec<...> = raw_messages.iter()...`) will use the same variable.

- [ ] **Step 2: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib session_compactor`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/memory/session_compactor/mod.rs
git commit -m "compactor: skip LanceDB query for short sessions (early return)"
```

---

### Task 7: Backend RPC handler for execution config

**Files:**
- Create: `src/gateway/handlers/execution_config.rs`
- Modify: `src/gateway/handlers/mod.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs`

- [ ] **Step 1: Create handler following browser_config pattern**

```rust
// src/gateway/handlers/execution_config.rs
//! Execution engine configuration RPC handlers
//!
//! Provides RPC methods for managing agent execution settings (timeout, iterations).

use crate::config::Config;
use crate::config::types::ExecutionConfig;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde_json::Value;
use tokio::sync::RwLock;

/// Get execution configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;
    match serde_json::to_value(&cfg.execution) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {}", e),
        ),
    }
}

/// Update execution configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let update: ExecutionConfig = match serde_json::from_value(params) {
        Ok(u) => u,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            );
        }
    };

    // Validate ranges
    if update.default_timeout_secs < 60 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "default_timeout_secs must be at least 60 (1 minute)",
        );
    }
    if update.default_timeout_secs > 604_800 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "default_timeout_secs must be at most 604800 (7 days)",
        );
    }
    if update.max_iterations < 5 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "max_iterations must be at least 5",
        );
    }
    if update.max_iterations > 10_000 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "max_iterations must be at most 10000",
        );
    }

    {
        let mut cfg = config.write().await;
        cfg.execution = update.clone();

        if let Err(e) = cfg.save_incremental(&["execution"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("execution".to_string()),
        value: serde_json::to_value(&update).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
```

- [ ] **Step 2: Register module in `handlers/mod.rs`**

Add after `pub mod browser_config;` (line 52):
```rust
pub mod execution_config;
```

- [ ] **Step 3: Register RPC handlers in `builder/handlers.rs`**

Add import after `use alephcore::gateway::handlers::browser_config;` (line 538):
```rust
    use alephcore::gateway::handlers::execution_config;
```

Add handler registration after the browser_config block (after line 633):
```rust
    // Execution config
    register_handler!(server, "execution_config.get", execution_config::handle_get, config);
    register_handler!(server, "execution_config.update", execution_config::handle_update, config, event_bus);
```

- [ ] **Step 4: Compile and test**

Run: `cargo check -p alephcore && cargo check --bin aleph-server`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/execution_config.rs src/gateway/handlers/mod.rs src/bin/aleph-server/commands/start/builder/handlers.rs
git commit -m "gateway: add execution_config RPC handler (get/update with validation)"
```

---

### Task 8: Panel Execution settings page

**Files:**
- Create: `interfaces/webchat/src/views/settings/execution.rs`
- Modify: `interfaces/webchat/src/views/settings/mod.rs`
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs`
- Modify: `interfaces/webchat/src/app.rs`

- [ ] **Step 1: Create Execution settings page**

```rust
// interfaces/webchat/src/views/settings/execution.rs
//! Execution engine settings page

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::api::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionConfig {
    pub default_timeout_secs: u64,
    pub max_iterations: usize,
}

struct ExecutionConfigApi;

impl ExecutionConfigApi {
    async fn get(state: &DashboardState) -> Result<ExecutionConfig, String> {
        let result = state.rpc_call("execution_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    async fn update(state: &DashboardState, config: &ExecutionConfig) -> Result<(), String> {
        let params = serde_json::to_value(config).map_err(|e| e.to_string())?;
        state.rpc_call("execution_config.update", params).await?;
        Ok(())
    }
}

fn format_duration(secs: u64) -> String {
    if secs >= 86400 {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        if hours > 0 {
            format!("{} days {} hours", days, hours)
        } else {
            format!("{} days", days)
        }
    } else if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins > 0 {
            format!("{} hours {} min", hours, mins)
        } else {
            format!("{} hours", hours)
        }
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{} sec", secs)
    }
}

#[component]
pub fn ExecutionView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let config = RwSignal::new(ExecutionConfig::default());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load on mount
    {
        let state = state.clone();
        spawn_local(async move {
            match ExecutionConfigApi::get(&state).await {
                Ok(c) => {
                    config.set(c);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    loading.set(false);
                }
            }
        });
    }

    let save = move |_| {
        let state = state.clone();
        saving.set(true);
        error.set(None);
        spawn_local(async move {
            let c = config.get();
            match ExecutionConfigApi::update(&state, &c).await {
                Ok(()) => saving.set(false),
                Err(e) => {
                    error.set(Some(e));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="p-8 max-w-5xl mx-auto">
            <h1 class="text-2xl font-bold mb-6 text-text-primary">"Execution"</h1>

            <Show when=move || loading.get()>
                <p class="text-text-secondary">"Loading..."</p>
            </Show>

            <Show when=move || !loading.get()>
                <div class="space-y-6">
                    // Default Timeout
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-1">"Default Agent Timeout"</h2>
                        <p class="text-sm text-text-secondary mb-4">
                            "Maximum time an agent run can execute before being terminated. "
                            "Individual agents can override this value."
                        </p>
                        <div class="flex items-center gap-4">
                            <input
                                type="number"
                                min="60"
                                max="604800"
                                class="w-40 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                                prop:value=move || config.get().default_timeout_secs.to_string()
                                on:change=move |ev| {
                                    let val: u64 = event_target_value(&ev).parse().unwrap_or(172_800);
                                    config.update(|c| c.default_timeout_secs = val);
                                }
                            />
                            <span class="text-sm text-text-secondary">
                                "seconds ("
                                {move || format_duration(config.get().default_timeout_secs)}
                                ")"
                            </span>
                        </div>
                    </div>

                    // Max Iterations
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-1">"Max Iterations"</h2>
                        <p class="text-sm text-text-secondary mb-4">
                            "Maximum number of think-act loop iterations per agent run."
                        </p>
                        <input
                            type="number"
                            min="5"
                            max="10000"
                            class="w-40 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                            prop:value=move || config.get().max_iterations.to_string()
                            on:change=move |ev| {
                                let val: usize = event_target_value(&ev).parse().unwrap_or(200);
                                config.update(|c| c.max_iterations = val);
                            }
                        />
                    </div>

                    // Error display
                    <Show when=move || error.get().is_some()>
                        <div class="text-red-500 text-sm">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    // Save button
                    <button
                        class="px-4 py-2 bg-accent-primary text-white rounded-lg hover:bg-accent-primary/90 disabled:opacity-50"
                        disabled=move || saving.get()
                        on:click=save
                    >
                        {move || if saving.get() { "Saving..." } else { "Save" }}
                    </button>
                </div>
            </Show>
        </div>
    }
}
```

- [ ] **Step 2: Register in settings/mod.rs**

Add after `pub mod browser;` (line 18):
```rust
pub mod execution;
```

Add after `pub use auth::AuthView;` (line 38):
```rust
pub use execution::ExecutionView;
```

- [ ] **Step 3: Add tab to settings_sidebar.rs**

Add `Execution` variant to `SettingsTab` enum (after `Browser` on line 38):
```rust
    Execution,
```

Add path mapping in `path()` method (after Browser line 66):
```rust
            Self::Execution => "/settings/execution",
```

Add label in `i18n_label()` method (after Browser line 94):
```rust
            Self::Execution => "Execution".to_string(),
```

Add icon in `icon_svg()` method (after Browser line 122):
```rust
            Self::Execution => r#"<path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"/>"#,
```

Add to the "Advanced" group in `SETTINGS_GROUPS` (after `SettingsTab::Browser` on line 187):
```rust
            SettingsTab::Execution,
```

- [ ] **Step 4: Add route in app.rs**

Add after the Browser route (after line 150):
```rust
            "/settings/execution" => view! { <ExecutionView /> }.into_any(),
```

- [ ] **Step 5: Compile Panel**

Run: `cd /Users/zouguojun/Workspace/Aleph/interfaces/webchat && trunk build`
Expected: PASS (or use `cargo check` for the webchat crate)

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/settings/execution.rs interfaces/webchat/src/views/settings/mod.rs interfaces/webchat/src/components/settings_sidebar.rs interfaces/webchat/src/app.rs
git commit -m "panel: add Execution settings page (timeout + iterations config)"
```

---

### Task 9: Final integration test + cleanup

**Files:**
- All modified files

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore && cargo check --bin aleph-server`
Expected: PASS

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: All tests PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -W warnings`
Expected: No new warnings

- [ ] **Step 4: Verify no unused imports / dead code**

If clippy reports unused imports (e.g., `std::time::Instant` in loop_core.rs after removing the timeout check), clean them up.

- [ ] **Step 5: Final commit (if any cleanup)**

```bash
git add -A
git commit -m "chore: cleanup unused imports after timeout unification"
```
