# Error Recovery & Context Management Enhancement

**Date**: 2026-04-03  
**Status**: Approved  
**Scope**: Surgical improvements to error recovery and context window management

## Background

Comparative analysis of OpenClaw (Claude Code) source code revealed specific gaps in Aleph's error recovery and context management. This spec targets the highest-ROI improvements without architectural changes.

## Changes

### 1. Error Recovery Enhancement

#### 1.1 Rate Limit Classification (retry.rs)

**Current**: 429 always maps to `Fatal`. Never retries, never falls back.

**Change**: Distinguish model-specific vs account-wide rate limits:
- Model-specific rate limit (error mentions specific model) → `Fallback` (switch provider may help)
- Account-wide rate limit ("account", "organization", "quota") → `Fatal` (switching won't help)
- Default 429 → `Fallback` (conservative: try switching)

**Files**: `src/agent_loop/retry.rs`

#### 1.2 Retry-After Header Bridge

**Current**: `providers/retry.rs` has `parse_retry_after()` but agent_loop ignores it.

**Change**: 
- Add `retry_after: Option<Duration>` to `RateLimitError` and `ProviderError` in `error.rs`
- `classify_error()` uses retry_after as delay when present, falls back to hardcoded delays

**Files**: `src/error.rs`, `src/agent_loop/retry.rs`, `src/providers/retry.rs` (populate field)

#### 1.3 Circuit Breaker Enhancement (failover.rs)

**Current**: Linear 5-minute cooldown per provider.

**Change**: Three-state circuit breaker:
- `Closed` → normal operation
- `Open` → reject requests (after N consecutive failures, default 3)
- `HalfOpen` → allow 1 probe request after cooldown
  - Success → `Closed`
  - Failure → `Open` with doubled cooldown (max 10 min)

**Files**: `src/providers/failover.rs`

### 2. Context Management Enhancement

#### 2.1 Tool Result Truncation (tool_pipeline.rs)

**Current**: No size limit on tool results. Relies on downstream compaction.

**Change**: Post-execution truncation in tool pipeline:
- `MAX_TOOL_RESULT_TOKENS = 8000` (~28K chars)
- Truncate at safe boundary (newline for text, object boundary for JSON)
- Append truncation suffix
- Skip truncation for Error results

**Files**: `src/agent_loop/tool_pipeline.rs`

#### 2.2 Bootstrap Budget Tracking (context_budget/)

**Current**: Overhead (system prompt + tools) lumped into used_tokens, not separately tracked.

**Change**:
- Add `overhead_tokens` and `available_for_messages` fields to `ContextPressure`
- Warn when overhead > 30% of budget
- No change to directive decision logic

**Files**: `src/agent_loop/context_budget/mod.rs`, `src/agent_loop/context_budget/pressure.rs`

## Non-Goals

- Auth profile rotation system (new concept, out of scope)
- Session file locking / repair
- Tool policy grouping
- LLM-based semantic compaction in main loop
