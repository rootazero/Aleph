# Per-tool Budget + DiminishingReturnsDetector Wiring — Design Spec

**Cycle**: Cycle 3 — Long-task hardening follow-up
**Date**: 2026-05-21
**Scope**: Item 2 of 4 deferred from [Cycle 2](./2026-05-20-long-task-hardening-design.md)
**Net LOC estimate**: +400

## Problem Statement

Two long-task reliability gaps surfaced during Cycle 2 audit:

1. **`turn_timeout` is opt-in with no per-tool granularity.** `turn_timeout_secs` defaults to `None` in production config (`src/config/types/phase6_wiring.rs:38`). When unset, a single misbehaving tool (hung HTTP, blocked shell exec, stalled MCP server) can block the harness loop indefinitely — only the LLM provider's own client timeout protects the next Think turn. Even when set, one uniform value can't fit both fast read-only tools (`memory_search` should return in ~2s) and legitimately slow tools (`web_fetch` of large pages, `markdown_skill` shell execution may need 30–60s).

2. **`DiminishingReturnsDetector` is fully unwired.** `ContextBudget::after_turn()` (`src/context/budget/mod.rs:362`) is called only from its own unit tests — a grep across all of `src/` returns zero production callsites. `LoopDirective::StopDiminishing` is a defined enum variant that is **never emitted**. When the model spins unproductively (e.g., 4 turns of tool_use with no text emission, or repeated failure → retry → failure loops), there is no early stop signal; only `turn_budget` / context-window pressure rescue the run.

## Scope

### In Scope (Cycle 3)

- **A. Per-tool execution budget metadata + static defaults**
  - `ToolDefinitionMetadata.max_duration_ms: Option<u64>` field
  - `src/tools/budget.rs` const lookup table for builtin tools
  - Resolution at `src/harness/agent/act.rs`: tool metadata > global `turn_timeout` > unbounded

- **B. DiminishingReturnsDetector live wiring**
  - Call `ContextBudget::after_turn(metrics)` from `src/harness/agent/think.rs` after the Act phase
  - Route `LoopDirective::StopDiminishing` through the existing Cycle 2 grace-turn infrastructure
  - Keep the current weak `productive = executed > 0` heuristic — heuristic upgrade is explicitly out of scope

### Out of Scope (deferred to future cycles)

- Cost-aware productive heuristic (add `text_output_tokens` / `tool_output_tokens` inputs, cross-reference token cost vs progress)
- MCP server-declared timeout inheritance into `max_duration_ms`
- Markdown-skill inner-timeout vs outer-budget reconciliation (currently both layers coexist independently — observe first)
- Telegram / Channel interface-layer LLM-call budgets

## Design A — Per-tool Execution Budget

### Metadata field

Extend `ToolDefinitionMetadata` in `src/tools/service.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinitionMetadata {
    #[serde(default)] pub hidden_from_llm: bool,
    #[serde(default)] pub requires_approval: bool,
    #[serde(default)] pub tags: Vec<String>,
    #[serde(default)] pub idempotent: bool,
    /// Per-tool wall-clock budget. `None` falls back to the harness-wide
    /// `turn_timeout`; if both are `None`, the call is unbounded.
    #[serde(default)] pub max_duration_ms: Option<u64>,
}
```

Field is non-breaking: `#[serde(default)]` lets existing serialized metadata round-trip unchanged.

### Static defaults table

New file `src/tools/budget.rs` — parallels the Cycle 2 `IDEMPOTENT_BUILTIN_TOOLS` pattern in `src/tools/retry.rs`:

```rust
/// Wall-clock budget per builtin tool. Tools omitted fall back to the
/// harness-wide `turn_timeout`. Values reflect empirical p99 of well-behaved
/// invocations plus a margin.
pub const BUILTIN_TOOL_BUDGETS_MS: &[(&str, u64)] = &[
    // Read-only / pure query — should be fast
    ("memory_search",   5_000),
    ("memory_browse",   5_000),
    ("memory_timeline", 5_000),
    ("memory_explore",  5_000),
    ("recall_context",  5_000),
    ("session_search",  5_000),
    ("user_profile",    3_000),
    ("skill_status",    3_000),
    ("skill_reader",    5_000),
    ("list_tools",      2_000),
    ("get_tool_schema", 2_000),
    ("note_orient",     3_000),
    ("note_schema",     3_000),
    // Legit slow
    ("search",         20_000),
    ("web_fetch",      30_000),
    ("markdown_skill", 60_000),
];

pub fn builtin_tool_budget_ms(name: &str) -> Option<u64> {
    BUILTIN_TOOL_BUDGETS_MS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ms)| *ms)
}
```

`BuiltinHandler::definition()` reads this table to populate `max_duration_ms`, identical in shape to the idempotent lookup added in Cycle 2.

### Resolution at the exec site

In `src/harness/agent/act.rs` (around line 128):

```rust
// Resolve the effective wall-clock budget for THIS tool call.
let tool_def = self.deps.tools.describe(&call.name).await;
let per_tool_budget = tool_def
    .as_ref()
    .and_then(|d| d.metadata.max_duration_ms)
    .map(Duration::from_millis);
let effective_budget = per_tool_budget.or(self.deps.turn_timeout);

let exec_fut = self.deps.tools.execute(&call.name, call.arguments.clone());
let exec_result = match effective_budget {
    Some(budget) => {
        let started_call = Instant::now();
        match tokio::time::timeout(budget, exec_fut).await {
            Ok(inner) => Ok(inner),
            Err(_) => Err(HarnessError::StalledTurn {
                phase: TurnPhase::Act { tool_name: call.name.clone() },
                elapsed: started_call.elapsed(),
            }),
        }
    }
    None => Ok(exec_fut.await),
};
```

`describe()` is a cheap lookup against the in-memory tool registry (already cached). The added per-call overhead is one async map probe.

**No new error categories.** Timeout still surfaces as `HarnessError::StalledTurn { phase: Act { tool_name } }`. Downstream trace, retry guard, and final-reply logic are unchanged.

## Design B — DiminishingReturnsDetector Wiring

### Current state

`src/harness/agent/think.rs:142` already calls `context_budget.before_turn(...)` and routes `LoopDirective::{CompactAndContinue, FinalReply}` correctly (Cycle 2 grace turn lives in `FinalReply` branch at line 174). The Act phase runs in the same function around line 401, recording `executed` and `requested` counts.

`after_turn()` exists in `src/context/budget/mod.rs:362` and is fully tested in isolation, but is never invoked from production code.

### Change

After the Act phase completes, before returning from the turn function, call `after_turn` and route `StopDiminishing` through the existing grace-turn path:

```rust
// src/harness/agent/think.rs — immediately after `executed` is computed
// (around line 418, in the `else` branch where tools ran)

let output_tokens = response
    .usage
    .as_ref()
    .map(|u| u.output_tokens as usize)
    .unwrap_or(0);

let directive_after = if let Some(budget) = self.deps.context_budget.as_ref() {
    let mut guard = budget.lock().await;
    Some(guard.after_turn(crate::context::budget::TurnMetrics {
        output_tokens,                  // OUTPUT only — required by detector threshold semantics
        tool_calls: requested,
        productive: executed > 0,
    }))
} else {
    None
};

if matches!(directive_after, Some(LoopDirective::StopDiminishing)) {
    self.hit_limit.store(true, Ordering::Relaxed);
    self.fire_grace_turn(
        &mut events,
        &messages,
        callback,
        GraceReason::Diminishing,
    ).await;
    return Ok((TurnState::Done, executed, true));
}
```

### Grace-turn helper extraction

Refactor the inline grace-turn block at `src/harness/agent/think.rs:174-220` (added in Cycle 2 for `FinalReply`) into a private helper:

```rust
enum GraceReason {
    Budget,        // FinalReply path — "out of context budget, summarize"
    Diminishing,   // StopDiminishing path — "no measurable progress, summarize"
}

impl GraceReason {
    fn nudge(self) -> &'static str {
        match self {
            Self::Budget => GRACE_NUDGE_BUDGET,
            Self::Diminishing => GRACE_NUDGE_DIMINISHING,
        }
    }
}

async fn fire_grace_turn(
    &self,
    events: &mut Vec<SessionEventRecord>,
    messages: &[UnifiedMessage],
    callback: &dyn LoopCallback,
    reason: GraceReason,
) { /* shared body */ }
```

The Cycle 2 `GRACE_NUDGE` const becomes `GRACE_NUDGE_BUDGET`; new sibling `GRACE_NUDGE_DIMINISHING` is added.

### Productive heuristic — explicit deferral

`productive: executed > 0` is too loose: it treats any turn with at least one tool call as productive, even if the same failing call repeats. This means `DiminishingReturnsDetector` will rarely trigger in practice during Cycle 3.

Trade-off accepted: wire first, observe trace output, then upgrade the heuristic in a follow-up cycle. The alternative (upgrade + wire in one cycle) inflates scope and couples two unrelated test surfaces. A trace counter (number of `after_turn` calls that returned `Continue` vs `StopDiminishing` in real sessions) feeds the future-cycle decision.

## R-rule Compliance

| Rule | Check |
|------|-------|
| R3 (Core Minimalism) | A adds ~110 LOC (budget.rs + service.rs field + act.rs resolution); B reuses existing infra, adds ~80 LOC for wiring + helper. No new dependencies. |
| R7 (LLM Sovereignty) | Neither feature replaces LLM reasoning. A is a wall-clock guard; B uses an existing detector and an existing directive variant. |
| R10 (Thin Harness, Dumb Loop) | No new decision categories. A stays in the existing `StalledTurn` error path; B reuses `StopDiminishing` (defined but never emitted before this cycle). Grace-turn helper extraction reduces duplication, not adds intelligence. |

## Testing

| Layer | File | Coverage |
|-------|------|----------|
| Unit | `src/tools/budget.rs::tests` | `builtin_tool_budget_ms("memory_search") == Some(5_000)`; unknown name returns `None`; table size matches expected count |
| Unit | `src/harness/agent/act.rs::tests` | per-tool metadata wins over global; both `None` = unbounded; timeout surfaces `StalledTurn { phase: Act { tool_name } }` |
| Unit | `src/harness/agent/think.rs::tests` | `fire_grace_turn` helper: `GraceReason::Budget` uses `GRACE_NUDGE_BUDGET`; `GraceReason::Diminishing` uses `GRACE_NUDGE_DIMINISHING` |
| Integration | `src/harness/tests/task10_wiring.rs` | `after_turn` is called once per turn when `context_budget` is wired; `before_turn` and `after_turn` directives compose correctly |
| Integration | same file | `StopDiminishing` triggers grace turn with the diminishing nudge; persisted `AssistantMessage` event has non-empty text; `hit_limit == true` |
| Integration | same file | Per-tool timeout fires before global `turn_timeout` when both are set (e.g., metadata = 100ms, global = 60s, tool sleeps 200ms → `StalledTurn`) |

Target: 6 new unit tests + 3 new integration tests. All Cycle 2 task10_wiring tests preserved.

## Risks

| ID | Risk | Mitigation |
|----|------|------------|
| R1 | New builtin tool added without updating `BUILTIN_TOOL_BUDGETS_MS` | Falls back to global / unbounded (safe); code-review checklist item; same risk profile as Cycle 2 idempotent table |
| R2 | `productive: executed > 0` is too weak — detector may never trigger | Wire first, measure trip frequency via trace, upgrade heuristic in a follow-up cycle |
| R3 | MCP / Extension / Markdown-skill tool handlers don't populate `max_duration_ms` | Inherit global fallback; explicit out-of-scope item |
| R4 | `markdown_skill` already has its own `tokio::time::timeout` at `executor.rs:97` — outer budget wraps inner | Two timeouts compose (outer fires first cancels inner); add a test for nested-cancel correctness |

## Implementation Order

1. **A.1** Add `max_duration_ms` field to `ToolDefinitionMetadata` + tests for round-trip
2. **A.2** Create `src/tools/budget.rs` with const table + lookup fn + unit tests
3. **A.3** Wire `BuiltinHandler::definition()` to populate `max_duration_ms` from the table
4. **A.4** Modify `act.rs` resolution; add unit tests for the three cases (per-tool, fallback, unbounded)
5. **B.1** Extract `fire_grace_turn` helper from existing FinalReply block; add `GraceReason` enum; preserve all Cycle 2 tests green
6. **B.2** Add `after_turn` call after Act phase in `think.rs`; route `StopDiminishing` through `fire_grace_turn`
7. **B.3** Add integration tests in `task10_wiring.rs` for both wirings

Each step lands as its own commit. Worktree branch: `worktree-feat+tool-budget-cost-breaker`.

## Reference

- Cycle 2 design (precedent for static-const-table pattern): [`2026-05-20-long-task-hardening-design.md`](./2026-05-20-long-task-hardening-design.md)
- Cycle 2 memory entry: `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_long_task_hardening_cycle2.md`
- `LoopDirective` enum: `src/context/budget/mod.rs:81`
- `TurnMetrics`: `src/context/budget/mod.rs:98`
- Existing `before_turn` callsite: `src/harness/agent/think.rs:142`
- Existing grace-turn block: `src/harness/agent/think.rs:174-220`
- Existing tool exec site: `src/harness/agent/act.rs:128`
