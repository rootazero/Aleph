# Goal-Loop Observability & Unattended Trust — Design

**Date:** 2026-06-13
**Branch:** `goal-loop-observable`
**Scope:** Harden Aleph's standing-goal autonomous-continuation loop (the "Ralph loop") so unattended pursuit is **observable** and **trustworthy**, informed by a gap analysis against `codex` (Rust agent loop) and `hermes-agent` (Python routines).

## Context

Aleph's Ralph loop already exists end-to-end and is mature (`goal` tool → `Goal`/`GoalStore`/`goal::global()` → `StandingGoalLayer` → `execute.rs` continuation driver → `goal_pursuit.rs` pure decisions → `GoalLessonsPromoteStage`). This work does **not** rebuild it — it closes three observed gaps.

### Reference synthesis (gap analysis)

Both references **validate** Aleph's architecture, and Aleph is **ahead** on several axes:

| Axis | codex | hermes | Aleph | Verdict |
|------|-------|--------|-------|---------|
| Loop driver | Stop-hook + `stop_hook_active` re-entry flag, zero Rust judgment | No Stop-hook (cron-extern + tool-loop, unmerged) | Stop-hook continuation + `continuations_used`/`max_iter` counter | Aleph ≈ codex, ahead of hermes |
| Completion feedback | `block`+reason → continuation_fragment | structural (no tool call = done) | gate veto → `gate_failure_prompt` + lessons | three-way isomorphic |
| Time/iter/token caps | none | iteration + repeat-count | `deadline_ms` + `max_iter` + `token_budget` | **Aleph ahead** |
| New-input handling | pre-empts (kills turn) | — | FIFO queue (does not kill in-flight goal) | **Aleph better** |
| Continuation visibility / multi-channel | single CLI stream | cron `deliver` field | **output dropped** | **Aleph-unique gap (G1)** |
| Lessons capture | — | anti-pattern list ("never persist negative tool claims") | free append, **no guard** | **adopt hermes hardening (L1)** |
| Continuation failure | interactive (visible) | cron logs error, keeps firing | **silent stall** | **adopt fail-closed notify (G3)** |

## Problems

- **G1 — Autonomous continuation is invisible.** `spawn_continuation_run` (execute.rs) runs the continuation with a `CollectingEventEmitter` (buffers, never broadcasts) and drops the result (logs only on error). A user who sets `pursuit_max_iterations=10` and walks away sees **nothing** in real time; the work only surfaces on their next manual turn via the `<standing_goal>` line + session history. This violates R5 ("AI comes to you" / multi-channel push) and R6. Neither reference has multi-channel push — this is Aleph's differentiator left unwired.
- **G3 — Continuation failure stalls the loop silently.** The continuation hook (execute.rs) lives in the run's `Ok` arm; a continuation run that errors (e.g. transient rate-limit) does not re-fire, leaving the goal `Active` with no in-flight run and **no notification**. Pursuit silently halts until the user returns.
- **L1 — Lessons can poison future iterations.** `Goal.lessons` is re-injected into every continuation prompt. The `lesson` capture surface gives the model no guidance, so it can record environment-specific failures or negative tool claims ("X is broken") that, per hermes's `background_review` anti-pattern list, "harden into refusals the agent cites against itself for months."

## Design

### G1 — Surface continuation runs (reuse existing infra)

Mirror the proven `subagent_announce.rs` pattern (and `handlers/agent.rs`): build the continuation's emitter as a **`GatewayEventEmitter`** (broadcasts to the gateway event bus → Panel + `aleph watch` see it live) wrapped in **`OriginFanoutEmitter`** (delivers the final reply to the session's bound origin channel — Telegram/Slack — when `AgentInstance::origin_route` resolves one).

- `ContinuationDeps` (engine.rs): add `event_bus: Option<Arc<GatewayEventBus>>` (the engine already holds it).
- Boot wiring (`agent_init/mod.rs`): populate `event_bus` (in scope at the `continuation_cell.set` site).
- `spawn_continuation_run` (execute.rs): take `event_bus`; inside the spawned task (after `cont_agent` resolves), build base = `GatewayEventEmitter` if `event_bus` is `Some` else `CollectingEventEmitter` (test/early-boot fallback), then wrap with `OriginFanoutEmitter` when `cont_agent.origin_route(&session_key)` and `origin_fanout::channel_registry()` are both `Some`.
- `RunComplete.final_response` is populated by run_loop's event_drain regardless of emitter, so delivery fires correctly. Delivery failures are swallowed by `OriginFanoutEmitter` (hermes "split delivery vs success" — a delivery failure must never mis-mark the goal).

### G3 — Notify + Block on continuation failure (fail-closed)

In `spawn_continuation_run`'s spawned task, when `adapter.execute` returns `Err(e)`:
- If `e` is `ExecutionError::Cancelled` → log only (the user interrupted; same rationale as the post-run rescue's `Completed`-only guard). Do **not** block.
- Otherwise → load the goal via `goal::global()`, transition it to `Blocked` with a note naming the failure, persist, and deliver a one-line failure notice to the origin channel (reusing the resolved `origin_route` + `channel_registry`). This ends the silent stall and tells the user pursuit halted.

### L1 — Lesson capture guidance (prompt-only, R9)

Extend the `lesson` field doc on `GoalArgs` (the schema description the model reads) and the `goal` tool description with hermes's anti-pattern guidance: capture durable, transferable insights; do **not** record environment-specific failures, transient resolved errors, or negative "tool/codebase is broken" claims. Pure prompt content — no loop change.

## Out of scope

- **G2 — Real token-budget enforcement.** `token_budget` is currently a soft prompt hint (`should_continue` is passed `tokens_now = 0`; `tokens_at_start = 0`). codex enforces via API `TokenUsage`. Threading a live cumulative count into the hook requires changing `run_agent_loop`'s return type (`Result<String, _>` → carry tokens) — invasive, and risky under the "no cargo check" constraint. The current behavior is already honestly documented as soft. **Deferred** to a future round.
- Review-after-done sub-agent (codex `/review`): conflicts with Aleph's objective-gate philosophy (zero-LLM exit-code gate). Deferred.
- Recurring/scheduled goals (hermes routines): Aleph deliberately separates cron (distinct session key); `goal_pursuit.rs` documents why. Out of scope.

## Test plan

(Authored as `#[cfg(test)]` units; per the round's resource constraint, **not** run via cargo here — verified by inspection + type-correctness.)

- `goal_pursuit` / `goal` tests already cover the pure decisions; L1 changes only doc strings (no behavior).
- G1/G3 live in gateway wiring (`spawn_continuation_run`), exercised by the existing engine integration tests; the emitter fallback (`event_bus = None` → `CollectingEventEmitter`) keeps all current tests behavior-identical.
- Regression invariant: when `event_bus` is `None` and no origin channel is bound, behavior is byte-identical to today (collect-and-drop), so non-production/test paths are unaffected.
