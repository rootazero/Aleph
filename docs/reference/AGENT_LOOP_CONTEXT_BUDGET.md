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

Runs before each LLM call to proactively manage context.

### PreflightStage Trait

```rust
pub trait PreflightStage: Send + Sync {
    fn name(&self) -> &str;
    fn run(
        &self,
        ctx: &mut PreflightContext,
        budget: &Budget,
    ) -> impl Future<Output = Result<PreflightOutcome, PreflightError>> + Send;
}
```

### PreflightPipeline

Executes stages in sequence until budget is satisfied:

```rust
pub struct PreflightPipeline {
    stages: Vec<Box<dyn PreflightStage>>,
}
```

### Stages

#### MicrocompactStage

Content-addressed deduplication of repeated content patterns.

#### ContextCollapseStage

Collapses groups of similar messages into single representative messages.

#### AutocompactStage

LLM-based summarization of message sequences.

## Tier 3: Emergency (Autocompact)

Final fallback when pre-flight stages cannot reduce context enough.

### AutocompactError

```rust
pub enum AutocompactError {
    BudgetTooSmall,
    SummarizationFailed(String),
    LlmClientError(String),
}
```

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
