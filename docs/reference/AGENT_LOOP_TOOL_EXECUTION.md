# Agent Loop Tool Execution

> Tool execution context, progress tracking, and pipeline architecture.

## Overview

Tools are executed through a 7-stage pipeline with comprehensive progress tracking and error recovery.

## Tool Pipeline Stages

```
┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐
│ Approve │ → │ Invoke  │ → │ Collect │ → │ Merge   │ → │ Compact │ → │ Budget  │ → │ Return  │
│         │   │         │   │ Results │   │ Results │   │ Results │   │ Check   │   │         │
└─────────┘   └─────────┘   └─────────┘   └─────────┘   └─────────┘   └─────────┘   └─────────┘
```

## ToolExecutionContext

```rust
pub struct ToolExecutionContext {
    pub tool_name: String,
    pub max_tool_result_tokens: usize,
    pub truncate_to_tokens: usize,
    pub truncation_policy: TruncationPolicy,
    pub cascade_policy: CascadePolicy,
}
```

## CascadePolicy

```rust
pub enum CascadePolicy {
    /// Abort all sibling tools when one fails
    AbortSiblings,
    /// Run all tools regardless of failures
    Isolated,
}
```

## ToolProgress Events

```rust
pub enum ToolProgress {
    Started { tool_name: String, invocation_id: String },
    Approved { tool_name: String },
    Rejected { tool_name: String, reason: String },
    Completed { tool_name: String, result_tokens: usize },
    Failed { tool_name: String, error: String },
}
```

## ToolCall Structure

```rust
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    pub invocation_id: String,
}
```

## ToolResult Structure

```rust
pub struct ToolResult {
    pub invocation_id: String,
    pub content: ToolResultContent,
    pub error: Option<String>,
}

pub enum ToolResultContent {
    Text(String),
    Image { data: String, media_type: String },
    Audio { data: String, media_type: String },
}
```

## Approval Flow

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  ToolCall    │ ──▶ │  Approval    │ ──▶ │   Executor   │
│  Received    │     │   Gate       │     │              │
└──────────────┘     └──────────────┘     └──────────────┘
                           │
                           ▼
                     ┌──────────────┐
                     │   Rejected   │ ──▶ Return error to LLM
                     └──────────────┘
```

## Result Merging

When multiple tools run in parallel, results are merged based on `CascadePolicy`:

- **AbortSiblings**: First failure aborts remaining tools
- **Isolated**: All tools complete, results merged regardless of failures

## Result Compact

After merging, results pass through compact stage which applies `TruncationPolicy`:

```rust
pub enum TruncationPolicy {
    HeadAndTail { keep_head_tokens: usize, keep_tail_tokens: usize },
    HeadOnly { keep_tokens: usize },
    TailOnly { keep_tokens: usize },
}
```

## Related Documents

- [AGENT_LOOP_CONTEXT_BUDGET.md](./AGENT_LOOP_CONTEXT_BUDGET.md) - Context budget tiers
- [AGENT_LOOP_RECOVERY.md](./AGENT_LOOP_RECOVERY.md) - Recovery from truncation errors
