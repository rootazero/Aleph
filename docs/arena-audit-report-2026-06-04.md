# Rust Logic Audit Report — src/arena

**Module:** `src/arena` (+ consumers: `builtin_tools/arena.rs`, `gateway/handlers/arena.rs`)
**Date:** 2026-06-04
**Auditor:** rust-logic-audit (static, no diff)
**Follow-up to:** [arena-audit-report-2026-05-31.md](arena-audit-report-2026-05-31.md)

## Summary

Re-audit of the full module (6 files, ~2.3k LOC) plus its four consumers. Since the
2026-05-31 audit closed the runtime-wiring gaps, the module has evolved
(`MAX_PARTICIPANTS` is now 100, not 8). Core logic remains **sound**: the
Created→Active→Settling→Archived state machine is correct, the manager→arena lock
order has no inversions, poisoned locks recover via `into_inner()`, the Kahn's-algorithm
cycle check plus the explicit duplicate-stage guard are correct, NaN confidence is
rejected by `(0.0..=1.0).contains`, and the per-slot / per-arena limits are enforced.

No Critical correctness bugs. Five Warning/Note findings, two of them fixed.

## Findings

| # | Severity | Title | Action |
|---|----------|-------|--------|
| 1 | Warning | `total_steps` never assigned (always 0), surfaced to the LLM as a denominator | Documented (no safe fix) |
| 2 | Warning | `arena_settle` / `arena_query` bypass the `can_merge` handle gate | Documented (needs caller identity) |
| 3 | Warning | `peer` strategy silently drops `stages` | **Fixed** |
| 4 | Warning | `query_arena` slot order non-deterministic | **Fixed** |
| 5 | Note | `ArenaQueryTool` masks read-permission errors via `unwrap_or_default()` | Documented |

### 1. `total_steps` is dead (Warning, not fixed)

`ArenaProgress.total_steps` is only ever written as `0` (`aggregate.rs`), yet it is read
and exposed through `arena_query` (`manager.rs`, `builtin_tools/arena.rs`) and
`snapshot_for_context` (`handle.rs`). The LLM therefore sees `completed_steps=N / total_steps=0`.

Not auto-fixed: `completed_steps` is an unbounded sum of arbitrary per-agent `completed`
counts, so synthesising a total from `participants.len()` / `stages.len()` would routinely
yield `completed > total`, which is *more* misleading than a zero. The correct resolution
is a design decision — add a setter when work is planned, or remove the field. Left to the
module owner.

### 2. Settle/query permission bypass (Warning, not fixed)

`ArenaHandle::begin_settling` enforces `can_merge` ("coordinator only"), but the
`arena_settle` tool and `arena.settle` RPC call `ArenaManager::settle_with_facts` directly,
with no participant/coordinator check; `arena_query` (no `agent_id`) likewise reads any arena.
Any agent that knows the arena_id can settle/inspect an arena it does not coordinate.

Not auto-fixed: tool/RPC params carry no caller identity, so enforcement requires plumbing
the calling agent into the tool layer (outside `src/arena`). Low severity under the
single-user model with UUID ids, but the two settle paths are inconsistent.

### 3. `peer` strategy silently drops `stages` (Warning, FIXED)

`ArenaManifest::build` only reads `stages` in the `"pipeline"` arm, while the gateway
handler accepts a `stages` field for any strategy. A `peer` request carrying stages had them
silently discarded. **Fix:** `build` now returns
`"Pipeline stages are only valid for the 'pipeline' strategy"` when `strategy == "peer"`
and `stages.is_some()` (fail-fast, P7). Regression test added.

### 4. `query_arena` slot order non-deterministic (Warning, FIXED)

`query_arena` iterated `HashMap::values()` (non-deterministic), while the sibling
`snapshot_for_context` already sorts agents. **Fix:** slots are now sorted by `agent_id`
before serialisation, matching `snapshot_for_context`. Regression test added.

### 5. Read-permission errors masked (Note)

`ArenaQueryTool` uses `handle.list_artifacts(..).unwrap_or_default()`, turning a permission
denial into "0 artifacts". Harmless today (every role has `can_read_other_slots`), fragile if
a no-read role is later introduced. No change.

## Files changed

- `src/arena/types.rs` — reject `stages` for peer strategy + test
- `src/arena/manager.rs` — sort `query_arena` slots by `agent_id` + test
