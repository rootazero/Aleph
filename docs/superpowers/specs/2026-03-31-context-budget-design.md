# Context Budget: Multi-tier Pressure Sensing + Circuit Breaker + Diminishing Returns

**Date**: 2026-03-31
**Status**: Approved
**Scope**: `src/agent_loop/context_budget.rs` (new) + integration into loop_core, session_compactor

## Problem

Aleph's agent loop lacks graduated context pressure awareness. Current behavior:
- Single threshold (`context_threshold = 0.80`) for tool result compaction
- No circuit breaker — compaction LLM calls can fail repeatedly, wasting tokens
- No per-turn budget tracking — long agent loops may spin unproductively without detection
- Hard truncation (`enforce_context_limit`) is the only safety net after the single threshold

Claude Code solves these with three-tier warnings, a circuit breaker (3 failures → stop), and diminishing returns detection. Aleph needs equivalent capability, adapted to its Rust architecture.

## Design

### New file: `src/agent_loop/context_budget.rs` (~200 lines)

#### 1. ContextPressure — Three-tier State Machine

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPressure {
    Normal,   // < 70% of token_budget
    Warning,  // 70%–85% → force tool result compaction
    Critical, // > 85% → block new tool calls, LLM must reply directly
}
```

Thresholds (fraction of `token_budget`):

| Level    | Threshold | Action                                                     |
|----------|-----------|-------------------------------------------------------------|
| Normal   | < 70%     | No intervention                                             |
| Warning  | 70%–85%   | Force `compact_if_needed()`, emit tracing::warn             |
| Critical | > 85%     | Block tool calls, inject `[SYSTEM] Context critical` prompt |

Existing `enforce_context_limit()` remains as the final safety net after Critical.

#### 2. CompactionCircuitBreaker

```rust
#[derive(Debug)]
struct CompactionCircuitBreaker {
    consecutive_failures: u32,
    max_failures: u32,  // default: 3
    tripped: bool,
}
```

Two-state model (Closed → Open):
- **Closed**: normal compaction via LLM
- **Open** (after 3 consecutive failures): skip LLM, use `deterministic_truncate()` directly
- Reset: on compaction success or new agent loop run (per-instance lifecycle)

No Half-Open state — within a single session, if the provider is failing, retrying is wasteful.

#### 3. DiminishingReturnsDetector

```rust
#[derive(Debug)]
struct DiminishingReturnsDetector {
    window: VecDeque<TurnMetrics>,
    window_size: usize,         // default: 4
    low_output_threshold: usize, // default: 500 tokens
}

#[derive(Debug, Clone)]
struct TurnMetrics {
    output_tokens: usize,
    tool_calls: usize,
    productive: bool, // at least one successful tool execution or meaningful text
}
```

Detection algorithm:
1. Record `TurnMetrics` into sliding window after each turn
2. When window is full (4 turns):
   - Compute average `output_tokens` across window
   - If average < 500 tokens AND >= 75% turns are non-productive → `StopDiminishing`
3. Simple Q&A (`tool_calls_made == 0` total) skips detection entirely

#### 4. LoopDirective — Turn Control Signal

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDirective {
    Continue,
    CompactAndContinue,
    FinalReply,
    StopDiminishing,
}
```

#### 5. ContextBudget — Unified Orchestrator

```rust
pub struct ContextBudget {
    token_budget: usize,
    warning_threshold: f64,      // 0.70
    critical_threshold: f64,     // 0.85
    circuit_breaker: CompactionCircuitBreaker,
    diminishing: DiminishingReturnsDetector,
    pressure: ContextPressure,
    token_estimate_ratio: f64,
    fresh_tail_count: usize,
}

impl ContextBudget {
    pub fn new(config: &ContextBudgetConfig) -> Self;

    /// Before each LLM call: assess pressure, return directive
    pub fn before_turn(
        &mut self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
    ) -> LoopDirective;

    /// After each turn: record metrics, detect diminishing returns
    pub fn after_turn(&mut self, metrics: TurnMetrics) -> LoopDirective;

    /// Compaction outcome callbacks
    pub fn on_compaction_success(&mut self);
    pub fn on_compaction_failure(&mut self);

    /// Current pressure level (for logging/monitoring)
    pub fn pressure(&self) -> ContextPressure;
}
```

#### 6. ContextBudgetConfig

```rust
pub struct ContextBudgetConfig {
    pub token_budget: u64,
    pub warning_threshold: f64,       // default: 0.70
    pub critical_threshold: f64,      // default: 0.85
    pub token_estimate_ratio: f64,    // default: 3.5
    pub fresh_tail_count: usize,      // default: 6
    pub circuit_breaker_max: u32,     // default: 3
    pub diminishing_window: usize,    // default: 4
    pub diminishing_threshold: usize, // default: 500
}
```

## Integration Points

### loop_core.rs — Main Loop

Replace the current manual `compact_if_needed` + `enforce_context_limit` block with:

```rust
// Before LLM call:
let directive = self.context_budget.before_turn(&messages, &system_prompt, &tool_defs);
match directive {
    LoopDirective::Continue => {}
    LoopDirective::CompactAndContinue => {
        compact_if_needed(&mut messages, ...);
    }
    LoopDirective::FinalReply => {
        messages.push(UnifiedMessage::user(CRITICAL_CONTEXT_NOTICE));
        // Next LLM call will produce final reply without tools
    }
    LoopDirective::StopDiminishing => {
        messages.push(UnifiedMessage::user(DIMINISHING_RETURNS_NOTICE));
        // Let LLM summarize progress, then break
    }
}

// enforce_context_limit() stays as final safety net (unchanged)
enforce_context_limit(&mut messages, ...);

// ... LLM call + tool execution ...

// After turn:
let post_directive = self.context_budget.after_turn(TurnMetrics { ... });
if post_directive == LoopDirective::StopDiminishing {
    // Inject summary prompt and break on next iteration
}
```

### session_compactor/mod.rs — Circuit Breaker Integration

The `generate_summary()` method's existing 3-level fallback stays. The circuit breaker wraps it:

- Before calling `generate_summary()`: check `circuit_breaker.tripped` → if true, skip to deterministic
- On success: `context_budget.on_compaction_success()` resets counter
- On failure (all 3 levels fail): `context_budget.on_compaction_failure()` increments counter

Note: The circuit breaker lives in `ContextBudget` (per agent loop run), not in `SessionCompactor` (shared singleton). The `ContextBudget` is passed to the compactor via a callback or checked by the caller.

### AgentLoop struct

```rust
pub struct AgentLoop<P: LoopProvider> {
    provider: P,
    tool_registry: LoopToolRegistry,
    prompt_builder: PromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
    context_budget: ContextBudget,  // NEW — replaces tool_compactor_config
    delta_sink: Box<dyn DeltaSink>,
}
```

## Migration & Cleanup

1. `ToolCompactorConfig` → mark `#[deprecated]` with message pointing to `ContextBudgetConfig`
2. `AgentLoop.tool_compactor_config: Option<ToolCompactorConfig>` → replaced by `context_budget: ContextBudget`
3. Construction sites in `run_loop.rs` that build `ToolCompactorConfig` → migrate to `ContextBudgetConfig`
4. Remove manual `compact_if_needed` call block from the main loop (now driven by directive)
5. Delete `ToolCompactorConfig` entirely in next release

## Testing

Unit tests in `context_budget.rs`:
- `pressure_normal_when_under_threshold`
- `pressure_warning_triggers_compact_directive`
- `pressure_critical_triggers_final_reply`
- `circuit_breaker_trips_after_max_failures`
- `circuit_breaker_resets_on_success`
- `diminishing_returns_detected_after_window`
- `diminishing_returns_skipped_for_simple_qa`
- `before_turn_estimates_tokens_correctly`

## Non-Goals

- Prompt caching optimization (separate concern)
- Post-compact context restoration / file re-injection (future work, #4 from gap analysis)
- Image stripping before summarization (future work, #6)
- Token counting precision improvement (future work — current char/ratio estimation is sufficient)
