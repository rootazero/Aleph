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

### Tier-2 stages NOT currently registered

`src/context/budget/{microcompact,context_collapse,autocompact}.rs`
define additional stages (`MicrocompactStage`, `ContextCollapseStage`,
`AutocompactStage`) that implement different trait shapes. They are
*not* registered in the live `PreflightPipeline`. Future cycles may
wire them in.

## Tier 3: LLM Compactor

`src/context/compact/compactor.rs::ContextCompactor::compact()` is invoked
by `harness/agent/think.rs` step 2c when the budget directive is
`CompactAndContinue`. Falls back to deterministic truncation on provider
failure. The 5-section summary template (Primary Request / Key Decisions /
Files & Code / Current State / Pending) is hermes-compatible.

The standalone `CompactionOrchestrator` (`src/context/compact/orchestrator.rs`)
is built but has zero production consumers — the harness calls
`ContextCompactor` directly.

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
