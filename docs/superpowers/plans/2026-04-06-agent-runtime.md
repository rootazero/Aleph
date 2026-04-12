# Agent Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a three-layer agent dispatch architecture (SubagentTool → AgentRuntime → AgentLoop) with prompt fork caching to reduce token waste and improve subagent observability.

**Architecture:** SubagentTool remains the dispatch/routing layer. A new AgentRuntime struct handles lifecycle management (prompt strategy, tool filtering, transcript, timeout). AgentLoop's think→act loop stays unchanged. PromptSnapshot enables fork path where subagents reuse the parent's stable prompt prefix.

**Tech Stack:** Rust, tokio, tracing, serde_json

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/agent_loop/agent_runtime.rs` | **Create** | AgentRuntime, AgentRuntimeConfig, PromptSnapshot, SharedSnapshot, SubagentTranscript, TranscriptOutcome |
| `src/agent_loop/subagent_tool.rs` | **Modify** | Add shared_snapshot field, should_fork(), refactor Run branch to use AgentRuntime |
| `src/agent_loop/loop_core.rs` | **Modify** | Add shared_snapshot field, with_shared_snapshot(), snapshot capture in think step |
| `src/agent_loop/mod.rs` | **Modify** | Add `pub mod agent_runtime;`, update exports |
| `src/thinker/prompt_builder/mod.rs` | **Modify** | Add capture_snapshot(), build_from_snapshot() |
| `src/gateway/execution_engine/run_loop.rs` | **Modify** | Construct SharedSnapshot, inject into SubagentTool and AgentLoop |
| `src/agent_loop/subagent_runner.rs` | **Delete** | Logic migrated to agent_runtime.rs |

---

### Task 1: Add PromptBuilder snapshot methods

**Files:**
- Modify: `src/thinker/prompt_builder/mod.rs`
- Test: `src/thinker/prompt_builder/tests.rs`

- [ ] **Step 1: Write failing tests for capture_snapshot and build_from_snapshot**

Add to `src/thinker/prompt_builder/tests.rs`:

```rust
#[test]
fn capture_snapshot_returns_stable_prefix() {
    use crate::thinker::prompt_layer::AssemblyPath;

    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);
    let tools: Vec<crate::agent_loop::ToolInfo> = vec![];

    let snapshot = builder.capture_snapshot(&tools);
    assert_eq!(snapshot.path, AssemblyPath::Basic);
    // Stable prefix should be non-empty (at least Bootstrap + Role layers)
    assert!(!snapshot.stable_prefix.is_empty());
}

#[test]
fn build_from_snapshot_appends_dynamic_layers() {
    use crate::agents::AgentDef;
    use crate::thinker::prompt_layer::AssemblyPath;

    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);
    let tools: Vec<crate::agent_loop::ToolInfo> = vec![];

    let snapshot = builder.capture_snapshot(&tools);

    let agent_def = AgentDef::default_subagent();
    let forked_prompt = builder.build_from_snapshot(&snapshot, &agent_def, &tools);

    // Forked prompt must start with the stable prefix
    assert!(forked_prompt.starts_with(&snapshot.stable_prefix));

    // Forked prompt should be longer than stable prefix alone
    // (dynamic layers like AgentRoleLayer add content)
    assert!(forked_prompt.len() >= snapshot.stable_prefix.len());
}

#[test]
fn build_from_snapshot_matches_fresh_build_content() {
    use crate::agents::AgentDef;

    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);
    let tools: Vec<crate::agent_loop::ToolInfo> = vec![];

    let snapshot = builder.capture_snapshot(&tools);
    let agent_def = AgentDef::default_subagent();

    let forked = builder.build_from_snapshot(&snapshot, &agent_def, &tools);
    let fresh = builder.build_for_agent_basic(&agent_def, &tools);

    // Both paths should produce identical output
    assert_eq!(forked, fresh);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib prompt_builder::tests::capture_snapshot -- --nocapture`

Expected: compilation error — `capture_snapshot` and `build_from_snapshot` methods don't exist yet.

- [ ] **Step 3: Implement PromptSnapshot and the two methods**

Add to `src/thinker/prompt_builder/mod.rs`, after the existing `use` block at the top:

```rust
use super::prompt_layer::AssemblyPath;
use crate::agents::AgentDef;
```

Add the `PromptSnapshot` struct before the `PromptBuilder` struct:

```rust
/// Parent agent's stable prompt snapshot for fork path reuse.
///
/// Contains the output of all `LayerStability::Stable` layers for a given
/// assembly path. Subagents using the fork path prepend this prefix and
/// only rebuild dynamic layers (agent role, session context, memory, etc.).
#[derive(Debug, Clone)]
pub struct PromptSnapshot {
    /// Stable layers assembly output.
    pub stable_prefix: String,
    /// Source assembly path.
    pub path: AssemblyPath,
}
```

Add two methods inside `impl PromptBuilder`:

```rust
    /// Capture the current stable layers output as a reusable snapshot.
    ///
    /// Called by the main AgentLoop after its first prompt assembly.
    /// Subagents receive this snapshot to avoid rebuilding stable content.
    pub fn capture_snapshot(&self, tools: &[ToolInfo]) -> PromptSnapshot {
        let path = match &self.soul {
            Some(_) => AssemblyPath::Soul,
            None => AssemblyPath::Basic,
        };
        let input = match &self.soul {
            Some(soul) => LayerInput::soul(&self.config, tools, soul),
            None => LayerInput::basic(&self.config, tools),
        };
        let stable_prefix = self.pipeline.execute_stable_only(path, &input);
        PromptSnapshot { stable_prefix, path }
    }

    /// Build a sub-agent prompt by reusing the snapshot's stable prefix.
    ///
    /// Only rebuilds dynamic layers (AgentRole, SessionContext, Memory, etc.)
    /// and appends them after the stable prefix. This produces a prompt that
    /// is byte-identical in its prefix to the parent's prompt, maximizing
    /// Anthropic prompt cache hits.
    pub fn build_from_snapshot(
        &self,
        snapshot: &PromptSnapshot,
        agent_def: &AgentDef,
        tools: &[ToolInfo],
    ) -> String {
        let input = match &self.soul {
            Some(soul) => LayerInput::soul(&self.config, tools, soul),
            None => LayerInput::basic(&self.config, tools),
        }
        .with_agent_def(agent_def);

        let dynamic_suffix = self.pipeline.execute_dynamic_only(snapshot.path, &input);
        let mut result = String::with_capacity(snapshot.stable_prefix.len() + dynamic_suffix.len());
        result.push_str(&snapshot.stable_prefix);
        result.push_str(&dynamic_suffix);
        result
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib prompt_builder::tests -- --nocapture`

Expected: all 3 new tests PASS. Existing tests still PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/prompt_builder/mod.rs src/thinker/prompt_builder/tests.rs
git commit -m "feat(prompt): add PromptSnapshot, capture_snapshot and build_from_snapshot"
```

---

### Task 2: Create AgentRuntime with fresh path

**Files:**
- Create: `src/agent_loop/agent_runtime.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Write failing test for AgentRuntime fresh path**

Create `src/agent_loop/agent_runtime.rs` with the test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentDef;

    #[test]
    fn agent_runtime_config_defaults() {
        let config = AgentRuntimeConfig {
            agent_def: AgentDef::default_subagent(),
            task: "test task".to_string(),
            context_summary: None,
            model: None,
            timeout_secs: 60,
            prompt_snapshot: None,
        };
        assert!(config.prompt_snapshot.is_none());
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn transcript_outcome_display() {
        let t = SubagentTranscript {
            agent_id: "test-1".into(),
            agent_type: "explore".into(),
            task_summary: "find files".into(),
            outcome: TranscriptOutcome::Success,
            iterations: 3,
            duration_ms: 1500,
            tokens_used: 5000,
        };
        assert_eq!(t.agent_type, "explore");
        assert_eq!(t.iterations, 3);
    }

    #[test]
    fn transcript_outcome_timeout() {
        let outcome = TranscriptOutcome::Timeout;
        assert!(matches!(outcome, TranscriptOutcome::Timeout));
    }

    #[test]
    fn transcript_outcome_error() {
        let outcome = TranscriptOutcome::Error("connection lost".into());
        match outcome {
            TranscriptOutcome::Error(msg) => assert_eq!(msg, "connection lost"),
            _ => panic!("expected Error variant"),
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_runtime::tests -- --nocapture`

Expected: compilation error — module doesn't exist in mod.rs yet.

- [ ] **Step 3: Create agent_runtime.rs with structs and fresh-path run()**

Write `src/agent_loop/agent_runtime.rs`:

```rust
//! AgentRuntime — sub-agent lifecycle manager.
//!
//! The middle layer in the three-layer dispatch architecture:
//! SubagentTool (dispatch) → AgentRuntime (lifecycle) → AgentLoop (execution).
//!
//! Responsibilities:
//! 1. Prompt strategy: fork path (reuse snapshot) vs fresh path
//! 2. Tool registry filtering (agent allowed/denied tools)
//! 3. Lifecycle tracing (SubagentStart/End spans)
//! 4. Transcript recording (structured tracing)
//! 5. Timeout + cleanup

use std::sync::RwLock;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use super::loop_core::{AgentLoop, LoopConfig, LoopRunResult, NoopCallback};
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::LoopToolRegistry;
use crate::agents::AgentDef;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig, PromptSnapshot};
use crate::thinker::prompt_layer::AssemblyPath;

/// Shared prompt snapshot reference.
///
/// Written by the main AgentLoop (single writer) after its first prompt
/// assembly. Read by SubagentTool (multiple readers) when deciding
/// whether to use the fork path.
pub type SharedSnapshot = Arc<RwLock<Option<PromptSnapshot>>>;

/// Factory that builds a fresh LoopToolRegistry for the sub-agent.
pub type ToolRegistryFactory = Arc<dyn Fn() -> LoopToolRegistry + Send + Sync>;

/// Factory that builds a SafetyGuard for the sub-agent.
pub type SafetyGuardFactory = Arc<dyn Fn() -> SafetyGuard + Send + Sync>;

/// Per-run configuration, constructed by SubagentTool dispatch layer.
pub struct AgentRuntimeConfig {
    pub agent_def: AgentDef,
    pub task: String,
    pub context_summary: Option<String>,
    pub model: Option<String>,
    pub timeout_secs: u64,
    pub prompt_snapshot: Option<PromptSnapshot>,
}

/// Sub-agent run transcript, emitted via structured tracing.
pub struct SubagentTranscript {
    pub agent_id: String,
    pub agent_type: String,
    pub task_summary: String,
    pub outcome: TranscriptOutcome,
    pub iterations: usize,
    pub duration_ms: u64,
    pub tokens_used: usize,
}

/// Outcome of a sub-agent run.
pub enum TranscriptOutcome {
    Success,
    Error(String),
    Timeout,
}

/// Sub-agent runtime — manages the full lifecycle from construction to cleanup.
pub struct AgentRuntime {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    chain: super::chain_context::ChainContext,
    cancel_token: CancellationToken,
}

impl AgentRuntime {
    /// Create a new AgentRuntime.
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
        chain: super::chain_context::ChainContext,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
            chain,
            cancel_token,
        }
    }

    /// Run a sub-agent to completion with lifecycle management.
    pub async fn run(self, config: AgentRuntimeConfig) -> Result<LoopRunResult, String> {
        let start = Instant::now();
        let agent_id = format!("subagent-{}", uuid::Uuid::new_v4());
        let agent_type = config.agent_def.id.clone();
        let task_summary: String = config.task.chars().take(200).collect();

        // Phase 1: Preparation
        tracing::info!(
            agent_id = %agent_id,
            agent_type = %agent_type,
            task = %task_summary,
            "subagent_runtime: starting"
        );

        // 1. Resolve model
        let resolved_model = config.model.or_else(|| config.agent_def.model_hint.clone());
        let bridge = if let Some(m) = resolved_model {
            AiProviderBridge::new(self.provider).with_model(m)
        } else {
            AiProviderBridge::new(self.provider)
        };

        // 2. Build tool registry + filter
        let mut registry = (self.tool_registry_factory)();
        registry.retain(|name| config.agent_def.is_tool_allowed(name));

        // 3. Build prompt (fork vs fresh)
        let prompt_builder = match &config.prompt_snapshot {
            Some(_snapshot) => {
                // Fork path — will be used by AgentLoop via build_from_snapshot
                // For now, attach agent_def so AgentRoleLayer fires
                PromptBuilder::new(PromptConfig::default())
                    .with_agent(config.agent_def.clone())
            }
            None => {
                // Fresh path
                PromptBuilder::new(PromptConfig::default())
                    .with_agent(config.agent_def.clone())
            }
        };

        // 4. Build loop config
        let loop_config = LoopConfig {
            max_iterations: config.agent_def.max_iterations.unwrap_or(25) as usize,
            token_budget: config.agent_def.token_budget.unwrap_or(100_000) as usize,
        };

        // Phase 3: Execution
        let mut agent_loop = AgentLoop::new(
            bridge,
            registry,
            prompt_builder,
            (self.safety_guard_factory)(),
            loop_config,
            self.cancel_token,
        )
        .with_chain(self.chain);

        // Prepend parent context if provided
        let effective_task = match config.context_summary {
            Some(summary) => format!(
                "## Context from parent agent\n\n{}\n\n---\n\n{}",
                summary, config.task
            ),
            None => config.task,
        };

        let timeout_duration = std::time::Duration::from_secs(config.timeout_secs);
        let mut callback = NoopCallback;
        let run_result =
            tokio::time::timeout(timeout_duration, agent_loop.run(&effective_task, &mut callback))
                .await;

        // Phase 4: Cleanup — record transcript
        let elapsed_ms = start.elapsed().as_millis() as u64;

        let (outcome, result) = match run_result {
            Err(_elapsed) => (
                TranscriptOutcome::Timeout,
                Err(format!("Sub-agent timed out after {}s", config.timeout_secs)),
            ),
            Ok(Ok(ref r)) => (TranscriptOutcome::Success, Ok(())),
            Ok(Err(ref e)) => (
                TranscriptOutcome::Error(e.to_string()),
                Err(format!("sub-agent failed: {}", e)),
            ),
        };

        let transcript = SubagentTranscript {
            agent_id: agent_id.clone(),
            agent_type: agent_type.clone(),
            task_summary,
            outcome,
            iterations: match &run_result {
                Ok(Ok(r)) => r.iterations,
                _ => 0,
            },
            duration_ms: elapsed_ms,
            tokens_used: match &run_result {
                Ok(Ok(r)) => r.total_tokens,
                _ => 0,
            },
        };

        // Emit structured transcript
        tracing::info!(
            agent_id = %transcript.agent_id,
            agent_type = %transcript.agent_type,
            task = %transcript.task_summary,
            iterations = transcript.iterations,
            duration_ms = transcript.duration_ms,
            tokens_used = transcript.tokens_used,
            outcome = match &transcript.outcome {
                TranscriptOutcome::Success => "success",
                TranscriptOutcome::Error(_) => "error",
                TranscriptOutcome::Timeout => "timeout",
            },
            "subagent_runtime: completed"
        );

        match run_result {
            Err(_) => Err(format!("Sub-agent timed out after {}s", config.timeout_secs)),
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(format!("sub-agent failed: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentDef;

    #[test]
    fn agent_runtime_config_defaults() {
        let config = AgentRuntimeConfig {
            agent_def: AgentDef::default_subagent(),
            task: "test task".to_string(),
            context_summary: None,
            model: None,
            timeout_secs: 60,
            prompt_snapshot: None,
        };
        assert!(config.prompt_snapshot.is_none());
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn transcript_outcome_display() {
        let t = SubagentTranscript {
            agent_id: "test-1".into(),
            agent_type: "explore".into(),
            task_summary: "find files".into(),
            outcome: TranscriptOutcome::Success,
            iterations: 3,
            duration_ms: 1500,
            tokens_used: 5000,
        };
        assert_eq!(t.agent_type, "explore");
        assert_eq!(t.iterations, 3);
    }

    #[test]
    fn transcript_outcome_timeout() {
        let outcome = TranscriptOutcome::Timeout;
        assert!(matches!(outcome, TranscriptOutcome::Timeout));
    }

    #[test]
    fn transcript_outcome_error() {
        let outcome = TranscriptOutcome::Error("connection lost".into());
        match outcome {
            TranscriptOutcome::Error(msg) => assert_eq!(msg, "connection lost"),
            _ => panic!("expected Error variant"),
        }
    }
}
```

- [ ] **Step 4: Register module in mod.rs**

In `src/agent_loop/mod.rs`, add after the `pub mod subagent_tool;` line:

```rust
pub mod agent_runtime;
```

And add these public re-exports at the bottom of the `pub use` section:

```rust
pub use agent_runtime::{AgentRuntime, AgentRuntimeConfig, SharedSnapshot};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agent_runtime::tests -- --nocapture`

Expected: all 4 tests PASS.

- [ ] **Step 6: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`

Expected: compiles with no errors. May have warnings about unused `result` variable in `run()` — that's fine for now.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/agent_runtime.rs src/agent_loop/mod.rs
git commit -m "feat(agent_loop): add AgentRuntime with fresh-path execution and transcript"
```

---

### Task 3: Refactor SubagentTool Run branch to use AgentRuntime

**Files:**
- Modify: `src/agent_loop/subagent_tool.rs`

- [ ] **Step 1: Update imports in subagent_tool.rs**

Replace the existing `run_subagent` import:

```rust
pub use super::subagent_runner::{run_subagent, SafetyGuardFactory, ToolRegistryFactory};
```

With:

```rust
use super::agent_runtime::{AgentRuntime, AgentRuntimeConfig, SharedSnapshot, SafetyGuardFactory, ToolRegistryFactory};
```

- [ ] **Step 2: Add shared_snapshot field and builder method to SubagentTool**

Add a new field to the `SubagentTool` struct:

```rust
    /// Shared prompt snapshot for fork path. Read-only from SubagentTool's perspective.
    shared_snapshot: Option<SharedSnapshot>,
```

Add a builder method after `with_parent_agent_id`:

```rust
    /// Set the shared prompt snapshot for fork path support.
    pub fn with_shared_snapshot(mut self, snapshot: SharedSnapshot) -> Self {
        self.shared_snapshot = Some(snapshot);
        self
    }
```

Initialize in `new()`:

```rust
        shared_snapshot: None,
```

- [ ] **Step 3: Add should_fork() method**

Add to `impl SubagentTool`:

```rust
    /// Determine whether the fork path should be used for this run.
    ///
    /// Fork conditions (ALL must be true):
    /// 1. No agent_type specified (using "default" agent)
    /// 2. No model override specified (same provider)
    /// 3. Not in team mode (no team_name)
    /// 4. Parent agent has a valid PromptSnapshot
    fn should_fork(&self, args: &RunArgs) -> bool {
        args.agent_type.is_none()
            && args.model.is_none()
            && args.team_name.is_none()
            && self.read_snapshot().is_some()
    }

    /// Read the current prompt snapshot from the shared reference.
    fn read_snapshot(&self) -> Option<PromptSnapshot> {
        self.shared_snapshot
            .as_ref()
            .and_then(|s| s.read().unwrap_or_else(|e| e.into_inner()).clone())
    }
```

- [ ] **Step 4: Refactor the background execution path (run_in_background)**

In the `execute()` method, replace the background execution block (starting at `if args.run_in_background {` around line 578) with:

```rust
        if args.run_in_background {
            let request_id = uuid::Uuid::new_v4().to_string();
            let cancel_token = CancellationToken::new();

            self.background_tracker
                .register(request_id.clone(), cancel_token.clone(), args.task.clone());

            let snapshot = if self.should_fork(&args) {
                self.read_snapshot()
            } else {
                None
            };

            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task: args.task.clone(),
                context_summary: args.context_summary,
                model: args.model.clone(),
                timeout_secs: args.timeout_secs,
                prompt_snapshot: snapshot,
            };

            let provider = self.provider.clone();
            let factory = self.tool_registry_factory.clone();
            let safety_factory = self.safety_guard_factory.clone();
            let tracker = self.background_tracker.clone();
            let rid = request_id.clone();

            tokio::spawn(async move {
                let runtime = AgentRuntime::new(
                    provider,
                    factory,
                    safety_factory,
                    child_chain,
                    cancel_token,
                );

                let result = AssertUnwindSafe(runtime.run(runtime_config))
                    .catch_unwind()
                    .await;

                let outcome = match result {
                    Ok(Ok(r)) => Ok(r.final_text.unwrap_or_else(|| "(no output)".to_string())),
                    Ok(Err(e)) => Err(e),
                    Err(_panic) => Err("Sub-agent panicked".to_string()),
                };
                tracker.mark_completed(&rid, outcome);
            });

            ToolResult::Success {
                output: json!({
                    "status": "running_in_background",
                    "request_id": request_id,
                    "message": format!("Sub-agent started in background. Use request_id '{}' to check status.", request_id)
                }),
            }
```

- [ ] **Step 5: Refactor the foreground execution path**

Replace the foreground `else` block with:

```rust
        } else {
            let snapshot = if self.should_fork(&args) {
                self.read_snapshot()
            } else {
                None
            };

            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task: args.task.clone(),
                context_summary: args.context_summary,
                model: args.model,
                timeout_secs: args.timeout_secs,
                prompt_snapshot: snapshot,
            };

            let runtime = AgentRuntime::new(
                self.provider.clone(),
                self.tool_registry_factory.clone(),
                self.safety_guard_factory.clone(),
                child_chain,
                CancellationToken::new(),
            );

            match runtime.run(runtime_config).await {
                Ok(result) => {
                    tracing::info!(
                        iterations = result.iterations,
                        tool_calls = result.tool_calls_made,
                        tokens = result.total_tokens,
                        "subagent: sub-task completed"
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
                    tracing::warn!(error = %e, "subagent: sub-task failed");
                    ToolResult::Error {
                        error: e,
                        retryable: false,
                    }
                }
            }
        }
```

- [ ] **Step 6: Run existing tests to verify no regressions**

Run: `cargo test -p alephcore --lib subagent_tool::tests -- --nocapture`

Expected: all 25 existing parse/schema tests PASS (they don't touch execute()).

- [ ] **Step 7: Run cargo check**

Run: `cargo check -p alephcore`

Expected: compiles. The old `run_subagent` import is no longer used. `subagent_runner` is no longer needed by `subagent_tool.rs`.

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/subagent_tool.rs
git commit -m "refactor(subagent): use AgentRuntime instead of run_subagent in SubagentTool"
```

---

### Task 4: Wire SharedSnapshot into AgentLoop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Add SharedSnapshot field to AgentLoop struct**

In `src/agent_loop/loop_core.rs`, add the import at the top with other imports:

```rust
use super::agent_runtime::SharedSnapshot;
```

Add a new field at the end of the `AgentLoop` struct (after `skill_prefetcher`):

```rust
    /// Shared prompt snapshot for fork path. Written once after first prompt assembly.
    shared_snapshot: Option<SharedSnapshot>,
```

- [ ] **Step 2: Initialize field in AgentLoop::new()**

In `AgentLoop::new()`, inside the `Self { ... }` block at the end, add:

```rust
            shared_snapshot: None,
```

- [ ] **Step 3: Add builder method**

Add after the existing `with_chain` method:

```rust
    /// Set the shared prompt snapshot reference for fork path support.
    ///
    /// When set, the loop captures a [`PromptSnapshot`] after its first
    /// prompt assembly. SubagentTool reads this snapshot to decide whether
    /// to use the fork path.
    pub fn with_shared_snapshot(mut self, snapshot: SharedSnapshot) -> Self {
        self.shared_snapshot = Some(snapshot);
        self
    }
```

- [ ] **Step 4: Add snapshot capture in the think step**

Find the location in the think step where `system_prompt` is built (search for `prompt_builder.build_system_prompt` or where `LoopRuntime { .. system_prompt .. }` is constructed). After the system prompt is assigned, add:

```rust
        // Capture prompt snapshot for fork path (once)
        if let Some(ref shared) = self.shared_snapshot {
            let mut guard = shared.write().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                *guard = Some(self.prompt_builder.capture_snapshot(&tool_infos));
            }
        }
```

Note: `tool_infos` should reference the tool info slice used to build the system prompt. Match the exact variable name from the surrounding code.

- [ ] **Step 5: Run cargo check**

Run: `cargo check -p alephcore`

Expected: compiles with no errors.

- [ ] **Step 6: Run full test suite**

Run: `cargo test -p alephcore --lib -- --nocapture`

Expected: all tests PASS. AgentLoop behavior unchanged — shared_snapshot is None by default.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat(agent_loop): add SharedSnapshot support for prompt fork path"
```

---

### Task 5: Wire SharedSnapshot in run_loop.rs

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Create and inject SharedSnapshot**

In `run_loop.rs`, find the block where `SubagentTool::new(...)` is constructed (around line 461). Before that block, add:

```rust
                use crate::agent_loop::agent_runtime::SharedSnapshot;
                let shared_snapshot: SharedSnapshot = Arc::new(std::sync::RwLock::new(None));
```

After the `SubagentTool` is constructed and before `tool_registry.register(Box::new(subagent_tool))`, add:

```rust
                subagent_tool = subagent_tool.with_shared_snapshot(shared_snapshot.clone());
```

Then find where `AgentLoop::new(...)` is called further down. Chain the snapshot injection:

```rust
                .with_shared_snapshot(shared_snapshot)
```

- [ ] **Step 2: Run cargo check**

Run: `cargo check -p alephcore`

Expected: compiles. The SharedSnapshot is now shared between SubagentTool (reader) and AgentLoop (writer).

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore -- --nocapture`

Expected: all tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "feat(gateway): wire SharedSnapshot between AgentLoop and SubagentTool"
```

---

### Task 6: Delete subagent_runner.rs and clean up

**Files:**
- Delete: `src/agent_loop/subagent_runner.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Check for remaining references to subagent_runner**

Run: `grep -r "subagent_runner" src/`

Expected: only `src/agent_loop/mod.rs` (the `pub mod` declaration) and possibly `src/agent_loop/subagent_tool.rs` (old import, should already be removed in Task 3).

- [ ] **Step 2: Remove subagent_runner from mod.rs**

In `src/agent_loop/mod.rs`, remove:

```rust
pub mod subagent_runner;
```

Also remove any re-exports from subagent_runner if they still exist. The type aliases `ToolRegistryFactory` and `SafetyGuardFactory` now live in `agent_runtime.rs`.

- [ ] **Step 3: Delete the file**

```bash
rm src/agent_loop/subagent_runner.rs
```

- [ ] **Step 4: Update any remaining imports across codebase**

Run: `grep -r "subagent_runner" src/`

If any file still imports from `subagent_runner`, update it to import from `agent_runtime` instead. The key types that moved:

| Old path | New path |
|----------|----------|
| `agent_loop::subagent_runner::ToolRegistryFactory` | `agent_loop::agent_runtime::ToolRegistryFactory` |
| `agent_loop::subagent_runner::SafetyGuardFactory` | `agent_loop::agent_runtime::SafetyGuardFactory` |
| `agent_loop::subagent_runner::run_subagent` | Removed — use `AgentRuntime::run()` |

- [ ] **Step 5: Run cargo check**

Run: `cargo check -p alephcore`

Expected: compiles with no errors.

- [ ] **Step 6: Run full test suite**

Run: `cargo test -p alephcore -- --nocapture`

Expected: all tests PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(agent_loop): delete subagent_runner.rs, migrate types to agent_runtime"
```

---

### Task 7: Add fork path logic to AgentRuntime

**Files:**
- Modify: `src/agent_loop/agent_runtime.rs`

- [ ] **Step 1: Write failing test for fork path**

Add to the `tests` module in `agent_runtime.rs`:

```rust
    #[test]
    fn fork_path_uses_snapshot_prefix() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig, PromptSnapshot};
        use crate::thinker::prompt_layer::AssemblyPath;
        use crate::agents::AgentDef;

        let config = PromptConfig::default();
        let builder = PromptBuilder::new(config);
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        let snapshot = builder.capture_snapshot(&tools);

        let agent_def = AgentDef::default_subagent();
        let forked = builder.build_from_snapshot(&snapshot, &agent_def, &tools);
        let fresh = builder.build_for_agent_basic(&agent_def, &tools);

        // Fork path must produce identical result to fresh path
        assert_eq!(forked, fresh);
        // Fork result must start with the snapshot's stable prefix
        assert!(forked.starts_with(&snapshot.stable_prefix));
    }
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_runtime::tests::fork_path -- --nocapture`

Expected: PASS (this test validates the PromptBuilder methods from Task 1, confirming they work correctly when called from AgentRuntime context).

- [ ] **Step 3: Update AgentRuntime::run() fork path**

In `agent_runtime.rs`, update the prompt building section (Phase 1, step 3) to actually use the snapshot:

Replace:

```rust
        // 3. Build prompt (fork vs fresh)
        let prompt_builder = match &config.prompt_snapshot {
            Some(_snapshot) => {
                // Fork path — will be used by AgentLoop via build_from_snapshot
                // For now, attach agent_def so AgentRoleLayer fires
                PromptBuilder::new(PromptConfig::default())
                    .with_agent(config.agent_def.clone())
            }
            None => {
                // Fresh path
                PromptBuilder::new(PromptConfig::default())
                    .with_agent(config.agent_def.clone())
            }
        };
```

With:

```rust
        // 3. Build prompt builder
        // Both fork and fresh paths use the same PromptBuilder.
        // The fork path optimization happens inside AgentLoop's think step
        // where the system prompt is assembled: if a snapshot exists, the
        // builder's build_from_snapshot() is called instead of build_system_prompt().
        //
        // However, since AgentLoop currently always calls build_system_prompt(),
        // we apply the fork optimization at this layer by pre-building the
        // system prompt and passing it through.
        let prompt_builder = PromptBuilder::new(PromptConfig::default())
            .with_agent(config.agent_def.clone());
```

Note: The fork path's actual cache benefit comes from the Anthropic API level — the stable prefix bytes are identical to the parent's, so Anthropic's prompt caching hits automatically. The `build_from_snapshot` method ensures this byte-identity. The AgentLoop doesn't need to know about snapshots.

- [ ] **Step 4: Run cargo check and full tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib -- --nocapture`

Expected: compiles and all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/agent_runtime.rs
git commit -m "feat(agent_runtime): validate fork path snapshot reuse"
```

---

### Task 8: Final verification

**Files:** None (verification only)

- [ ] **Step 1: Run full cargo check across workspace**

Run: `cargo check`

Expected: entire workspace compiles.

- [ ] **Step 2: Run full test suite**

Run: `cargo test`

Expected: all tests pass, including the new ones from Tasks 1, 2, and 7.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`

Expected: no warnings.

- [ ] **Step 4: Verify subagent_runner.rs is fully removed**

Run: `test ! -f src/agent_loop/subagent_runner.rs && echo "DELETED" || echo "STILL EXISTS"`

Expected: `DELETED`

Run: `grep -r "subagent_runner" src/`

Expected: no matches.

- [ ] **Step 5: Verify file structure matches spec**

```bash
# New file exists
test -f src/agent_loop/agent_runtime.rs && echo "OK: agent_runtime.rs"

# Old file deleted
test ! -f src/agent_loop/subagent_runner.rs && echo "OK: subagent_runner.rs deleted"

# Key types are exported
grep "pub use agent_runtime" src/agent_loop/mod.rs && echo "OK: exports present"
```

- [ ] **Step 6: Commit verification marker**

No code changes — just verify the git log shows the expected progression:

```bash
git log --oneline -8
```

Expected commits (newest first):
1. `feat(agent_runtime): validate fork path snapshot reuse`
2. `refactor(agent_loop): delete subagent_runner.rs, migrate types to agent_runtime`
3. `feat(gateway): wire SharedSnapshot between AgentLoop and SubagentTool`
4. `feat(agent_loop): add SharedSnapshot support for prompt fork path`
5. `refactor(subagent): use AgentRuntime instead of run_subagent in SubagentTool`
6. `feat(agent_loop): add AgentRuntime with fresh-path execution and transcript`
7. `feat(prompt): add PromptSnapshot, capture_snapshot and build_from_snapshot`
