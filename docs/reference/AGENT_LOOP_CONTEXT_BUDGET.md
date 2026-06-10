# Agent Loop Context Budget

> Context management architecture for the Aleph agent system.

## Overview

The context budget system manages token usage across agent turns using a three-tier architecture:

```
┌─────────────────────────────────────────────────────────────┐
│                    TIER 3: Emergency                        │
│            LLM summarization (AutocompactStage)            │
├─────────────────────────────────────────────────────────────┤
│                    TIER 2: Pre-flight                       │
│    Microcompact → ContextCollapse → Autocompact pipeline    │
├─────────────────────────────────────────────────────────────┤
│                    TIER 1: Inline                          │
│           Per-tool result truncation (head+tail)            │
└─────────────────────────────────────────────────────────────┘
```

## Tier 1: Inline Truncation

Applied immediately after each tool execution.

### ToolExecutionContext

```rust
pub struct ToolExecutionContext {
    pub max_tool_result_tokens: usize,
    pub truncate_to_tokens: usize,
    pub truncation_policy: TruncationPolicy,
}
```

### TruncationPolicy

```rust
pub enum TruncationPolicy {
    HeadAndTail { keep_head_tokens: usize, keep_tail_tokens: usize },
    HeadOnly { keep_tokens: usize },
    TailOnly { keep_tokens: usize },
}
```

### CascadePolicy

Controls sibling tool behavior when one tool fails:

```rust
pub enum CascadePolicy {
    /// Abort all sibling tools when one fails
    AbortSiblings,
    /// Run all tools regardless of failures
    Isolated,
}
```

## Tier 2: Pre-flight Pipeline

Runs at the start of every Think turn — BEFORE the budget pressure check
and BEFORE the LLM compactor — to proactively shrink context with cheap,
LLM-free transforms.

### Wiring (2026-05-20)

`src/harness/agent/think.rs::run_turn` step 2a invokes
`HarnessDeps.preflight_pipeline.run(&mut messages, &pressure, fresh_tail)`
on every turn when `[context_budget]` is configured. The pipeline is
assembled in `src/orchestrator/harness_bridge.rs` and is `Some` whenever
`context_compactor` is `Some` — same opt-in as the compactor itself.

### PreflightStage Trait

`src/context/budget/preflight.rs`:

```rust
#[async_trait]
pub trait PreflightStage: Send + Sync {
    fn name(&self) -> &'static str;
    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize; // returns tokens freed
}
```

### PreflightPipeline

Runs stages in registration order, summing tokens freed:

```rust
pub struct PreflightPipeline {
    stages: Vec<Box<dyn PreflightStage>>,
}

impl PreflightPipeline {
    pub async fn run(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize;
}
```

### Cheap-Pass Stages (live)

Located in `src/context/budget/cheap_passes/`:

#### ToolResultPruningStage

Replaces stale `ToolResult` content larger than 200 tokens with a one-line
placeholder: `"[pruned tool_result: <tool_name>, ~N tokens]"`. Protects
`fresh_tail_count` messages. Skips when the placeholder wouldn't save
tokens. (Hermes-borrowed heuristic.)

#### HistoricalImageStrippingStage

Drops `ContentBlock::Image` blocks from every message preceding the newest
image-bearing turn (and outside the fresh tail), replacing each with
`"[image stripped from history]"`. ~1500 tokens saved per image.
(Matches Anthropic pricing + hermes constant.)

## Tier 3: LLM Compactor

`src/context/compact/compactor.rs::ContextCompactor::compact()` is invoked
by `harness/agent/think.rs` step 2c when the budget directive is
`CompactAndContinue`. Falls back to deterministic truncation on provider
failure. The 5-section summary template (Primary Request / Key Decisions /
Files & Code / Current State / Pending) is hermes-compatible.

### Fingerprint cache (per-run)

The harness rebuilds the message list from the session log every turn, so an
in-place compaction is discarded by the next rebuild. The compactor therefore
keeps a per-run fingerprint cache (`CompactionCache { start, end, hash,
summary }`, openteams compression-cache parity): when the previously covered
range still hashes identically in the rebuilt list, the cached summary is
reapplied with **zero API cost** (`CompactStrategy::CacheReuse`). Once the
un-summarized gap behind the summary grows past 8 messages / ~4 K estimated
tokens, one LLM merge over `[old summary + gap]` absorbs it (openclaw "merge
prior summaries") and the cache cover widens monotonically. Any change inside
the covered prefix (e.g. a preflight pass pruning differently) misses the hash
and falls back to a full recompaction. Without the cache, a high-pressure run
paid a fresh side-channel summarization call on every Think turn and the
changing summary text thrashed the provider prompt cache.

## Token-Estimate Calibration (server-observed feedback)

Every tier above reacts to a single number: `ContextPressure::ratio`, derived
from a heuristic char-per-token estimate (`pressure.rs::detect_content_ratio`,
1.5 CJK / 2.5 code / 3.5 prose). A fixed ratio is necessarily wrong for any
given conversation's real token mix, so the budget **calibrates the estimate
against the provider's reported prompt size** after each turn.

### Mechanism

`src/context/budget/mod.rs`:

```rust
impl ContextBudget {
    /// Feed back the provider's ground-truth prompt size for the request just sent.
    pub fn observe_actual_usage(&mut self, observed_prompt_tokens: usize);
}
```

- `before_turn()` / `note_compaction_effect()` scale their `ContextPressure`
  snapshot by `self.calibration` (via `ContextPressure::calibrated`). Until the
  first observation `calibration` is `None` → factor `1.0` → **byte-identical**
  to the pre-calibration path.
- After each LLM turn, `harness/agent/think.rs` calls
  `observe_actual_usage(usage.prompt_tokens_total())`. The saved `last_pressure`
  is the calibrated estimate of *that exact prompt*, so `observed / estimated`
  (with the previous factor backed out) is the residual error. It is clamped to
  `[0.25, 4.0]` (rejecting transient noise — mid-flight resends, degenerate
  usage reports) and EWMA-smoothed (`α = 0.3`) into the running multiplier.
- `TokenUsage::prompt_tokens_total()` (`src/providers/adapter.rs`) folds the
  cached + cache-creation portions back in using the same Anthropic-vs-OpenAI
  convention detection as `cache_hit_ratio`, so a warm cache hit (tiny
  `input_tokens`) doesn't look like the prompt shrank.

### Effect

The estimate converges to *this conversation's* true tokenizer ratio within a
few turns, adapting to content mix, the provider's tokenizer, and cache
behaviour the static ratio cannot capture. This is purely an accuracy
improvement to the number that already drives compaction — it adds no new
decision category and makes no LLM call (R7/R10-safe). Compared to codex's
one-shot `ServerObserved` prefill snapshot, the EWMA multiplier is continuous:
it also corrects the estimate of the *growing tail* the provider hasn't yet
counted.

## Budget Structure

```rust
pub struct Budget {
    pub max_tokens: usize,
    pub warning_threshold: f32,
    pub critical_threshold: f32,
}
```

## Related Documents

- [AGENT_LOOP_TOOL_EXECUTION.md](./AGENT_LOOP_TOOL_EXECUTION.md) - Tool execution context and pipeline
- [AGENT_LOOP_RECOVERY.md](./AGENT_LOOP_RECOVERY.md) - Truncation recovery mechanisms
