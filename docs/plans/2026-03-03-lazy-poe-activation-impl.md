# Lazy POE Activation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate lightweight POE validation into the normal channel message execution flow, so tool-using tasks get automatic hallucination detection, result validation, and retry — without overhead for simple conversations.

**Architecture:** LazyPoeEvaluator activates on first `Decision::UseTool`, tracks tool invocations via `LightManifest`, validates each step and completion with rule-based checks (no LLM), and retries via `StepDirective::ContinueWithHint` (max 2). Integrates through the existing `on_step_evaluate` callback hook.

**Tech Stack:** Rust, async-trait, tokio, serde_json

---

### Task 1: Create LightManifest and ToolInvocation structs

**Files:**
- Create: `core/src/poe/lazy_evaluator.rs`

**Step 1: Create the file with data structs**

```rust
// core/src/poe/lazy_evaluator.rs
//! Lazy POE Evaluator — lightweight rule-based validation for channel messages.
//!
//! Activates on first tool use, tracks invocations via LightManifest,
//! validates steps and completion without LLM overhead.

use serde::{Deserialize, Serialize};

/// Record of a single tool invocation during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub had_result: bool,
    pub result_non_empty: bool,
}

/// Lightweight success manifest — rule-based, no LLM generation needed.
///
/// Tracks tool usage and validates agent claims against actual invocations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightManifest {
    /// Original user request text
    original_query: String,
    /// Tools that have been invoked during execution
    tools_invoked: Vec<ToolInvocation>,
    /// Maximum retry attempts
    max_retries: u8,
    /// Current retry count
    retry_count: u8,
    /// Whether POE mode is active
    active: bool,
}

impl LightManifest {
    /// Create a new inactive manifest.
    pub fn new() -> Self {
        Self {
            original_query: String::new(),
            tools_invoked: Vec::new(),
            max_retries: 2,
            retry_count: 0,
            active: false,
        }
    }

    /// Activate the manifest with the user's original query.
    pub fn activate(&mut self, original_query: &str) {
        if !self.active {
            self.active = true;
            self.original_query = original_query.to_string();
        }
    }

    /// Whether the evaluator is currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Record a tool invocation.
    pub fn record_tool(&mut self, tool_name: &str, had_result: bool, result_non_empty: bool) {
        self.tools_invoked.push(ToolInvocation {
            tool_name: tool_name.to_string(),
            had_result,
            result_non_empty,
        });
    }

    /// Check if a specific tool was invoked.
    pub fn tool_was_invoked(&self, tool_name: &str) -> bool {
        self.tools_invoked.iter().any(|t| t.tool_name == tool_name)
    }

    /// Get list of invoked tool names.
    pub fn invoked_tool_names(&self) -> Vec<&str> {
        self.tools_invoked.iter().map(|t| t.tool_name.as_str()).collect()
    }

    /// Whether retry budget is available.
    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }

    /// Consume one retry attempt. Returns false if budget exhausted.
    pub fn consume_retry(&mut self) -> bool {
        if self.can_retry() {
            self.retry_count += 1;
            true
        } else {
            false
        }
    }

    /// Get the original query.
    pub fn original_query(&self) -> &str {
        &self.original_query
    }

    /// Get retry count.
    pub fn retry_count(&self) -> u8 {
        self.retry_count
    }
}

impl Default for LightManifest {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles (file is not yet wired into module tree, so it won't be compiled — need to add `mod lazy_evaluator;` to `core/src/poe/mod.rs` first)

**Step 3: Register the module in poe/mod.rs**

Modify: `core/src/poe/mod.rs` — add after the existing submodule declarations (near line 75):

```rust
pub mod lazy_evaluator;
```

And add a re-export near line 133 (where other interceptor types are re-exported):

```rust
pub use lazy_evaluator::{LazyPoeEvaluator, LightManifest};
```

(Note: `LazyPoeEvaluator` doesn't exist yet — we'll add it in Task 2. For now just export `LightManifest`.)

```rust
pub use lazy_evaluator::LightManifest;
```

**Step 4: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/poe/lazy_evaluator.rs core/src/poe/mod.rs
git commit -m "poe: add LightManifest for lazy POE activation"
```

---

### Task 2: Add LazyPoeEvaluator with step validation

**Files:**
- Modify: `core/src/poe/lazy_evaluator.rs`

**Step 1: Add LazyPoeEvaluator struct and step validation logic**

Append to `core/src/poe/lazy_evaluator.rs`:

```rust
use crate::agent_loop::decision::{Action, ActionResult};
use crate::agent_loop::state::{LoopState, LoopStep};
use crate::poe::interceptor::directive::StepDirective;
use tracing::{debug, warn};

/// Lazy POE evaluator — activates on first tool use, validates via rules.
///
/// Designed for channel messages (Telegram, Discord, etc.) where full POE
/// is too heavy but hallucination detection is still needed.
#[derive(Debug)]
pub struct LazyPoeEvaluator {
    manifest: tokio::sync::Mutex<LightManifest>,
}

impl LazyPoeEvaluator {
    /// Create a new inactive evaluator.
    pub fn new() -> Self {
        Self {
            manifest: tokio::sync::Mutex::new(LightManifest::new()),
        }
    }

    /// Activate on first tool use. Idempotent — only activates once.
    pub async fn activate(&self, original_query: &str) {
        let mut manifest = self.manifest.lock().await;
        if !manifest.is_active() {
            debug!(query = %original_query, "LazyPoeEvaluator activated");
            manifest.activate(original_query);
        }
    }

    /// Whether the evaluator is currently active.
    pub async fn is_active(&self) -> bool {
        self.manifest.lock().await.is_active()
    }

    /// Record a completed tool invocation and its result.
    pub async fn record_tool_result(&self, tool_name: &str, result: &ActionResult) {
        let (had_result, result_non_empty) = match result {
            ActionResult::ToolSuccess { output, .. } => {
                let non_empty = !output.is_null()
                    && output.as_str().map_or(true, |s| !s.trim().is_empty());
                (true, non_empty)
            }
            ActionResult::ToolError { .. } => (true, false),
            _ => (false, false),
        };

        let mut manifest = self.manifest.lock().await;
        manifest.record_tool(tool_name, had_result, result_non_empty);
    }

    /// Evaluate a step after tool execution (per-step validation).
    ///
    /// Rules:
    /// - ToolResultNonEmpty: tool returned non-empty, non-error result
    /// - ToolExecutionSuccess: tool didn't error out
    pub async fn evaluate_step(&self, step: &LoopStep, _state: &LoopState) -> StepDirective {
        let manifest = self.manifest.lock().await;
        if !manifest.is_active() {
            return StepDirective::Continue;
        }

        // Only evaluate tool call steps
        let tool_name = match &step.action {
            Action::ToolCall { tool_name, .. } => tool_name,
            _ => return StepDirective::Continue,
        };

        // Check tool result
        match &step.result {
            ActionResult::ToolError { error, retryable } => {
                if *retryable && manifest.can_retry() {
                    warn!(tool = %tool_name, error = %error, "Tool execution failed, suggesting retry");
                    return StepDirective::ContinueWithHint {
                        hint: format!(
                            "Tool '{}' execution failed with error: {}. Try a different approach or parameters.",
                            tool_name, error
                        ),
                    };
                }
            }
            ActionResult::ToolSuccess { output, .. } => {
                let is_empty = output.is_null()
                    || output.as_str().map_or(false, |s| s.trim().is_empty());
                if is_empty {
                    debug!(tool = %tool_name, "Tool returned empty result");
                    return StepDirective::ContinueWithHint {
                        hint: format!(
                            "Tool '{}' returned empty result. Consider using different parameters or an alternative tool.",
                            tool_name
                        ),
                    };
                }
            }
            _ => {}
        }

        StepDirective::Continue
    }
}

impl Default for LazyPoeEvaluator {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Update poe/mod.rs re-export**

Change the earlier `pub use lazy_evaluator::LightManifest;` to:

```rust
pub use lazy_evaluator::{LazyPoeEvaluator, LightManifest};
```

**Step 3: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/poe/lazy_evaluator.rs core/src/poe/mod.rs
git commit -m "poe: add LazyPoeEvaluator with per-step validation"
```

---

### Task 3: Add completion validation with hallucination detection

**Files:**
- Modify: `core/src/poe/lazy_evaluator.rs`

**Step 1: Add completion validation and hallucination detection methods to `LazyPoeEvaluator`**

Add these methods to the `impl LazyPoeEvaluator` block:

```rust
    /// Validate completion — checks agent claims against actual tool usage.
    ///
    /// Called when agent issues Decision::Complete. Returns None if valid,
    /// or Some(hint) if validation fails and retry is needed.
    ///
    /// Rules:
    /// - ToolActuallyUsed: at least one tool must have been called
    /// - NoHallucination: agent claims match tool invocation record
    /// - QueryRelevance: response addresses original query (keyword overlap)
    pub async fn validate_completion(&self, summary: &str) -> Option<String> {
        let mut manifest = self.manifest.lock().await;
        if !manifest.is_active() {
            return None;
        }

        // Rule 1: ToolActuallyUsed — if POE is active, tools should have been called
        if manifest.tools_invoked.is_empty() {
            if manifest.consume_retry() {
                warn!("Completion validation failed: no tools used despite POE activation");
                return Some(
                    "You indicated you would use tools but completed without calling any. \
                     Please actually use the relevant tools to fulfill the request."
                        .to_string(),
                );
            }
        }

        // Rule 2: NoHallucination — check for claims that don't match tool records
        if let Some(hint) = Self::detect_hallucination(summary, &manifest) {
            if manifest.consume_retry() {
                warn!(hint = %hint, "Hallucination detected in completion");
                return Some(hint);
            }
        }

        // Rule 3: QueryRelevance — basic keyword overlap check
        if let Some(hint) = Self::check_query_relevance(summary, &manifest) {
            if manifest.consume_retry() {
                debug!(hint = %hint, "Low query relevance in completion");
                return Some(hint);
            }
        }

        None
    }

    /// Detect hallucination by checking if agent claims match tool invocations.
    ///
    /// Heuristics (no LLM needed):
    /// - Claims PDF/file generated → pdf_generate/file tool must be in invoked list
    /// - Provides specific data/numbers → search/retrieve tool must have been called
    /// - References URLs → web_search/browse must have been called
    fn detect_hallucination(summary: &str, manifest: &LightManifest) -> Option<String> {
        let summary_lower = summary.to_lowercase();
        let invoked: Vec<&str> = manifest.invoked_tool_names();

        // Check PDF generation claims
        let pdf_claim_keywords = ["pdf已生成", "pdf 已生成", "已生成pdf", "pdf ready", "generated pdf", "pdf generated"];
        let pdf_tools = ["pdf_generate", "pdf"];
        if pdf_claim_keywords.iter().any(|kw| summary_lower.contains(kw)) {
            if !invoked.iter().any(|t| pdf_tools.iter().any(|pt| t.contains(pt))) {
                return Some(
                    "Your response claims a PDF was generated, but no PDF tool was called. \
                     Please actually call the PDF generation tool."
                        .to_string(),
                );
            }
        }

        // Check URL references without web search
        let url_indicators = ["http://", "https://", "www."];
        let web_tools = ["web_search", "browse", "search", "fetch"];
        if url_indicators.iter().any(|ind| summary_lower.contains(ind)) {
            if !invoked.iter().any(|t| web_tools.iter().any(|wt| t.contains(wt))) {
                return Some(
                    "Your response references URLs but no web search tool was called. \
                     URLs may be fabricated. Please use search tools to find real sources."
                        .to_string(),
                );
            }
        }

        None
    }

    /// Check if completion addresses the original query (basic keyword overlap).
    fn check_query_relevance(summary: &str, manifest: &LightManifest) -> Option<String> {
        let query = manifest.original_query();
        if query.is_empty() || summary.is_empty() {
            return None;
        }

        // Extract significant words from query (>= 2 chars, skip common words)
        let query_words: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() >= 2)
            .collect();

        if query_words.is_empty() {
            return None;
        }

        let summary_lower = summary.to_lowercase();
        let query_lower_words: Vec<String> = query_words.iter().map(|w| w.to_lowercase()).collect();

        let matching = query_lower_words
            .iter()
            .filter(|w| summary_lower.contains(w.as_str()))
            .count();

        let relevance = matching as f64 / query_lower_words.len() as f64;

        // If less than 15% keyword overlap, flag as potentially irrelevant
        if relevance < 0.15 && query_lower_words.len() >= 3 {
            return Some(format!(
                "Your response may not address the user's original question: '{}'. \
                 Please make sure your response directly answers what was asked.",
                query
            ));
        }

        None
    }
```

**Step 2: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 3: Commit**

```bash
git add core/src/poe/lazy_evaluator.rs
git commit -m "poe: add completion validation and hallucination detection"
```

---

### Task 4: Write unit tests for LightManifest

**Files:**
- Modify: `core/src/poe/lazy_evaluator.rs`

**Step 1: Add test module at the end of `lazy_evaluator.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_new_is_inactive() {
        let m = LightManifest::new();
        assert!(!m.is_active());
        assert!(m.tools_invoked.is_empty());
        assert_eq!(m.retry_count(), 0);
    }

    #[test]
    fn test_manifest_activate() {
        let mut m = LightManifest::new();
        m.activate("generate a report");
        assert!(m.is_active());
        assert_eq!(m.original_query(), "generate a report");
    }

    #[test]
    fn test_manifest_activate_idempotent() {
        let mut m = LightManifest::new();
        m.activate("first query");
        m.activate("second query");
        assert_eq!(m.original_query(), "first query");
    }

    #[test]
    fn test_manifest_record_tool() {
        let mut m = LightManifest::new();
        m.activate("test");
        m.record_tool("web_search", true, true);
        m.record_tool("pdf_generate", true, false);

        assert!(m.tool_was_invoked("web_search"));
        assert!(m.tool_was_invoked("pdf_generate"));
        assert!(!m.tool_was_invoked("unknown_tool"));
        assert_eq!(m.invoked_tool_names(), vec!["web_search", "pdf_generate"]);
    }

    #[test]
    fn test_manifest_retry_budget() {
        let mut m = LightManifest::new();
        assert!(m.can_retry());
        assert!(m.consume_retry());
        assert!(m.can_retry());
        assert!(m.consume_retry());
        assert!(!m.can_retry());
        assert!(!m.consume_retry());
        assert_eq!(m.retry_count(), 2);
    }

    #[test]
    fn test_hallucination_pdf_claim_without_tool() {
        let mut m = LightManifest::new();
        m.activate("生成比特币报告");
        // No tools invoked
        let result = LazyPoeEvaluator::detect_hallucination("PDF已生成，请查收", &m);
        assert!(result.is_some());
        assert!(result.unwrap().contains("PDF"));
    }

    #[test]
    fn test_hallucination_pdf_claim_with_tool() {
        let mut m = LightManifest::new();
        m.activate("生成比特币报告");
        m.record_tool("pdf_generate", true, true);
        let result = LazyPoeEvaluator::detect_hallucination("PDF已生成，请查收", &m);
        assert!(result.is_none());
    }

    #[test]
    fn test_hallucination_url_without_search() {
        let mut m = LightManifest::new();
        m.activate("find info");
        let result = LazyPoeEvaluator::detect_hallucination(
            "You can find it at https://example.com/data",
            &m,
        );
        assert!(result.is_some());
        assert!(result.unwrap().contains("URL"));
    }

    #[test]
    fn test_hallucination_url_with_search() {
        let mut m = LightManifest::new();
        m.activate("find info");
        m.record_tool("web_search", true, true);
        let result = LazyPoeEvaluator::detect_hallucination(
            "You can find it at https://example.com/data",
            &m,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_query_relevance_good_match() {
        let mut m = LightManifest::new();
        m.activate("比特币 交易 报告 分析");
        let result = LazyPoeEvaluator::check_query_relevance(
            "以下是比特币交易的分析报告...",
            &m,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_query_relevance_poor_match() {
        let mut m = LightManifest::new();
        m.activate("bitcoin trading report analysis overview");
        let result = LazyPoeEvaluator::check_query_relevance(
            "The weather today is sunny and warm",
            &m,
        );
        assert!(result.is_some());
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib poe::lazy_evaluator`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add core/src/poe/lazy_evaluator.rs
git commit -m "poe: add unit tests for LightManifest and hallucination detection"
```

---

### Task 5: Write unit tests for LazyPoeEvaluator

**Files:**
- Modify: `core/src/poe/lazy_evaluator.rs`

**Step 1: Add async tests to the test module**

```rust
    // --- Async tests for LazyPoeEvaluator ---

    #[tokio::test]
    async fn test_evaluator_inactive_by_default() {
        let eval = LazyPoeEvaluator::new();
        assert!(!eval.is_active().await);
    }

    #[tokio::test]
    async fn test_evaluator_activates_on_call() {
        let eval = LazyPoeEvaluator::new();
        eval.activate("test query").await;
        assert!(eval.is_active().await);
    }

    #[tokio::test]
    async fn test_evaluate_step_inactive_returns_continue() {
        let eval = LazyPoeEvaluator::new();
        let step = LoopStep {
            step_id: 0,
            observation_summary: String::new(),
            thinking: crate::agent_loop::state::Thinking {
                raw_content: String::new(),
                reasoning: None,
                decision: crate::agent_loop::decision::Decision::Complete {
                    summary: "done".to_string(),
                },
                tokens_used: None,
            },
            action: Action::ToolCall {
                tool_name: "test".to_string(),
                arguments: serde_json::Value::Null,
            },
            result: ActionResult::ToolSuccess {
                output: serde_json::Value::String("ok".to_string()),
                duration_ms: 0,
            },
            tokens_used: 0,
            duration_ms: 0,
        };
        let state = LoopState::new("test-session".to_string(), "query".to_string());
        let directive = eval.evaluate_step(&step, &state).await;
        assert!(matches!(directive, StepDirective::Continue));
    }

    #[tokio::test]
    async fn test_evaluate_step_empty_result_gives_hint() {
        let eval = LazyPoeEvaluator::new();
        eval.activate("test query").await;

        let step = LoopStep {
            step_id: 0,
            observation_summary: String::new(),
            thinking: crate::agent_loop::state::Thinking {
                raw_content: String::new(),
                reasoning: None,
                decision: crate::agent_loop::decision::Decision::UseTool {
                    tool_name: "search".to_string(),
                    arguments: serde_json::Value::Null,
                },
                tokens_used: None,
            },
            action: Action::ToolCall {
                tool_name: "search".to_string(),
                arguments: serde_json::Value::Null,
            },
            result: ActionResult::ToolSuccess {
                output: serde_json::json!(""),
                duration_ms: 0,
            },
            tokens_used: 0,
            duration_ms: 0,
        };
        let state = LoopState::new("test-session".to_string(), "query".to_string());
        let directive = eval.evaluate_step(&step, &state).await;
        assert!(matches!(directive, StepDirective::ContinueWithHint { .. }));
    }

    #[tokio::test]
    async fn test_validate_completion_no_tools_invoked() {
        let eval = LazyPoeEvaluator::new();
        eval.activate("generate a report").await;
        // No tools recorded
        let result = eval.validate_completion("Here is your report...").await;
        assert!(result.is_some());
        assert!(result.unwrap().contains("tools"));
    }

    #[tokio::test]
    async fn test_validate_completion_with_tools_ok() {
        let eval = LazyPoeEvaluator::new();
        eval.activate("search for bitcoin price").await;
        eval.record_tool_result(
            "web_search",
            &ActionResult::ToolSuccess {
                output: serde_json::json!({"price": "50000"}),
                duration_ms: 100,
            },
        )
        .await;
        let result = eval
            .validate_completion("Bitcoin price is currently $50,000 based on search results")
            .await;
        assert!(result.is_none());
    }
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib poe::lazy_evaluator`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add core/src/poe/lazy_evaluator.rs
git commit -m "poe: add async tests for LazyPoeEvaluator"
```

---

### Task 6: Integrate LazyPoeEvaluator into EventEmittingCallback

**Files:**
- Modify: `core/src/gateway/loop_callback_adapter.rs:27-64` (struct + constructor)

**Step 1: Add LazyPoeEvaluator field to EventEmittingCallback**

In `core/src/gateway/loop_callback_adapter.rs`, add import at top (after existing imports around line 10):

```rust
use crate::poe::lazy_evaluator::LazyPoeEvaluator;
```

Add field to `EventEmittingCallback` struct (after `user_question_tx` around line 41):

```rust
    /// Lazy POE evaluator (activates on first UseTool)
    lazy_poe: LazyPoeEvaluator,
```

**Step 2: Initialize LazyPoeEvaluator in constructors**

In `new()` (around line 53), add to Self:

```rust
        lazy_poe: LazyPoeEvaluator::new(),
```

In `with_user_channel()` (around line 69), add to Self:

```rust
        lazy_poe: LazyPoeEvaluator::new(),
```

**Step 3: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/gateway/loop_callback_adapter.rs
git commit -m "gateway: add LazyPoeEvaluator to EventEmittingCallback"
```

---

### Task 7: Wire LazyPoeEvaluator into on_step_evaluate and on_action_done

**Files:**
- Modify: `core/src/gateway/loop_callback_adapter.rs:204-241` (on_action_done)
- Modify: `core/src/gateway/loop_callback_adapter.rs` (on_step_evaluate — currently returns Continue)

**Step 1: Record tool results in on_action_done**

In `on_action_done()` (around line 204), after the existing `ToolEnd` event emission (after line 239), add:

```rust
        // Record tool result for lazy POE evaluation
        if let Action::ToolCall { tool_name, .. } = action {
            self.lazy_poe.record_tool_result(tool_name, result).await;
        }
```

**Step 2: Implement on_step_evaluate**

The current `on_step_evaluate` method in `EventEmittingCallback` returns `StepDirective::Continue` by default (from the trait). We need to override it explicitly. Find where other LoopCallback methods are implemented and add:

```rust
    async fn on_step_evaluate(
        &self,
        step: &crate::agent_loop::state::LoopStep,
        state: &crate::agent_loop::state::LoopState,
    ) -> crate::poe::interceptor::directive::StepDirective {
        self.lazy_poe.evaluate_step(step, state).await
    }
```

**Step 3: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/gateway/loop_callback_adapter.rs
git commit -m "gateway: wire LazyPoeEvaluator into step evaluation and tool recording"
```

---

### Task 8: Add tool activation hook in on_action_start

**Files:**
- Modify: `core/src/gateway/loop_callback_adapter.rs` (on_action_start)

**Step 1: Activate LazyPoeEvaluator when a tool call starts**

In `on_action_start()` — find the method in the LoopCallback implementation. When the action is a `ToolCall`, activate the evaluator with the original query from state.

However, `on_action_start` only receives `&Action`, not `&LoopState`. We need an alternative approach.

**Alternative: Store original_query in callback at construction time.**

Add a new field to `EventEmittingCallback`:

```rust
    /// Original user request for POE context
    original_query: String,
```

Update both constructors to accept and store it (default to empty):

```rust
    pub fn new(emitter: Arc<E>, run_id: String) -> Self {
        Self {
            // ... existing fields ...
            original_query: String::new(),
            lazy_poe: LazyPoeEvaluator::new(),
        }
    }
```

Add a setter:

```rust
    /// Set the original user query for POE context.
    pub fn set_original_query(&self, query: String) {
        // We need interior mutability — use std::sync::OnceLock or just store in lazy_poe
    }
```

**Better approach: Activate in on_action_start, use LoopState from on_loop_start.**

Add field:

```rust
    original_query: tokio::sync::Mutex<String>,
```

Override `on_loop_start`:

```rust
    async fn on_loop_start(&self, state: &LoopState) {
        // Store original query for POE
        *self.original_query.lock().await = state.original_request.clone();
        // ... existing on_loop_start logic ...
    }
```

Then in `on_action_start`, when action is `ToolCall`:

```rust
    async fn on_action_start(&self, action: &Action) {
        if let Action::ToolCall { tool_name, .. } = action {
            // Activate lazy POE on first tool use
            let query = self.original_query.lock().await.clone();
            if !query.is_empty() {
                self.lazy_poe.activate(&query).await;
            }

            // ... existing on_action_start logic (ToolStart event emission) ...
        }
    }
```

**Step 2: Update constructors to initialize `original_query`**

```rust
    original_query: tokio::sync::Mutex::new(String::new()),
```

**Step 3: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/gateway/loop_callback_adapter.rs
git commit -m "gateway: activate LazyPoeEvaluator on first tool call via on_action_start"
```

---

### Task 9: Add completion validation to Decision::Complete path

**Files:**
- Modify: `core/src/agent_loop/callback.rs:21` (add `on_validate_completion` to trait)
- Modify: `core/src/gateway/loop_callback_adapter.rs` (implement `on_validate_completion`)
- Modify: `core/src/agent_loop/agent_loop.rs:510-521` (hook into Decision::Complete)

**Step 1: Add `on_validate_completion` method to LoopCallback trait**

In `core/src/agent_loop/callback.rs`, add a new method to the `LoopCallback` trait (after `on_step_evaluate`, around line 54):

```rust
    /// Called when the agent issues Decision::Complete.
    /// Returns None to accept, or Some(hint) to retry with feedback.
    /// Default: accept all completions (backward compatible).
    async fn on_validate_completion(
        &self,
        _summary: &str,
        _state: &super::state::LoopState,
    ) -> Option<String> {
        None
    }
```

**Step 2: Implement in EventEmittingCallback**

In `core/src/gateway/loop_callback_adapter.rs`, add implementation:

```rust
    async fn on_validate_completion(
        &self,
        summary: &str,
        _state: &crate::agent_loop::state::LoopState,
    ) -> Option<String> {
        self.lazy_poe.validate_completion(summary).await
    }
```

**Step 3: Hook into Decision::Complete in agent_loop.rs**

In `core/src/agent_loop/agent_loop.rs`, around lines 510-521, change the `Decision::Complete` handling:

Before:
```rust
Decision::Complete { summary } => {
    callback.on_complete(summary).await;
    self.compaction_trigger
        .emit_loop_stop(StopReason::Completed)
        .await;
    return LoopResult::Completed {
        summary: summary.clone(),
        steps: state.step_count,
        total_tokens: state.total_tokens,
    };
}
```

After:
```rust
Decision::Complete { summary } => {
    // Lazy POE: validate completion before accepting
    if let Some(hint) = callback.on_validate_completion(summary, &state).await {
        // Validation failed — inject hint and retry
        state.set_poe_hint(hint);
        continue;
    }

    callback.on_complete(summary).await;
    self.compaction_trigger
        .emit_loop_stop(StopReason::Completed)
        .await;
    return LoopResult::Completed {
        summary: summary.clone(),
        steps: state.step_count,
        total_tokens: state.total_tokens,
    };
}
```

**Step 4: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests still pass, plus the new lazy_evaluator tests.

**Step 6: Commit**

```bash
git add core/src/agent_loop/callback.rs core/src/agent_loop/agent_loop.rs core/src/gateway/loop_callback_adapter.rs
git commit -m "poe: wire completion validation into Decision::Complete path"
```

---

### Task 10: Update NoOpLoopCallback and other implementations

**Files:**
- Modify: `core/src/agent_loop/callback.rs` (NoOpLoopCallback, LoggingCallback, CollectingCallback)

**Step 1: Verify default trait method suffices**

Since `on_validate_completion` has a default implementation returning `None`, the existing implementations (`NoOpLoopCallback`, `LoggingCallback`, `CollectingCallback`) don't need changes. Verify by checking that all compile.

**Step 2: Check PoeLoopCallback implementation**

Read `core/src/poe/interceptor/callback.rs` and ensure `PoeLoopCallback` either:
- Inherits the default `on_validate_completion` (returns None), OR
- Already has its own validation path that won't conflict.

If PoeLoopCallback has its own `on_step_evaluate` override, that's fine — it handles the full POE case separately.

**Step 3: Run `cargo check -p alephcore` to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

**Step 5: Commit (if any changes were needed)**

```bash
git add core/src/agent_loop/callback.rs
git commit -m "poe: ensure backward compatibility with all LoopCallback implementations"
```

---

### Task 11: Integration test — full lazy POE flow

**Files:**
- Modify: `core/src/poe/lazy_evaluator.rs` (add integration test)

**Step 1: Write integration test simulating full flow**

Add to the test module in `lazy_evaluator.rs`:

```rust
    #[tokio::test]
    async fn test_full_lazy_poe_flow() {
        // Simulate: user asks for research, agent uses tools, completes
        let eval = LazyPoeEvaluator::new();

        // 1. Agent loop starts — evaluator inactive
        assert!(!eval.is_active().await);

        // 2. Agent decides to use tool → activate
        eval.activate("帮我查一下比特币最新价格").await;
        assert!(eval.is_active().await);

        // 3. Tool executes with good result
        eval.record_tool_result(
            "web_search",
            &ActionResult::ToolSuccess {
                output: serde_json::json!({"price": "67000", "source": "coinbase"}),
                duration_ms: 500,
            },
        )
        .await;

        // 4. Step evaluation — should continue (good result)
        let step = LoopStep {
            step_id: 0,
            observation_summary: String::new(),
            thinking: crate::agent_loop::state::Thinking {
                raw_content: String::new(),
                reasoning: None,
                decision: crate::agent_loop::decision::Decision::UseTool {
                    tool_name: "web_search".to_string(),
                    arguments: serde_json::json!({"query": "bitcoin price"}),
                },
                tokens_used: None,
            },
            action: Action::ToolCall {
                tool_name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "bitcoin price"}),
            },
            result: ActionResult::ToolSuccess {
                output: serde_json::json!({"price": "67000"}),
                duration_ms: 500,
            },
            tokens_used: 0,
            duration_ms: 500,
        };
        let state = LoopState::new("test".to_string(), "帮我查一下比特币最新价格".to_string());
        let directive = eval.evaluate_step(&step, &state).await;
        assert!(matches!(directive, StepDirective::Continue));

        // 5. Completion with valid summary — should pass
        let result = eval
            .validate_completion("根据搜索结果，比特币当前价格为 $67,000")
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_hallucination_retry_flow() {
        let eval = LazyPoeEvaluator::new();
        eval.activate("生成一份PDF报告").await;

        // Agent completes without calling pdf_generate → hallucination
        let result = eval.validate_completion("PDF已生成，请查收").await;
        assert!(result.is_some()); // First retry

        // Second attempt, still no tool
        let result = eval.validate_completion("PDF已生成").await;
        assert!(result.is_some()); // Second retry

        // Third attempt — budget exhausted, accepted
        let result = eval.validate_completion("PDF已生成").await;
        assert!(result.is_none()); // Accepted (best effort)
    }
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib poe::lazy_evaluator`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add core/src/poe/lazy_evaluator.rs
git commit -m "poe: add integration tests for lazy POE flow"
```

---

### Task 12: Final compilation check and cleanup

**Files:**
- All modified files

**Step 1: Run full build check**

Run: `cargo check -p alephcore`
Expected: PASS with zero warnings related to our changes

**Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: No new warnings from our changes (pre-existing warnings may exist)

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

**Step 4: Final commit (if any clippy fixes needed)**

```bash
git add -A
git commit -m "poe: lazy POE activation — clippy cleanup"
```

---

## Summary of All Changes

| File | Action | Purpose |
|------|--------|---------|
| `core/src/poe/lazy_evaluator.rs` | CREATE | LightManifest, LazyPoeEvaluator, validation rules, hallucination detection, tests |
| `core/src/poe/mod.rs` | MODIFY | Register `lazy_evaluator` module, add re-exports |
| `core/src/gateway/loop_callback_adapter.rs` | MODIFY | Add LazyPoeEvaluator field, wire into on_action_start/on_action_done/on_step_evaluate/on_validate_completion |
| `core/src/agent_loop/callback.rs` | MODIFY | Add `on_validate_completion` trait method with default impl |
| `core/src/agent_loop/agent_loop.rs` | MODIFY | Hook completion validation into Decision::Complete path |

## Files NOT Modified

- `InboundRouter` — execution path unchanged
- `ExecutionAdapter` trait — interface unchanged
- `Thinker / PromptPipeline` — system prompt unchanged
- `PoeManager` — independent system, not affected
- `SuccessManifest / ManifestBuilder` — not used by lazy POE
- `ExecutionEngine` — callback construction unchanged (original_query captured via on_loop_start)
