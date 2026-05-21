# Context Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-tier context pressure sensing, compaction circuit breaker, and per-turn diminishing returns detection to the agent loop.

**Architecture:** New `context_budget.rs` module in `agent_loop/` encapsulates all three features behind a `ContextBudget` struct that returns `LoopDirective` enums. The main loop in `loop_core.rs` replaces manual compaction calls with directive-driven control flow. `ToolCompactorConfig` is removed; its fields migrate into `ContextBudgetConfig`.

**Tech Stack:** Rust, `std::collections::VecDeque`, existing `estimate_tokens` from `session_compactor::context_window`

**Spec:** `docs/superpowers/specs/2026-03-31-context-budget-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/agent_loop/context_budget.rs` | ContextPressure, CircuitBreaker, DiminishingReturnsDetector, ContextBudget, LoopDirective |
| Modify | `src/agent_loop/mod.rs` | Add `pub mod context_budget;` export |
| Modify | `src/agent_loop/loop_core.rs` | Replace `ToolCompactorConfig` with `ContextBudget`, directive-driven loop |
| Modify | `src/gateway/execution_engine/run_loop.rs` | Build `ContextBudgetConfig` instead of `ToolCompactorConfig` |

---

### Task 1: Create `context_budget.rs` — Types and ContextPressure

**Files:**
- Create: `src/agent_loop/context_budget.rs`

- [ ] **Step 1: Write failing tests for ContextPressure evaluation**

```rust
// At bottom of src/agent_loop/context_budget.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_normal_when_under_warning() {
        assert_eq!(
            evaluate_pressure(5000, 10000, 0.70, 0.85),
            ContextPressure::Normal
        );
    }

    #[test]
    fn pressure_warning_at_boundary() {
        // 70% of 10000 = 7000
        assert_eq!(
            evaluate_pressure(7000, 10000, 0.70, 0.85),
            ContextPressure::Warning
        );
    }

    #[test]
    fn pressure_warning_between_thresholds() {
        assert_eq!(
            evaluate_pressure(8000, 10000, 0.70, 0.85),
            ContextPressure::Warning
        );
    }

    #[test]
    fn pressure_critical_at_boundary() {
        // 85% of 10000 = 8500
        assert_eq!(
            evaluate_pressure(8500, 10000, 0.70, 0.85),
            ContextPressure::Critical
        );
    }

    #[test]
    fn pressure_critical_above() {
        assert_eq!(
            evaluate_pressure(9500, 10000, 0.70, 0.85),
            ContextPressure::Critical
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib context_budget -- --nocapture 2>&1 | head -30`
Expected: FAIL — module and function not defined

- [ ] **Step 3: Write types and `evaluate_pressure` function**

```rust
//! Context budget management for the agent loop.
//!
//! Provides multi-tier pressure sensing, compaction circuit breaking,
//! and per-turn diminishing returns detection. The [`ContextBudget`]
//! orchestrator returns a [`LoopDirective`] each turn to guide the
//! main loop's control flow.

use std::collections::VecDeque;

use crate::memory::session_compactor::context_window::estimate_tokens;
use crate::providers::message::UnifiedMessage;

// ---------------------------------------------------------------------------
// ContextPressure
// ---------------------------------------------------------------------------

/// Three-tier context pressure level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPressure {
    /// < warning_threshold — no intervention needed.
    Normal,
    /// warning..critical — force tool result compaction.
    Warning,
    /// >= critical_threshold — block new tool calls, request final reply.
    Critical,
}

/// Evaluate pressure from estimated token usage.
fn evaluate_pressure(
    used_tokens: usize,
    budget: usize,
    warning_threshold: f64,
    critical_threshold: f64,
) -> ContextPressure {
    if budget == 0 {
        return ContextPressure::Critical;
    }
    let ratio = used_tokens as f64 / budget as f64;
    if ratio >= critical_threshold {
        ContextPressure::Critical
    } else if ratio >= warning_threshold {
        ContextPressure::Warning
    } else {
        ContextPressure::Normal
    }
}

// ---------------------------------------------------------------------------
// LoopDirective
// ---------------------------------------------------------------------------

/// Control signal returned by [`ContextBudget`] after each evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDirective {
    /// Normal operation — proceed with tool execution.
    Continue,
    /// Trigger tool result compaction, then continue.
    CompactAndContinue,
    /// Context critical — inject system notice, skip tool execution,
    /// let LLM produce a final reply.
    FinalReply,
    /// Diminishing returns detected — inject summary prompt and stop.
    StopDiminishing,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib context_budget -- --nocapture 2>&1 | head -30`
Expected: All 5 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/context_budget.rs
git commit -m "feat(agent_loop): add ContextPressure and LoopDirective types"
```

---

### Task 2: Add CompactionCircuitBreaker

**Files:**
- Modify: `src/agent_loop/context_budget.rs`

- [ ] **Step 1: Write failing tests for circuit breaker**

```rust
// Add to existing #[cfg(test)] mod tests in context_budget.rs

    #[test]
    fn circuit_breaker_closed_initially() {
        let cb = CompactionCircuitBreaker::new(3);
        assert!(!cb.is_tripped());
    }

    #[test]
    fn circuit_breaker_trips_after_max_failures() {
        let mut cb = CompactionCircuitBreaker::new(3);
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_tripped());
        cb.record_failure();
        assert!(cb.is_tripped());
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let mut cb = CompactionCircuitBreaker::new(3);
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert!(!cb.is_tripped());
        assert_eq!(cb.consecutive_failures, 0);
    }

    #[test]
    fn circuit_breaker_stays_tripped_after_more_failures() {
        let mut cb = CompactionCircuitBreaker::new(2);
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_tripped());
        cb.record_failure(); // still tripped
        assert!(cb.is_tripped());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib context_budget::tests::circuit_breaker -- --nocapture 2>&1 | head -20`
Expected: FAIL — `CompactionCircuitBreaker` not found

- [ ] **Step 3: Implement CompactionCircuitBreaker**

Add after `LoopDirective` in `context_budget.rs`:

```rust
// ---------------------------------------------------------------------------
// CompactionCircuitBreaker
// ---------------------------------------------------------------------------

/// Prevents repeated LLM summarization calls when compaction keeps failing.
///
/// Two-state model: Closed (normal) and Open (tripped — use deterministic
/// fallback only). Resets on success or new `ContextBudget` instance.
#[derive(Debug)]
struct CompactionCircuitBreaker {
    consecutive_failures: u32,
    max_failures: u32,
    tripped: bool,
}

impl CompactionCircuitBreaker {
    fn new(max_failures: u32) -> Self {
        Self {
            consecutive_failures: 0,
            max_failures,
            tripped: false,
        }
    }

    fn is_tripped(&self) -> bool {
        self.tripped
    }

    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.max_failures {
            self.tripped = true;
            tracing::warn!(
                target: "context_budget",
                failures = self.consecutive_failures,
                "Circuit breaker tripped — switching to deterministic compaction"
            );
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.tripped = false;
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib context_budget::tests::circuit_breaker -- --nocapture 2>&1 | head -20`
Expected: All 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/context_budget.rs
git commit -m "feat(agent_loop): add CompactionCircuitBreaker"
```

---

### Task 3: Add DiminishingReturnsDetector

**Files:**
- Modify: `src/agent_loop/context_budget.rs`

- [ ] **Step 1: Write failing tests**

```rust
// Add to existing tests module

    #[test]
    fn diminishing_not_detected_when_window_not_full() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        d.record(TurnMetrics { output_tokens: 10, tool_calls: 1, productive: false });
        assert!(!d.is_diminishing());
    }

    #[test]
    fn diminishing_detected_when_all_turns_low_and_unproductive() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        for _ in 0..4 {
            d.record(TurnMetrics { output_tokens: 100, tool_calls: 1, productive: false });
        }
        assert!(d.is_diminishing());
    }

    #[test]
    fn diminishing_not_detected_when_productive() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        for i in 0..4 {
            d.record(TurnMetrics {
                output_tokens: 100,
                tool_calls: 1,
                productive: i % 2 == 0, // 50% productive — below 75% threshold
            });
        }
        assert!(!d.is_diminishing());
    }

    #[test]
    fn diminishing_not_detected_when_high_output() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        for _ in 0..4 {
            d.record(TurnMetrics { output_tokens: 1000, tool_calls: 1, productive: false });
        }
        // Average 1000 > threshold 500
        assert!(!d.is_diminishing());
    }

    #[test]
    fn diminishing_sliding_window_evicts_old() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        // Fill with bad turns
        for _ in 0..4 {
            d.record(TurnMetrics { output_tokens: 10, tool_calls: 1, productive: false });
        }
        assert!(d.is_diminishing());
        // Push a good turn — evicts oldest bad turn
        d.record(TurnMetrics { output_tokens: 3000, tool_calls: 1, productive: true });
        assert!(!d.is_diminishing());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib context_budget::tests::diminishing -- --nocapture 2>&1 | head -20`
Expected: FAIL — `DiminishingReturnsDetector` and `TurnMetrics` not found

- [ ] **Step 3: Implement TurnMetrics and DiminishingReturnsDetector**

Add after `CompactionCircuitBreaker` in `context_budget.rs`:

```rust
// ---------------------------------------------------------------------------
// TurnMetrics
// ---------------------------------------------------------------------------

/// Token production metrics for a single agent loop iteration.
#[derive(Debug, Clone)]
pub struct TurnMetrics {
    /// LLM output tokens this turn.
    pub output_tokens: usize,
    /// Number of tool calls executed this turn.
    pub tool_calls: usize,
    /// Whether this turn produced meaningful work (successful tool or useful text).
    pub productive: bool,
}

// ---------------------------------------------------------------------------
// DiminishingReturnsDetector
// ---------------------------------------------------------------------------

/// Sliding-window detector for agent loop stagnation.
///
/// Triggers when the recent N turns are both low-output AND mostly unproductive.
#[derive(Debug)]
struct DiminishingReturnsDetector {
    window: VecDeque<TurnMetrics>,
    window_size: usize,
    low_output_threshold: usize,
}

impl DiminishingReturnsDetector {
    fn new(window_size: usize, low_output_threshold: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size),
            window_size,
            low_output_threshold,
        }
    }

    fn record(&mut self, metrics: TurnMetrics) {
        if self.window.len() >= self.window_size {
            self.window.pop_front();
        }
        self.window.push_back(metrics);
    }

    fn is_diminishing(&self) -> bool {
        if self.window.len() < self.window_size {
            return false;
        }

        let total_output: usize = self.window.iter().map(|m| m.output_tokens).sum();
        let avg_output = total_output / self.window.len();

        let unproductive_count = self.window.iter().filter(|m| !m.productive).count();
        let unproductive_ratio = unproductive_count as f64 / self.window.len() as f64;

        avg_output < self.low_output_threshold && unproductive_ratio >= 0.75
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib context_budget::tests::diminishing -- --nocapture 2>&1 | head -20`
Expected: All 5 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/context_budget.rs
git commit -m "feat(agent_loop): add DiminishingReturnsDetector with sliding window"
```

---

### Task 4: Implement ContextBudget orchestrator

**Files:**
- Modify: `src/agent_loop/context_budget.rs`

- [ ] **Step 1: Write failing tests for ContextBudget**

```rust
// Add to existing tests module

    fn make_config() -> ContextBudgetConfig {
        ContextBudgetConfig {
            token_budget: 10000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0, // 1 char = 1 token for easy test math
            fresh_tail_count: 2,
            circuit_breaker_max: 3,
            diminishing_window: 4,
            diminishing_threshold: 500,
        }
    }

    fn make_messages(total_chars: usize) -> Vec<UnifiedMessage> {
        // Create messages that total approximately `total_chars` characters
        let msg = "x".repeat(total_chars);
        vec![UnifiedMessage::user(msg)]
    }

    #[test]
    fn before_turn_returns_continue_when_normal() {
        let mut budget = ContextBudget::new(&make_config());
        let msgs = make_messages(5000); // 50% of 10000
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::Continue);
        assert_eq!(budget.pressure(), ContextPressure::Normal);
    }

    #[test]
    fn before_turn_returns_compact_when_warning() {
        let mut budget = ContextBudget::new(&make_config());
        let msgs = make_messages(7500); // 75% of 10000
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::CompactAndContinue);
        assert_eq!(budget.pressure(), ContextPressure::Warning);
    }

    #[test]
    fn before_turn_returns_final_reply_when_critical() {
        let mut budget = ContextBudget::new(&make_config());
        let msgs = make_messages(9000); // 90% of 10000
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::FinalReply);
        assert_eq!(budget.pressure(), ContextPressure::Critical);
    }

    #[test]
    fn after_turn_returns_stop_when_diminishing() {
        let mut budget = ContextBudget::new(&make_config());
        for _ in 0..4 {
            let d = budget.after_turn(TurnMetrics {
                output_tokens: 100,
                tool_calls: 1,
                productive: false,
            });
            if d == LoopDirective::StopDiminishing {
                // Expected on 4th call
                return;
            }
        }
        panic!("Expected StopDiminishing after 4 unproductive turns");
    }

    #[test]
    fn circuit_breaker_exposed_via_budget() {
        let mut budget = ContextBudget::new(&make_config());
        assert!(!budget.is_compaction_tripped());
        budget.on_compaction_failure();
        budget.on_compaction_failure();
        budget.on_compaction_failure();
        assert!(budget.is_compaction_tripped());
        budget.on_compaction_success();
        assert!(!budget.is_compaction_tripped());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib context_budget::tests::before_turn -- --nocapture 2>&1 | head -20`
Expected: FAIL — `ContextBudget` and `ContextBudgetConfig` not found

- [ ] **Step 3: Implement ContextBudgetConfig and ContextBudget**

Add after `DiminishingReturnsDetector` in `context_budget.rs`:

```rust
// ---------------------------------------------------------------------------
// ContextBudgetConfig
// ---------------------------------------------------------------------------

/// Configuration for the [`ContextBudget`] system.
#[derive(Debug, Clone)]
pub struct ContextBudgetConfig {
    /// Total token budget for the model context window.
    pub token_budget: u64,
    /// Fraction at which Warning pressure activates (default 0.70).
    pub warning_threshold: f64,
    /// Fraction at which Critical pressure activates (default 0.85).
    pub critical_threshold: f64,
    /// Characters-per-token ratio for estimation (default 3.5).
    pub token_estimate_ratio: f64,
    /// Number of recent messages exempt from compaction (default 6).
    pub fresh_tail_count: usize,
    /// Consecutive compaction failures before circuit breaker trips (default 3).
    pub circuit_breaker_max: u32,
    /// Sliding window size for diminishing returns detection (default 4).
    pub diminishing_window: usize,
    /// Average output tokens below which a turn is "low output" (default 500).
    pub diminishing_threshold: usize,
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            token_budget: 500_000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
            circuit_breaker_max: 3,
            diminishing_window: 4,
            diminishing_threshold: 500,
        }
    }
}

// ---------------------------------------------------------------------------
// ContextBudget
// ---------------------------------------------------------------------------

/// Unified context budget orchestrator for the agent loop.
///
/// Combines pressure sensing, circuit breaking, and diminishing returns
/// detection. Call [`before_turn`] before each LLM invocation and
/// [`after_turn`] after each iteration to get a [`LoopDirective`].
#[derive(Debug)]
pub struct ContextBudget {
    token_budget: usize,
    warning_threshold: f64,
    critical_threshold: f64,
    token_estimate_ratio: f64,
    fresh_tail_count: usize,
    circuit_breaker: CompactionCircuitBreaker,
    diminishing: DiminishingReturnsDetector,
    pressure: ContextPressure,
}

impl ContextBudget {
    /// Create a new budget from configuration.
    pub fn new(config: &ContextBudgetConfig) -> Self {
        Self {
            token_budget: config.token_budget as usize,
            warning_threshold: config.warning_threshold,
            critical_threshold: config.critical_threshold,
            token_estimate_ratio: config.token_estimate_ratio,
            fresh_tail_count: config.fresh_tail_count,
            circuit_breaker: CompactionCircuitBreaker::new(config.circuit_breaker_max),
            diminishing: DiminishingReturnsDetector::new(
                config.diminishing_window,
                config.diminishing_threshold,
            ),
            pressure: ContextPressure::Normal,
        }
    }

    /// Assess context pressure before a LLM call and return a directive.
    ///
    /// Estimates total token usage from messages + system prompt + tool
    /// definitions, updates the pressure level, and returns the appropriate
    /// control signal.
    pub fn before_turn(
        &mut self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[crate::agent_loop::tool::ToolDefinition],
    ) -> LoopDirective {
        let ratio = self.token_estimate_ratio;
        let msg_tokens: usize = messages
            .iter()
            .map(|m| estimate_tokens(&m.text_content(), ratio))
            .sum();
        let prompt_tokens = estimate_tokens(system_prompt, ratio);
        let tool_tokens: usize = tool_defs
            .iter()
            .map(|td| {
                estimate_tokens(&td.name, ratio)
                    + estimate_tokens(&td.description, ratio)
                    + estimate_tokens(&td.parameters.to_string(), ratio)
            })
            .sum();
        let total = msg_tokens + prompt_tokens + tool_tokens;

        self.pressure = evaluate_pressure(
            total,
            self.token_budget,
            self.warning_threshold,
            self.critical_threshold,
        );

        match self.pressure {
            ContextPressure::Normal => LoopDirective::Continue,
            ContextPressure::Warning => {
                tracing::warn!(
                    target: "context_budget",
                    total_tokens = total,
                    budget = self.token_budget,
                    "Context pressure: Warning — triggering compaction"
                );
                LoopDirective::CompactAndContinue
            }
            ContextPressure::Critical => {
                tracing::warn!(
                    target: "context_budget",
                    total_tokens = total,
                    budget = self.token_budget,
                    "Context pressure: Critical — requesting final reply"
                );
                LoopDirective::FinalReply
            }
        }
    }

    /// Record turn metrics and check for diminishing returns.
    pub fn after_turn(&mut self, metrics: TurnMetrics) -> LoopDirective {
        self.diminishing.record(metrics);
        if self.diminishing.is_diminishing() {
            tracing::warn!(
                target: "context_budget",
                "Diminishing returns detected — recommending stop"
            );
            LoopDirective::StopDiminishing
        } else {
            LoopDirective::Continue
        }
    }

    /// Notify that a compaction succeeded (resets circuit breaker).
    pub fn on_compaction_success(&mut self) {
        self.circuit_breaker.record_success();
    }

    /// Notify that a compaction failed (increments circuit breaker).
    pub fn on_compaction_failure(&mut self) {
        self.circuit_breaker.record_failure();
    }

    /// Whether the circuit breaker has tripped.
    pub fn is_compaction_tripped(&self) -> bool {
        self.circuit_breaker.is_tripped()
    }

    /// Current pressure level.
    pub fn pressure(&self) -> ContextPressure {
        self.pressure
    }

    /// Token estimate ratio (needed by callers for compaction).
    pub fn token_estimate_ratio(&self) -> f64 {
        self.token_estimate_ratio
    }

    /// Fresh tail count (needed by callers for compaction).
    pub fn fresh_tail_count(&self) -> usize {
        self.fresh_tail_count
    }

    /// Token budget (needed by callers for compaction).
    pub fn token_budget(&self) -> u64 {
        self.token_budget as u64
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib context_budget -- --nocapture 2>&1 | head -40`
Expected: All 15 tests PASS (5 pressure + 4 breaker + 5 diminishing + 5 budget + helper tests might overlap in naming — just check count)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/context_budget.rs
git commit -m "feat(agent_loop): add ContextBudget orchestrator"
```

---

### Task 5: Wire `context_budget` module into `agent_loop/mod.rs`

**Files:**
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Add module declaration and re-exports**

In `src/agent_loop/mod.rs`, add the module declaration and update exports:

```rust
// Add after line 8 (after `mod safety;`)
pub mod context_budget;
```

Update the re-export line (currently line 21):

```rust
// Change from:
pub use loop_core::{
    AgentLoop, LoopCallback, LoopConfig, LoopProvider, LoopRunResult, ToolCompactorConfig,
};

// Change to:
pub use loop_core::{
    AgentLoop, LoopCallback, LoopConfig, LoopProvider, LoopRunResult,
};
pub use context_budget::{ContextBudget, ContextBudgetConfig, ContextPressure, LoopDirective, TurnMetrics};
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: Compilation errors in `loop_core.rs` and `run_loop.rs` referencing removed `ToolCompactorConfig` — this is expected and will be fixed in Tasks 6-7.

- [ ] **Step 3: Commit**

```bash
git add src/agent_loop/mod.rs
git commit -m "feat(agent_loop): export context_budget module"
```

---

### Task 6: Integrate ContextBudget into `loop_core.rs`

**Files:**
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Replace ToolCompactorConfig with ContextBudget in AgentLoop struct**

Remove the `ToolCompactorConfig` struct definition (lines 153-168) and update `AgentLoop`:

```rust
// DELETE the entire ToolCompactorConfig struct and its comment block (lines 149-168)

// In AgentLoop struct (around line 262), change:
//     tool_compactor_config: Option<ToolCompactorConfig>,
// to:
    context_budget: Option<super::context_budget::ContextBudget>,
```

- [ ] **Step 2: Update AgentLoop constructor and builder methods**

Replace `with_tool_compactor_config`:

```rust
    // Change from:
    //     pub fn with_tool_compactor_config(mut self, cfg: Option<ToolCompactorConfig>) -> Self {
    //         self.tool_compactor_config = cfg;
    //         self
    //     }
    // To:

    /// Attach a [`ContextBudget`] for pressure sensing and budget tracking.
    pub fn with_context_budget(
        mut self,
        budget: Option<super::context_budget::ContextBudget>,
    ) -> Self {
        self.context_budget = budget;
        self
    }
```

Update `new()` to initialize `context_budget: None` instead of `tool_compactor_config: None`.

- [ ] **Step 3: Add system notice constants**

Add after the existing `TRUNCATION_NOTICE` constant (line 28):

```rust
const CRITICAL_CONTEXT_NOTICE: &str =
    "[SYSTEM] Context window is critically full. You MUST respond directly to the user now. \
     Do NOT call any tools. Summarize your progress and provide the best answer you can \
     with the information you have.";

const DIMINISHING_RETURNS_NOTICE: &str =
    "[SYSTEM] Your recent iterations have produced minimal progress. Summarize: \
     (1) what you accomplished, (2) what you tried that didn't work, \
     (3) what the user should do next. Then stop.";
```

- [ ] **Step 4: Replace the compaction block in the main loop**

In `run_with_history_messages`, replace lines 384-411 (the manual compact + enforce block) with:

```rust
            // --- Context budget evaluation ---
            let mut budget_directive = super::context_budget::LoopDirective::Continue;
            if let Some(ref mut ctx_budget) = self.context_budget {
                budget_directive = ctx_budget.before_turn(&messages, &system_prompt, &tool_defs);

                match budget_directive {
                    super::context_budget::LoopDirective::CompactAndContinue => {
                        crate::memory::session_compactor::tool_compactor::compact_if_needed(
                            &mut messages,
                            ctx_budget.token_budget(),
                            ctx_budget.token_estimate_ratio() * 0.85 / 0.70, // use warning threshold as compaction trigger
                            ctx_budget.token_estimate_ratio(),
                            ctx_budget.fresh_tail_count(),
                        );
                    }
                    super::context_budget::LoopDirective::FinalReply => {
                        // Force compaction first as last-ditch effort
                        crate::memory::session_compactor::tool_compactor::compact_if_needed(
                            &mut messages,
                            ctx_budget.token_budget(),
                            0.5, // aggressive threshold
                            ctx_budget.token_estimate_ratio(),
                            ctx_budget.fresh_tail_count(),
                        );
                        messages.push(UnifiedMessage::user(CRITICAL_CONTEXT_NOTICE));
                    }
                    super::context_budget::LoopDirective::StopDiminishing => {
                        messages.push(UnifiedMessage::user(DIMINISHING_RETURNS_NOTICE));
                    }
                    super::context_budget::LoopDirective::Continue => {}
                }
            }

            // Hard safety net: enforce context limit (unchanged)
            enforce_context_limit(
                &mut messages,
                &system_prompt,
                &tool_defs,
                self.config.token_budget,
                self.context_budget
                    .as_ref()
                    .map(|b| b.fresh_tail_count())
                    .unwrap_or(6),
                self.context_budget
                    .as_ref()
                    .map(|b| b.token_estimate_ratio())
                    .unwrap_or(3.5),
            );
```

- [ ] **Step 5: Add after-turn metrics recording and directive check**

After the tool execution block (after line 671 `tool_calls_made += 1;`), before the `consecutive_errors` check, add:

```rust
            // --- After-turn: record metrics for diminishing returns detection ---
            if let Some(ref mut ctx_budget) = self.context_budget {
                let turn_productive = response.has_tool_calls()
                    && response.tool_calls.iter().any(|_| consecutive_errors == 0);
                let output_tokens = response
                    .usage
                    .as_ref()
                    .map(|u| u.output_tokens as usize)
                    .unwrap_or(0);
                let post_directive = ctx_budget.after_turn(
                    super::context_budget::TurnMetrics {
                        output_tokens,
                        tool_calls: response.tool_calls.len(),
                        productive: turn_productive,
                    },
                );
                if post_directive == super::context_budget::LoopDirective::StopDiminishing {
                    messages.push(UnifiedMessage::user(DIMINISHING_RETURNS_NOTICE));
                    // Will naturally trigger final reply in next iteration
                }
            }
```

- [ ] **Step 6: Skip tool execution when FinalReply or StopDiminishing**

Wrap the tool execution block (the `for tc in &response.tool_calls` loop) with a guard:

```rust
            // Skip tool execution if context budget says to stop
            let skip_tools = matches!(
                budget_directive,
                super::context_budget::LoopDirective::FinalReply
                    | super::context_budget::LoopDirective::StopDiminishing
            );

            if !skip_tools {
                // Act: process each tool call
                for tc in &response.tool_calls {
                    // ... existing tool execution code unchanged ...
                }
            }
```

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: Errors only in `run_loop.rs` (still referencing old `ToolCompactorConfig`) — fixed in Task 7.

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "feat(agent_loop): integrate ContextBudget into main loop"
```

---

### Task 7: Update `run_loop.rs` — Build ContextBudgetConfig

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Replace ToolCompactorConfig construction with ContextBudgetConfig**

Replace lines 131-139:

```rust
        // From:
        // let tool_compactor_config = self.session_compactor.as_ref().map(|sc| {
        //     crate::agent_loop::ToolCompactorConfig {
        //         token_budget: token_budget as u64,
        //         context_threshold: sc.config().context_threshold,
        //         token_estimate_ratio: sc.config().token_estimate_ratio,
        //         fresh_tail_count: sc.config().fresh_tail_count,
        //     }
        // });

        // To:
        let context_budget = self.session_compactor.as_ref().map(|sc| {
            let config = crate::agent_loop::ContextBudgetConfig {
                token_budget: token_budget as u64,
                warning_threshold: 0.70,
                critical_threshold: 0.85,
                token_estimate_ratio: sc.config().token_estimate_ratio,
                fresh_tail_count: sc.config().fresh_tail_count,
                circuit_breaker_max: 3,
                diminishing_window: 4,
                diminishing_threshold: 500,
            };
            crate::agent_loop::ContextBudget::new(&config)
        });
```

- [ ] **Step 2: Update AgentLoop builder call**

Find the `.with_tool_compactor_config(tool_compactor_config)` call in `run_loop.rs` and replace with:

```rust
            .with_context_budget(context_budget)
```

Run: `grep -n "with_tool_compactor_config\|with_context_budget" src/gateway/execution_engine/run_loop.rs`
to locate the exact line.

- [ ] **Step 3: Verify full compilation**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: Clean compilation, no errors.

- [ ] **Step 4: Run all existing tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All tests pass (existing + new context_budget tests).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "feat(gateway): wire ContextBudget into agent loop execution"
```

---

### Task 8: Cleanup — Remove ToolCompactorConfig

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Verify no remaining references to ToolCompactorConfig**

Run: `grep -rn "ToolCompactorConfig" src/ 2>&1`
Expected: Zero matches (if any remain, update them to use `ContextBudgetConfig`).

- [ ] **Step 2: Clean up any dead imports**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -40`
Fix any unused import warnings.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore 2>&1 | tail -30`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(agent_loop): remove deprecated ToolCompactorConfig"
```

---

### Task 9: Integration smoke test

**Files:**
- No new files — verification only

- [ ] **Step 1: Run cargo check for the entire workspace**

Run: `cargo check 2>&1 | tail -20`
Expected: Clean compilation.

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: No warnings.

- [ ] **Step 3: Run all core tests**

Run: `cargo test -p alephcore 2>&1 | tail -30`
Expected: All tests pass including the 15+ new context_budget tests.

- [ ] **Step 4: Run context_budget tests specifically with output**

Run: `cargo test -p alephcore --lib context_budget -- --nocapture 2>&1`
Expected: All pass with tracing output visible for Warning/Critical scenarios.

- [ ] **Step 5: Final commit (if any clippy fixes were needed)**

```bash
git add -A
git commit -m "chore: clippy fixes after context budget integration"
```
