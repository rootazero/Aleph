# Context Management Pipeline — Round 2 Optimization

**Date**: 2026-03-31
**Status**: Approved
**Scope**: `src/agent_loop/context_budget/`, `src/agent_loop/loop_core.rs`, `src/memory/session_compactor/`

## Background

Round 1 established a three-layer context management system: ContextBudget (pressure sensing) → ToolCompactor (deterministic compression) → SessionCompactor (LLM summarization). This round addresses gaps identified by comparing with Claude Code's implementation:

1. **Token estimation inaccuracy** — fixed `chars/3.5` ratio fails for CJK (~1.5) and code (~2.5)
2. **No API usage anchoring** — cumulative estimation compounds errors; CC anchors to server-reported usage
3. **Missing microcompact layer** — CC clears stale tool results before attempting summarization
4. **Coarse truncation** — `enforce_context_limit` cuts by individual messages, not by conversation round
5. **Simple summary prompt** — lacks structured analysis/scratchpad separation
6. **Zero observability** — no token breakdown, no duplicate detection, no pipeline metrics

## Architecture: Strategy Pipeline

Split `ContextBudget` into **sensing** (thin) and **execution** (pipeline):

```
ContextBudget (sensing)
  ├── PressureSensor        ← Token estimation + API usage anchoring
  ├── CompactionCircuitBreaker  ← Unchanged
  └── DiminishingReturnsDetector ← Unchanged

CompactionPipeline (execution)
  ├── Stage 0: ImageStripper    ← Images/attachments → text markers
  ├── Stage 1: MicroCompact     ← Clear stale tool results (zero cost)
  ├── Stage 2: ToolCompact      ← Compress to summary lines (existing)
  └── Stage 3: RoundDrop        ← Drop oldest API rounds

ContextDiagnostics (observability)
  └── Token breakdown + duplicate detection + pipeline metrics
```

Each stage runs in order. After each stage, pressure is re-measured. If below target, the pipeline stops. Cost goes low → high; most sessions resolve at Stage 0+1.

## Component Designs

### 1. PressureSensor

Replaces the inline `ContextPressure::compute` estimation logic.

**API usage anchoring**: After each LLM call, the API returns actual `input_tokens` in usage. The sensor stores this as an anchor point. For the next pressure measurement:

```
estimated_tokens = anchor.input_tokens + estimate_delta(messages_since_anchor)
```

This bounds estimation error to only the delta (typically 1-2 messages), not the entire history.

**Content-aware ratio**: Instead of fixed 3.5, detect content type per message:
- CJK content > 30% → ratio 1.5
- Code-like content → ratio 2.5
- Default English → ratio 3.5
- Mixed → weighted average

Detection is a fast char-scan (~10 CJK range checks + keyword heuristic), no LLM.

**Fallback**: When no anchor exists (first turn), uses content-aware ratio for full estimation.

```rust
pub struct PressureSensor {
    anchored_usage: Option<AnchoredUsage>,
    default_ratio: f64,
}

struct AnchoredUsage {
    input_tokens: usize,
    message_count_at_anchor: usize,
}

impl PressureSensor {
    /// Measure current pressure. Uses anchor + delta when available.
    pub fn measure(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        token_budget: u64,
    ) -> ContextPressure { ... }

    /// Update anchor from API response usage.
    pub fn update_anchor(&mut self, input_tokens: usize, message_count: usize) { ... }
}
```

### 2. CompactionPipeline

Trait-based pipeline with ordered stages.

```rust
pub trait CompactionStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn compact(
        &self,
        messages: &mut [UnifiedMessage],
        sensor: &PressureSensor,
        fresh_tail_count: usize,
    ) -> usize; // tokens_freed
}

pub struct CompactionPipeline {
    stages: Vec<Box<dyn CompactionStage>>,
}

pub struct PipelineResult {
    pub pressure_before: ContextPressure,
    pub pressure_after: ContextPressure,
    pub tokens_freed: usize,
    pub stages_run: Vec<(&'static str, usize)>,
}
```

Pipeline runs each stage in order. After each stage, re-measures pressure. Stops when ratio drops below target.

#### Stage 0: ImageStripper

Replaces image content blocks (base64) with `[image, ~2000 tokens]` text markers. Only operates on messages outside the fresh tail. Preserves the most recent image (user may still reference it).

Estimated savings: ~2000 tokens per image.

#### Stage 1: MicroCompact

Clears consumed tool results entirely, replacing with `[Old result cleared]`. More aggressive than ToolCompact — doesn't preserve summary information. Targets oldest results first.

Differences from Stage 2 (ToolCompact):
- MicroCompact: `"[Old result cleared]"` — zero information retained
- ToolCompact: `"[Read file, 100 lines, rust]"` — metadata retained

MicroCompact runs first because it's cheaper (no content analysis) and frees more space.

#### Stage 2: ToolCompact

Wraps existing `tool_compactor::compact_if_needed()`. No changes to the compressor logic — just adapts the interface to `CompactionStage` trait.

#### Stage 3: RoundDrop

Groups messages into API rounds (user → assistant + tool_results), then drops complete rounds oldest-first. Replaces `enforce_context_limit`, `find_safe_cut_point`, and `remove_oldest_complete_round`.

```rust
struct ApiRound {
    start_idx: usize,
    end_idx: usize,  // exclusive
    estimated_tokens: usize,
}
```

Grouping algorithm:
1. Scan messages for user-message boundaries
2. Each round = user message + following assistant + tool_results until next user message
3. Drop rounds from oldest, inserting truncation notice after first drop

This is strictly better than the current approach which can orphan context by cutting at arbitrary message boundaries.

### 3. ContextDiagnostics

Token breakdown tracking for observability.

```rust
pub struct ContextDiagnostics {
    tool_request_tokens: HashMap<String, usize>,
    tool_result_tokens: HashMap<String, usize>,
    user_tokens: usize,
    assistant_tokens: usize,
    system_tokens: usize,
    file_read_counts: HashMap<String, usize>,
    pipeline_runs: Vec<PipelineResult>,
    circuit_breaker_trips: usize,
}
```

**Output channels**:
1. `tracing::info!` structured log each turn — top token consumers, duplicates
2. `PipelineResult` after each compaction — stages run, tokens freed
3. `summary()` method for external consumers (future debug panel)

**Duplicate file read detection**: Scans tool_use messages where name contains "read"/"Read", extracts file path from input JSON, counts occurrences per path. Informational only — no automatic action.

### 4. Summary Prompt Upgrade

Upgrade `summary_engine.rs` prompt to use analysis/summary separation:

```
You are a conversation compressor. Condense the following conversation into a structured summary.

<analysis> (scratchpad — will be stripped before entering context)
1. User's primary request and intent
2. Key technical concepts and decisions
3. Files and code sections involved (preserve paths)
4. Errors encountered and fixes applied
5. Problem-solving approaches tried
</analysis>

<summary> (final output that enters context)
- Preserve ALL user message key points (never lose user intent)
- Current work state (most recent operations, detailed)
- Pending tasks
- Optional next steps (quote from conversation where relevant)
</summary>
```

The `<analysis>` block is stripped after LLM returns, before the summary enters context. This gives the LLM reasoning space without consuming context tokens.

## Integration Changes

### loop_core.rs

The ~50-line directive match block (lines 390-444) simplifies to:

```rust
CompactAndContinue => {
    let result = pipeline.run(&mut messages, &sensor, warning_threshold, fresh_tail);
    if result.pressure_after.ratio < warning_threshold || result.tokens_freed > 500 {
        budget.notify_compaction_success();
    }
    diagnostics.record_pipeline(result);
}
FinalReply => {
    let result = pipeline.run(&mut messages, &sensor, 0.5, fresh_tail);
    diagnostics.record_pipeline(result);
    messages.push(UnifiedMessage::user(CRITICAL_CONTEXT_NOTICE));
}
```

`enforce_context_limit()` call removed — Stage 3 (RoundDrop) handles this within the pipeline.

### AgentLoop struct

```rust
pub struct AgentLoop<P: LoopProvider> {
    // ... existing fields ...
    context_budget: Mutex<Option<ContextBudget>>,
    pressure_sensor: Mutex<PressureSensor>,       // NEW
    compaction_pipeline: CompactionPipeline,       // NEW
    diagnostics: Mutex<ContextDiagnostics>,        // NEW
}
```

### API usage anchoring in the loop

After each LLM response:
```rust
if let Some(usage) = &response.usage {
    sensor.update_anchor(usage.input_tokens as usize, messages.len());
}
```

## Code Cleanup

### Deletions
| Item | File | Reason |
|------|------|--------|
| `enforce_context_limit()` | loop_core.rs | Replaced by Stage 3 RoundDrop |
| `find_safe_cut_point()` | loop_core.rs | Logic migrated to RoundDrop |
| `remove_oldest_complete_round()` | loop_core.rs | Logic migrated to RoundDrop |
| Direct `compact_if_needed` calls | loop_core.rs | Called via pipeline Stage 2 |
| Inline token estimation in `ContextPressure::compute` | context_budget.rs | Delegated to PressureSensor |
| `token_estimate_ratio` field in ContextBudgetConfig | context_budget.rs | Moved to PressureSensor |

### Preserved
| Item | Reason |
|------|--------|
| `CompactionCircuitBreaker` | Logic unchanged, stays in ContextBudget |
| `DiminishingReturnsDetector` | Logic unchanged, stays in ContextBudget |
| `tool_compactor.rs` all code | Called by Stage 2, no changes |
| All existing tests | Adapted to new signatures |

## File Plan

| File | Action |
|------|--------|
| `src/agent_loop/context_budget.rs` | Refactor: extract to `context_budget/mod.rs`, slim down |
| `src/agent_loop/context_budget/pressure.rs` | New: PressureSensor |
| `src/agent_loop/context_budget/pipeline.rs` | New: CompactionPipeline + 4 stages |
| `src/agent_loop/context_budget/diagnostics.rs` | New: ContextDiagnostics |
| `src/agent_loop/loop_core.rs` | Modify: simplify directive handling, remove enforce_context_limit |
| `src/agent_loop/mod.rs` | Modify: update exports |
| `src/memory/session_compactor/summary_engine.rs` | Modify: upgrade prompt template |

## Testing Strategy

- **PressureSensor**: Unit tests for content-type detection, anchor update, delta estimation
- **Each CompactionStage**: Unit tests with crafted message sequences
- **CompactionPipeline**: Integration test verifying cheapest-first + early-stop behavior
- **ContextDiagnostics**: Unit tests for token classification and duplicate detection
- **RoundDrop**: Tests for API-round grouping, pair integrity, truncation notice insertion
- **Existing tests**: Adapt signatures, verify all pass
