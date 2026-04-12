# Agent Loop Recovery

> Truncation recovery and 413 Prompt-Too-Long handling.

## Overview

When the LLM returns a 413 (Prompt Too Large) error or max tokens is exceeded, the agent employs a 4-level recovery cascade.

## TruncationRecovery State Machine

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Active   │ ──▶ │  Recovering │ ──▶ │   Resolved  │
└─────────────┘     └─────────────┘     └─────────────┘
                           │
                           ▼
                    ┌─────────────┐
                    │   Failed    │
                    └─────────────┘
```

## 4-Level Recovery Cascade

### Level 1: Per-Tool Result Truncation

Immediately truncate tool results using `ToolExecutionContext`:

```rust
pub struct ToolExecutionContext {
    pub max_tool_result_tokens: usize,
    pub truncate_to_tokens: usize,
    pub truncation_policy: TruncationPolicy,
}
```

### Level 2: Pre-flight Pipeline

Run `PreflightPipeline` to proactively reduce context before LLM call:

1. **MicrocompactStage** - Deduplicate repeated content
2. **ContextCollapseStage** - Collapse similar message groups
3. **AutocompactStage** - LLM summarization

### Level 3: Context Collapse

Aggressive collapse of message history when pre-flight insufficient:

- Groups consecutive messages of same type
- Keeps first and last message in group
- Middle messages summarized or removed

### Level 4: Autocompact

Final fallback using LLM summarization:

```rust
pub struct AutocompactStage {
    pub min_budget_reduction: f32,
    pub max_summaries_per_turn: usize,
}
```

## 413 Recovery Flow

```
┌──────────────────────────────────────────────────────────────┐
│                        413 Error                             │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  Level 1: Inline Truncation   │
              │  (Per-tool result budgets)    │
              └───────────────────────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  Level 2: Pre-flight Pipeline │
              │  (Microcompact/Collapse)     │
              └───────────────────────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  Level 3: Context Collapse   │
              │  (Message group collapse)     │
              └───────────────────────────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  Level 4: Autocompact        │
              │  (LLM summarization)         │
              └───────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │   Retry LLM     │
                    │   with reduced  │
                    │   context       │
                    └─────────────────┘
```

## MaxTokens Escalation

When `max_tokens` parameter is too small for response:

1. Calculate minimum viable response size
2. If current `max_tokens` < minimum viable:
   - Increase `max_tokens` to minimum viable
   - Retry with same (reduced) context
3. If still fails after escalation:
   - Return error to caller

## Error Types

```rust
pub enum RecoveryError {
    BudgetTooSmall,
    TruncationFailed(String),
    AutocompactFailed(String),
    MaxTokensEscalationFailed,
}
```

## Related Documents

- [AGENT_LOOP_CONTEXT_BUDGET.md](./AGENT_LOOP_CONTEXT_BUDGET.md) - Context budget tiers
- [AGENT_LOOP_TOOL_EXECUTION.md](./AGENT_LOOP_TOOL_EXECUTION.md) - Tool execution pipeline
