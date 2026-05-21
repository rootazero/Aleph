# Hermes-Inspired Summary Wiring — Design

**Date**: 2026-05-20
**Branch**: `worktree-hermes-summary-wiring`
**Driver**: gap analysis vs `/Volumes/TBU4/Github/hermes-agent` for two pipelines:
1. Conversation message flow
2. Agent final-result summary output

## Why

Two parallel deep audits (Aleph vs Hermes) found the conversation message flow is
mostly aligned — prompt layers, callback fan-out, trace_sink, history-compression
all wired. The **summary output** pipeline tells a different story: the data the
gateway needs is already produced (per-tool `duration_ms` in
`LoopTraceEvent::ToolCallCompleted`; token breakdown in `TokenUsage`; multiple
`hit_limit` causes in `agent.rs`; `EnhancedRunSummary` defined in
`shared/protocol/src/events.rs:479` with `tool_summaries` + `errors` slots) **but
none of it reaches `FlowOutcome` or `RunComplete`**. Channels render a
near-empty `RunSummary { total_tokens, tool_calls, loops, final_response }`.

This is a textbook "缺连线" — match R10 thin-harness philosophy: do not invent
new intelligence; route data that already exists.

## Non-Goals (sharpen scope)

- ❌ No explicit "summary turn" — that would violate R7 (LLM Sovereignty). The
  model stops naturally; we only surface signals the loop already records.
- ❌ No live pricing API. Cost is best-effort against a hardcoded table.
- ❌ No breaking schema changes to `RunSummary` (legacy clients keep working);
  enrichment lives on `EnhancedRunSummary` which is already declared.
- ❌ No harness-line-count growth beyond what already lives there
  (R10 keeps `src/harness/` ≤ 9 files / ~1500 lines).

## Architecture

### Data flow today
```
loop exits → SessionCompleted{final_text:None} trace
           → harness_bridge re-reads session
             → 8-layer JSON fallback on final AssistantMessage
             → FlowOutcome{hit_limit:bool, total_tokens:u32, tool_calls_made:u32}
           → broadcast FlowStreamEvent::Complete(outcome)
           → event_drain → RunSummary{total_tokens, tool_calls, loops, final_response}
           → channels render basic text
```

### Data flow after
```
loop exits → SessionCompleted{final_text:Some, tool_timeline, terminate_reason, token_breakdown} trace
           → on_complete_with_outcome(&FlowOutcome) — no re-read needed
             → FlowOutcome{terminate_reason, token_breakdown, tool_timeline, duration_ms, estimated_cost, …}
           → broadcast Complete(outcome) via callback (single source)
           → event_drain → EnhancedRunSummary{…tool_summaries, errors, duration_ms…}
           → channels call FlowOutcome::render(style)
```

## Phases

| # | Phase | File(s) | Lines | Risk |
|---|-------|---------|-------|------|
| P1 | Expand FlowOutcome | `src/orchestrator/dispatch.rs` | +100 | none — new fields are additive |
| P2 | SessionCompleted self-sufficient | `src/harness/agent.rs` + `trace.rs` + `shared/protocol/src/events.rs` | +120 | low — new fields default-serialized |
| P3a | event_drain → EnhancedRunSummary | `src/gateway/execution_engine/event_drain.rs` | +60 | low — `EnhancedRunSummary` already exists |
| P3b | summary_format render | NEW `src/orchestrator/summary_format.rs` | +250 | none — pure formatting layer |
| P4 | callback.on_complete_with_outcome | `src/harness/callback.rs` + `orchestrator/harness_bridge/callback.rs` | +60 | medium — trait signature change (default impl preserves bw-compat) |
| P5 | Drop 8-layer JSON fallback | `src/orchestrator/harness_bridge.rs:336-394` | -60 | low — P2 makes trace authoritative |
| Cost | Static price table | NEW `src/pricing.rs` | +200 | none — opt-in feature |
| Ch | Channel render hookup | 4 files | +120 | low — per-channel additive |

**Estimated total**: ~870 new lines, ~60 deleted, ~5–8 commits. Touches no
harness-internal core files (`agent/think.rs`, `agent/act.rs`, `chain_context.rs`,
`loop_callback.rs`) — R10 constraint upheld.

## New Types

### `TerminateReason`
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TerminateReason {
    Completed,
    HitMaxIterations { used: u32 },
    StallTimeout { elapsed_ms: u64 },
    TurnTimeout { phase: String, elapsed_ms: u64 },
    ConsecutiveFailureCap { consecutive: u32 },
    VerifierVeto { vetos: u32 },
    Cancelled,
}
```
Mapped directly from `agent.rs` exit branches (lines 195/205/229/263/284/293).

### `TokenBreakdown`
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenBreakdown {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
    pub cache_creation: u32,
    pub reasoning: u32,
}
```
Populated by accumulating each `ProviderUsage` trace event.

### `ToolInvocation`
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}
```
Sourced from `LoopTraceEvent::ToolCallCompleted` — data is already there.

### `CostEstimate` (in `src/pricing.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub usd: f64,
    pub status: CostStatus,
    pub provider: String,
    pub model: String,
}
#[serde(rename_all = "snake_case")]
pub enum CostStatus { Complete, PartialMissingPrice, Unknown }
```

## Backwards Compatibility

- `FlowOutcome.hit_limit: bool` REMOVED; replaced by `terminate_reason`. Callers
  that asked `outcome.hit_limit` flip to
  `matches!(outcome.terminate_reason, TerminateReason::Completed | Cancelled)
  .not()` — refactor done in this PR (8 known call sites).
- `RunSummary` schema **unchanged**. `RunComplete.summary: RunSummary` keeps
  byte-compat — `EnhancedRunSummary` is the additive layer that channels can
  opt into.
- `aleph_protocol::AgentTraceEvent::SessionCompleted` gains `Option<…>` fields
  with `#[serde(default)]` — replays of old trace blobs still parse.

## Verification Plan

1. After each phase, `cargo check -p alephcore` + targeted `cargo test --lib`.
2. After P3a, run `tests/gateway_chat_streams_tool_events.rs` and
   `tests/gateway_chat_preserves_hit_limit.rs` to confirm channel-level events
   stay green.
3. Final pass: full `cargo test --lib`, diff failures vs baseline
   ([memory] project_baseline_test_failures: 19 known + 1 deadlock). New
   failures = blocker; baseline failures = ok.
4. Live smoke: `cargo run --bin aleph-server` + one chat turn through CLI;
   assert summary footer shows tool table + cost.

## Out of Scope (deferred)

- Per-channel truncation policy beyond the 4000-char Telegram cap copied from
  hermes — a future cycle if the resulting tool table grows huge.
- Cost rollup across sub-agent chains. Phase-1 estimate is per-run.
- Trace SessionCompleted version-bumped JSON schema export. Existing
  schemars derives remain authoritative.
