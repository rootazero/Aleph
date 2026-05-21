# Context Budget & Compaction Hardening — Design Spec

**Cycle**: hermes-inspired context-pipeline hardening
**Date**: 2026-05-21
**Branch**: `worktree-context-budget-hardening`
**Net LOC estimate**: +260 / −280

## Problem Statement

A deep comparison against `hermes-agent`'s conversation-history compression and
memory pipeline surfaced confirmed bugs and a dead wire in Aleph's
`src/context/` + harness compaction path. Aleph's infrastructure is largely
complete (preflight pipeline, LLM compactor, budget circuit-breaker, hierarchical
session summaries) — but three correctness bugs make the budget mis-fire, one
fully-built feature is never wired, and one cheap-pass throws away signal hermes
deliberately keeps.

## Confirmed Defects

| # | Defect | Evidence | Impact |
|---|--------|----------|--------|
| **B1** | Budget sensor ignores the system prompt + tool schema | `harness/agent/think.rs:198` — `before_turn(&messages, "", &[])` | System prompt (tens of K tokens) and tool definitions are excluded from pressure → compaction and `FinalReply` fire far too late, risking a hard provider context-overflow before the budget ever reacts. |
| **B2** | Circuit breaker never resets after an effective compaction | `context/budget/mod.rs:348` `notify_compaction_success()` exists but has zero harness callers | After 3 consecutive `CompactAndContinue` turns the breaker trips to `FinalReply` even when compaction is working — long autonomous tasks terminate prematurely. |
| **B3** | Token estimation counts bytes, not characters | `memory/session_compactor/context_window.rs:15` — `content.len()` | CJK text (3 bytes/char) is over-counted ~3× → compaction triggers far too early for Chinese users. The sibling `summary_source.rs:137` already correctly uses `chars().count()`. |
| **F1** | Zero-API-cost summary reuse is dead-wired **and** internally broken | `think.rs:208` — `compact(&mut messages, 0, None)` always passes `None`; `summary_source.rs:48` — `get_raw_by_path_prefix(prefix, "default", 50)` | `SessionSummarySource::try_reuse` reads the d0/d1/d2 summaries `SessionCompactor::post_turn_compress` writes — but the harness compactor never consults them. Worse: `try_reuse` hard-codes `"default"` as the `agent_id` filter, while `post_turn_compress` writes facts under the *real* owning agent id — so even once wired the path would match nothing. F1 must both wire it and fix the agent-id bug. |
| **F2** | Tool-result elision discards all semantic signal | `context/budget/cheap_passes/tool_result_pruning.rs:71` — `[pruned tool_result: <name>, ~<N> tokens]` | hermes keeps an informative one-line hint (`exit 0, 47 lines`); Aleph's generic placeholder erases continuity the model needs to avoid re-running tools. |
| **C1** | Dead code in the touched subsystem | `ConstraintInjector`, `FileContentTracker`, `PressureSensor` — zero production consumers | Maintenance noise. |

## Out of Scope (explicitly deferred)

- **D3 — per-turn O(n) compaction recompute.** The harness rebuilds + recompacts
  the full event log every turn; the compacted form is never persisted.
  Covered by the **Cycle 5 `session-split-compaction`** plan
  (`docs/superpowers/plans/2026-05-21-session-split-compaction.md`), merged to
  main as `89ad74842`. **Closed** — no separate work; this branch's merge of
  main pulls Cycle 5 in.

- **D7 — per-turn memory-retrieval refresh. CLOSED (won't-do).** Initially
  flagged because the system prompt's hybrid-retrieval memory block is
  assembled once per run from that run's user query. Investigation
  (`MemoryContextProvider` → `build_memory_user_message` → `WorkingMemoryAssembler`,
  and the `src/harness/` Think→Act loop) concluded that *automatic per-turn
  refresh is contraindicated by Aleph's architecture*, not a missing feature:
  - **R8 already covers it.** `memory_search`, `recall_context`, `memory_browse`,
    `memory_explore`, `memory_timeline`, `session_search` (the `memory_knowledge`
    tool group) let the LLM re-retrieve memory *on demand* the moment it judges
    its context has drifted — fresh hybrid retrieval, LLM-decided.
  - **Auto-refresh violates R7.** A loop that re-retrieves every turn is the
    harness deciding *for* the LLM when memory is needed — the "越俎代庖" R7 forbids.
  - **Auto-refresh violates R10.** Retrieval + relevance recompute inside the
    per-turn cycle is business logic in the dumb loop.
  - **Cost.** Hybrid retrieval is ~100–3500 ms/turn (embedding + SQLite +
    optional LLM rerank). Within a single run there is only one user query, so
    per-turn re-retrieval buys nothing while spending heavily — the opposite of
    "effective token control."
  - Per-*message* refresh already happens: each new user message is a fresh
    `run()` → fresh `build_system_prompt` → fresh retrieval on that message.

  Conclusion: the correct Aleph design is on-demand recall via tools (R8) plus
  the existing per-message re-retrieval. No code change.

## Design

### B3 — char-count token estimation

`context_window::estimate_tokens` switches `content.len()` →
`content.chars().count()`. This is the single estimator consumed by both
`ContextBudget` and `SessionCompactor`; both want character semantics. ASCII
behaviour is byte-identical, so existing English tests are unaffected.

### B1 — real overhead in the budget sensor

`ContextPressure::compute` and `ContextBudget::before_turn` change their tool
parameter from `tool_defs: &[tools::runtime::ToolDefinition]` to a precomputed
`tool_schema_tokens: usize`. This *decouples* `context::budget` from any tool-def
type (P1 low coupling) and lets the harness count the **actual wire schema**.

The harness (`think.rs`) computes the overhead from `dispatcher_schema()` — the
exact `dispatcher::ToolDefinition` slice sent to the provider — via a small
`estimate_tool_schema_tokens(tools, ratio)` helper, and passes the real
`deps.system_prompt`. The `dispatcher_schema()` fetch (an `Arc::clone`) moves
above the budget check.

### B2 — effective-reset circuit breaker (hermes anti-thrash)

New `ContextBudget::note_compaction_effect(messages, system_prompt, tool_tokens)`:
re-computes pressure on the post-compaction message list and compares it to the
`last_pressure` snapshot saved by `before_turn`. If pressure dropped by at least
`COMPACTION_EFFECTIVE_DROP` (5% of budget), it calls the existing
`record_success()` (resets the breaker); otherwise the breaker keeps counting.

Result: effective compactions oscillate the counter 1→0→1→0 and never trip;
three *consecutive ineffective* compactions still escalate to `FinalReply` — the
correct safety stop, and a direct port of hermes's `_ineffective_compression_count`.

`think.rs` calls `note_compaction_effect` after a successful `compact()`.

### F1 — wire `SessionSummarySource` + fix its agent-id bug

**Bug fix.** `SessionSummarySource::try_reuse` queried
`get_raw_by_path_prefix(prefix, "default", 50)`, but that function's second
argument is the `agent_id` SQL filter (`WHERE path LIKE ?1 AND agent_id = ?2`).
`post_turn_compress` writes the summaries under the real owning agent id, so the
hard-coded `"default"` matched nothing. `SessionSummarySource::new` now takes an
`agent_id` and `try_reuse` passes it through.

**Wiring.** `ContextCompactor` gains a `summary_reuse: Option<SummaryReuse>`
field (`SummaryReuse { backend, agent_id }`, builder `with_summary_reuse`).
`compact()`'s last parameter changes from
`summary_source: Option<&SessionSummarySource>` to `session_id: Option<&str>`;
when reuse is wired and a session id is supplied the compactor builds the
`SessionSummarySource` itself and tries the zero-cost reuse path before the LLM
call. `think.rs` passes `Some(session_id.to_key_string())`.

Boot wiring: `AgentHarnessRunner` gains `memory_backend: Option<MemoryBackend>`;
`agent_init.rs` (which already holds `memory_db`) populates the
`AgentHandlersResult`; `initialize_orchestrator` threads it through;
`harness_bridge.rs` calls `with_summary_reuse(backend, spec.agent)`. `None`
everywhere degrades gracefully to the existing LLM path — opt-in by wiring,
matching `context_compactor`/`context_budget`.

### F2 — informative tool-result elision

`ToolResultPruningStage` keeps a one-line hint: the first non-empty line of the
result (char-capped at 120) plus a line count —
`[pruned tool_result: <name>, ~<N> tokens — <hint> … (<L> lines)]`. The existing
"never grow" guard is retained: if the informative placeholder would not save
tokens the result is left verbatim.

### C1 — dead-code removal

Delete `constraint_injector.rs`, `file_content_tracker.rs`, and the
`PressureSensor` struct from `pressure.rs` (its file-mates `detect_content_ratio`
/ `estimate_tokens_smart` stay — they are live). Remove the corresponding `mod` /
`pub use` lines from `compact/mod.rs`. Verified zero production consumers first.

## R-rule Compliance

| Rule | Check |
|------|-------|
| R3 (Core Minimalism) | No new deps. F1 reuses `SessionSummarySource` + `MemoryBackend`; B2 reuses `record_success`. Net LOC negative. |
| R7 / R10 (LLM sovereignty / Thin Harness) | All changes are scaffolding: overhead arithmetic, a counter-reset, a dead-wire connection. No intent classification, no completion judgement added to the loop. The compaction *decision* stays in `ContextBudget`. |
| P1 (Low Coupling) | B1 *removes* `context::budget`'s dependency on a tool-def type. |

## Testing

| Item | Coverage |
|------|----------|
| B1 | `before_turn` with non-zero `tool_schema_tokens` raises pressure; `compute` overhead includes prompt + tool tokens. |
| B2 | effective compaction → breaker reset → no trip across many turns; 3 ineffective → `FinalReply`. |
| B3 | CJK string estimates ≈ char-count/ratio, not byte-count/ratio. |
| F1 | `compact(session_id=Some)` with seeded d-summaries → `SessionMemoryReuse`; `None` → LLM path. |
| F2 | pruned placeholder contains the first-line hint + line count; oversized-hint case still saves tokens or skips. |
| C1 | `cargo check` clean after removal (proves zero consumers). |

## Implementation Order

1. B3 — `context_window::estimate_tokens` char-count.
2. B1 — `ContextPressure::compute` / `before_turn` signature + `think.rs` overhead wiring.
3. B2 — `note_compaction_effect` + `think.rs` post-compaction call.
4. F2 — `ToolResultPruningStage` informative hint.
5. F1 — `ContextCompactor` backend field + `compact()` signature + boot wiring.
6. C1 — delete dead code.

Each step its own commit. **Merge policy**: stop at "ready" — do not merge to
`main` without explicit instruction.
