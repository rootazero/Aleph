# Agent Loop Evolution — Learning from Claude Code, Built for Aleph

**Date:** 2026-04-08
**Status:** Approved
**Scope:** agent_loop module — context management, tool execution, recovery mechanisms

---

## Motivation

Aleph's agent loop is production-grade with a solid 5-stage state machine (Prepare→Think→Resolve→Act→Finalize), 7-stage tool pipeline, and pressure-based context budget. However, comparing with Claude Code reveals gaps in context management depth, tool execution granularity, and recovery robustness.

This design addresses all gaps while preserving Aleph's Rust-native advantages: type safety, trait composability, tokio concurrency, and the existing CompactionPipeline architecture.

**Principle:** Learn from Claude Code, don't copy it. Every feature is redesigned to fit Aleph's architecture (R8 LLM Sovereignty, P1 Low Coupling, P6 Simplicity).

---

## Architecture: Three-Phase Context Management

Context management operates at three distinct moments, each with escalating aggressiveness:

```
Phase 1: Inline (tool result produced)
  └─ Per-tool result budget — cap individual tool outputs immediately

Phase 2: Pre-flight (before each Think turn)
  └─ MicrocompactStage → ContextCollapseStage → AutocompactStage
     Progressive: dedup → fold → LLM summarize
     Each gated by pressure threshold (0.3 → 0.5 → 0.65)

Phase 3: Emergency (pressure exceeds warning threshold)  ← existing
  └─ ImageStripper → ResultClearing → ToolCompact → RoundDrop
     Existing CompactionPipeline, unchanged
```

### New Trait: PreflightStage

Complements the existing synchronous `CompactionStage` with an async variant for pre-flight processing:

```rust
#[async_trait]
pub trait PreflightStage: Send + Sync {
    fn name(&self) -> &'static str;
    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize; // tokens freed
}
```

- `CompactionStage` — sync, local compute, emergency use
- `PreflightStage` — async, may call LLM, pre-turn use

### Integration Point (loop_core.rs Prepare stage)

```rust
TurnState::Prepare => {
    let directive = self.context_budget.before_turn(...);
    // NEW: pre-flight context preparation
    if matches!(directive, LoopDirective::Continue | LoopDirective::CompactAndContinue) {
        self.preflight.run(&mut messages, &pressure, fresh_tail).await;
    }
    // existing: if CompactAndContinue → run emergency pipeline
}
```

---

## P0: Per-tool Result Budget (Inline)

**Problem:** Global `MAX_TOOL_RESULT_TOKENS = 8000` treats all tools equally.

**Solution:** Add optional `max_result_tokens` to `ToolDefinition`. Apply after tool execution (Stage 5.5 in tool pipeline).

**Truncation strategy — head+tail preservation:**

```rust
fn apply_result_budget(output: &str, tool_def: &ToolDefinition) -> String {
    let limit = tool_def.max_result_tokens.unwrap_or(MAX_TOOL_RESULT_TOKENS);
    let estimated = estimate_tokens_smart(output);
    if estimated <= limit { return output.to_string(); }
    // Head 70% + Tail 30% — tail often contains errors/final results
    let head_tokens = (limit as f64 * 0.7) as usize;
    let tail_tokens = limit - head_tokens;
    format!(
        "{}\n\n[... truncated {} tokens ...]\n\n{}",
        take_head_tokens(output, head_tokens),
        estimated - limit,
        take_tail_tokens(output, tail_tokens),
    )
}
```

**Default budgets:** Read=12K, WebFetch=10K, Bash=8K, Grep=6K, others=8K.

**Improvement over Claude Code:** Head+tail strategy preserves error messages in output tail, vs Claude Code's head-only truncation.

---

## P1: Microcompact (Pre-flight)

**Problem:** Same file read multiple times, same directory globbed repeatedly — each full output occupies context.

**Solution:** Content-addressed cache. Replace consumed, duplicate tool results with compact references.

```rust
pub struct MicrocompactStage {
    entries: HashMap<String, CacheEntry>,  // tool_name:args → (hash, compact_ref)
}

struct CacheEntry {
    content_hash: u64,        // xxhash of output
    compact_ref: String,      // "[cached: Read src/main.rs, 150 lines, same as turn 3]"
    original_tokens: usize,
    turn_created: usize,
}
```

**Key rules:**
- Only replace outside fresh_tail (recent results stay intact)
- Only replace when content_hash matches (modified file → keep new version)
- Pressure gate: ratio > 0.3 (don't dedup at low pressure)
- Cache lifetime: per agent loop session

---

## P2: Cascading Error Abort + Per-tool Abort

**Problem:** Bash fails but sibling Read tools keep running. Only global CancellationToken exists.

**Solution:** `ToolExecutionContext` per tool call with parent-child cancel hierarchy.

```rust
pub struct ToolExecutionContext {
    pub cancel: CancellationToken,          // child of batch token
    pub progress_tx: mpsc::Sender<ToolProgress>,
    pub cascade_policy: CascadePolicy,
}

pub enum CascadePolicy {
    AbortSiblings,  // Bash, Write, Edit — failure cancels concurrent siblings
    Isolated,       // Read, Grep, Glob — failure is isolated
}
```

**Cancel hierarchy:**

```
Session CancellationToken
  └─ Turn CancellationToken
       └─ Batch CancellationToken
            ├─ Tool A CancellationToken  ← cascade can cancel individually
            ├─ Tool B CancellationToken
            └─ Tool C CancellationToken
```

Rust's `CancellationToken` parent-child propagation is more elegant than Claude Code's manual `addEventListener` approach.

**Improvement over Claude Code:** Cascade includes Write and Edit (not just Bash) — write failure makes subsequent reads meaningless.

**Synthetic result for aborted tools:**
```rust
ToolResult::Error {
    error: "[Aborted] {tool_name} cancelled because sibling failed: {cause}",
    retryable: true,
}
```

---

## P3: Progress Streaming

**Problem:** Long-running tools (WebFetch, complex Bash) give no feedback until completion.

**Solution:** Leverage `progress_tx` from ToolExecutionContext.

```rust
pub enum ToolProgress {
    Status { tool_id: String, message: String },
    PartialOutput { tool_id: String, chunk: String },
}
```

- `try_send` (non-blocking) — progress is best-effort, never blocks tool execution
- Channel capacity 64 — sufficient buffer
- Only streaming-capable tools send progress (Bash stdout, WebFetch chunks)
- DeltaSink forwards to WebSocket/SSE

Progress collected via `tokio::select!` alongside tool result collection in the executor loop.

---

## P4: Context Collapse (Pre-flight)

**Problem:** Exploratory message sequences (read 5 files, search 4 patterns) occupy disproportionate context.

**Solution:** Detect and fold consecutive exploratory message groups into summaries.

```rust
enum GroupType {
    FileExploration,   // 3+ consecutive Read/Glob rounds
    SearchSweep,       // 2+ consecutive Grep rounds
    DiagnosticRun,     // 3+ consecutive non-mutating Bash rounds
    ReasoningChain,    // Multi-turn assistant text without tool calls
}
```

**Key rules:**
- Pure local computation (no LLM) — fast and deterministic
- Pressure gate: ratio > 0.5
- Minimum savings threshold: 500 tokens (small groups not worth folding)
- Never fold groups containing Write/Edit (mutation context must be preserved)
- Summaries preserve key info: file paths, search patterns, match counts
- Process groups back-to-front to maintain stable indices

**Improvement over Claude Code:** Typed GroupType enum ensures exhaustive handling. Summaries extract domain-specific info (paths, patterns) instead of generic text.

---

## P5: Autocompact (Pre-flight, Async LLM)

**Problem:** Structural compression (P1-P4) has limits. True semantic compression requires LLM understanding.

**Solution:** Call a cheap/fast LLM to summarize old conversation segments.

```rust
pub struct AutocompactStage {
    provider: Arc<dyn LoopProvider>,
    trigger_threshold: f64,   // 0.65
    target_ratio: f64,        // 0.3
    cooldown_turns: usize,    // min turns between compactions
    last_compact_turn: AtomicUsize,
}
```

**Gates (all must pass):**
1. Pressure ratio > 0.65
2. Outside cooldown period
3. At least 6 messages in compressible zone

**Summary range:** Preserves first user message (original task) + fresh tail. Safe boundary detection prevents orphaning tool_call/tool_result pairs.

**LLM call:** Uses `ModelPreference::Cheapest`, max_tokens=2048, temperature=0.0. Failure is graceful — logs warning, returns 0 tokens freed, falls through to emergency pipeline.

**Improvements over Claude Code:**
- Explicit cheapest model preference (Claude Code uses same model)
- Cooldown mechanism prevents thrashing
- Dense paragraph prompt (not bullet lists) for token efficiency

### PreflightPipeline Orchestration

```rust
pub fn default_pipeline(provider: Arc<dyn LoopProvider>) -> PreflightPipeline {
    PreflightPipeline {
        stages: vec![
            Box::new(MicrocompactStage::new()),        // ratio > 0.3
            Box::new(ContextCollapseStage::new()),      // ratio > 0.5
            Box::new(AutocompactStage::new(provider)),  // ratio > 0.65
        ],
    }
}
```

Progressive thresholds ensure: low pressure → nothing, medium-low → dedup, medium-high → fold, high → LLM summarize.

---

## P6: Max Output Tokens Escalation

**Problem:** Simple truncation recovery without escalation loop.

**Solution:** `TruncationRecovery` struct with doubling strategy.

```rust
pub struct TruncationRecovery {
    current_max: usize,
    absolute_cap: usize,      // provider limit
    attempts: usize,
    max_attempts: usize,      // 3
    partial_fragments: Vec<String>,
}

pub enum EscalateDecision {
    Retry { new_max_tokens: usize, continuation_prompt: String },
    GiveUp { assembled: String },
}
```

**Strategy:** Double max_tokens each attempt (4K→8K→16K→cap), max 3 attempts. Partial fragments accumulated and assembled. Continuation prompt tells LLM to continue without repeating.

**Integration:** Resolve stage checks MaxTokens stop reason → record fragment → escalate → either retry Think or finalize with assembled output.

---

## P7: Prompt Caching

**Problem:** System prompt + tool definitions re-sent every turn, billed as full input tokens.

**Solution:** Add `cache_control: Option<CacheControl>` to `ContentBlock`.

```rust
pub enum CacheControl {
    Ephemeral,  // Anthropic: ~5 min TTL
}
```

System prompt blocks split into:
- Block 1: Base prompt → `CacheControl::Ephemeral`
- Block 2: Tool definitions → `CacheControl::Ephemeral`
- Block 3: Dynamic context (env, guidance) → no cache

Only effective for Anthropic API. Other providers ignore the field.

---

## P8: Enhanced 413 Recovery

**Problem:** Single-level emergency truncate on prompt-too-long.

**Solution:** Four-tier recovery cascade:

1. **Pre-flight pipeline** (if not already run this turn)
2. **Emergency compaction pipeline** (existing)
3. **Aggressive round drop** — halve fresh_tail protection
4. **Forced autocompact** — LLM summarize entire compressible history

Each tier only activates if previous tiers freed insufficient tokens.

---

## Module Structure

### New files

```
src/agent_loop/context_budget/
├── preflight.rs              # PreflightStage trait + PreflightPipeline
├── microcompact.rs           # MicrocompactStage
├── context_collapse.rs       # ContextCollapseStage + GroupType detection
└── autocompact.rs            # AutocompactStage (async LLM)

src/agent_loop/
└── tool_execution_context.rs # ToolExecutionContext + CascadePolicy + ToolProgress
```

### Modified files

```
src/agent_loop/
├── loop_core.rs              # Prepare: preflight call; Resolve: escalation
├── tool_pipeline.rs          # Stage 5.5: per-tool result budget
├── streaming_bridge.rs       # ToolExecutionContext integration + progress channel
├── tool_orchestrator.rs      # Cascade abort + batch_cancel hierarchy
├── truncation_recovery.rs    # Rewrite: escalation loop
└── context_budget/mod.rs     # Import preflight module

src/providers/
└── message.rs                # CacheControl on ContentBlock
```

### Code to remove

| Location | What | Replaced by |
|----------|------|-------------|
| `tool_pipeline.rs` | Global-only MAX truncation | Per-tool budget + head/tail |
| `loop_core.rs` | Simple `emergency_truncate()` | Four-tier 413 recovery |
| `loop_core.rs` | Simple truncation recovery | TruncationRecovery escalation |
| `streaming_bridge.rs` | Direct global cancel passing | Parent-child cancel hierarchy |

### Code preserved (unchanged)

| Location | What | Reason |
|----------|------|--------|
| `context_budget/pipeline.rs` | CompactionPipeline + 4 stages | Emergency layer, fully functional |
| `context_budget/pressure.rs` | PressureSensor | Shared by pre-flight and emergency |
| `context_budget/mod.rs` | LoopDirective, diminishing returns | Correct logic |
| `retry.rs` | Error classification + backoff | Complete |
| `tool_pipeline.rs` | 7-stage pipeline flow | Only insert at stage 5.5 |

---

## Implementation Phases

```
Phase 1: Infrastructure — PreflightStage trait, PreflightPipeline, ToolExecutionContext
Phase 2: Inline       — Per-tool Result Budget
Phase 3: Pre-flight   — Microcompact → ContextCollapse → Autocompact
Phase 4: Tool         — Cascade abort + Per-tool abort + Progress streaming
Phase 5: Recovery     — Max Output Tokens escalation + Enhanced 413
Phase 6: Optimize     — Prompt Caching
Phase 7: Cleanup      — Remove old code, cargo clippy + test
```

Each phase is independently compilable and testable.

---

## Design Decisions Summary

| Decision | Rationale |
|----------|-----------|
| Three-phase model (inline/pre-flight/emergency) | Separation of concerns (P1), each phase has clear trigger |
| PreflightStage is async trait | Autocompact needs LLM call; sync CompactionStage preserved for emergency |
| Head+tail truncation | Error messages often in output tail — better than head-only |
| Pressure-gated stages (0.3/0.5/0.65) | Progressive degradation: preserve info at low pressure |
| CascadePolicy includes Write/Edit | Write failure makes subsequent reads meaningless |
| try_send for progress | Best-effort, never blocks tool execution |
| Context collapse is local, not LLM | Fast + deterministic; LLM reserved for autocompact |
| Autocompact uses cheapest model | Summary doesn't need main model's full capability |
| Cooldown on autocompact | Prevent thrashing on borderline pressure |
| Four-tier 413 recovery | Graceful degradation: soft → medium → aggressive → nuclear |
