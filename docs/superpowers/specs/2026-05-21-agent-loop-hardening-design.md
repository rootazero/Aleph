# Agent Loop Hardening + Tool Wiring Cycle — Design

Date: 2026-05-21
Branch: `worktree-agent-loop-hardening`
Inspired by: hermes-agent (`/Volumes/TBU4/Github/hermes-agent`, Python reference)

## Background

A three-pronged comparison of Aleph against the hermes-agent reference (agent
loop / tool scheduling / intent-routing) surfaced a large defect list. This
cycle implements the **correctness-focused subset**: real bugs, dead-wire
defects (recently-shipped features that are inert on the production path), and
one dead-code removal. Larger items (parallel tool execution, `dispatcher/`
catalog consolidation, `task_router` R7 cleanup) are deferred to their own
cycles.

All harness changes respect CLAUDE.md R10 ("dumb loop"): they are round
scheduling / robustness scaffolding, never LLM-reasoning moved into code.

## Scope — 9 items

### Harness loop bugs

- **C1 — `max_iterations` grace turn.** When the `max_iterations` cap trips on
  a tool-use (Continue) turn, the loop breaks with no terminal LLM call: the
  user gets an empty / mid-thought response. The budget-exhaustion and
  diminishing-returns caps already call `fire_grace_turn`; the most common cap
  does not. Fix: at the `agent.rs` cap site, call a new
  `fire_max_iterations_grace_turn` that re-fetches the session log,
  re-assembles the prompt, and delegates to `fire_grace_turn` with a new
  `GraceReason::MaxIterations`. Skips entirely when the last assistant turn
  already has text — no cost for well-behaved runs.

- **H3 — empty-response bounded retry.** A response with no text, no
  tool_calls and no thinking is misclassified as `TurnState::Done` →
  `Completed`; the user gets nothing and the trace falsely reports success.
  Fix: in `think.rs`, after the LLM call, retry the call up to
  `EMPTY_RESPONSE_RETRIES` (2) times on a truly-empty response. If still empty,
  set `TerminateReason::EmptyResponseExhausted` before the `Done` return so the
  trace is honest. Pure round-scheduling of a known provider failure mode.

- **H4 — within-batch-only tool memo.** The `tool_call_cache`
  `(tool_name, canonical_args)` memo is threaded across every turn of a run,
  so a legitimate cross-turn repeat (`read_file` after `write_file`, any
  time-varying tool) returns the **first call's stale result** without
  re-executing. Fix: create the memo fresh inside each `act()` call so it only
  deduplicates duplicate calls within a single tool batch (matching hermes'
  `_deduplicate_tool_calls`). Removes the `tool_call_cache` parameter from
  `run`, `run_turn_internal`, `run_turn`, and `act`.

- **M9 — grace turn races cancel/timeout.** `fire_grace_turn` calls
  `llm.process()` directly, bypassing `race_llm_call`. A hung provider on the
  grace turn hangs the harness forever and ignores user cancel. Fix: thread
  `parent_cancel` into `fire_grace_turn` and route the call through
  `race_llm_call`.

- **M10 — grace turn token breakdown drift.** `fire_grace_turn` updates
  `total_tokens` but not `token_breakdown`, violating the documented
  `breakdown.total() == total_tokens()` invariant. Fix: add the one missing
  `accumulate_token_breakdown` call.

### Tool scheduling dead-wires

- **CRITICAL-1 — per-tool budget metadata.** `ScopedToolService` (the
  harness's production `ToolService`) hardcodes `ToolDefinitionMetadata::default()`
  in `loop_tool_to_definition`, `list()` and `dispatcher_schema()`. So
  `act.rs`'s `describe(...).metadata.max_duration_ms` is always `None` — the
  recently-shipped per-tool wall-clock budget never fires in production. Fix:
  populate metadata from `tools::budget::builtin_tool_budget_ms` +
  `tools::retry::is_idempotent_builtin_name` via one shared helper, exactly as
  `BuiltinHandler::definition()` already does.

- **MED-2 — propagate `retryable`.** `act.rs` builds the tool-error trace
  event with `retryable: false` hardcoded, discarding `ToolError::is_retryable()`.
  Fix: capture `is_retryable()` before stringifying the error and pass it into
  the trace `ToolResult::Error`.

### Routing bug

- **H1 — group session-key fallback.** In the zero-config (no route bindings)
  path, `resolve_session_key_with_agent` builds a group key via
  `SessionKey::peer(agent, "{channel}:group:{conv}")`, which produces a
  `DirectMessage` variant with an empty channel — a group chat persistently
  mistyped as a DM, with a key that differs from the bound-route path's
  `SessionKey::group(...)`. Fix: build groups with
  `SessionKey::group(agent, channel, PeerKind::Group, conversation_id)`,
  matching `resolve_route`.

### Dead-code cleanup

- **M1 — delete dead `intent/` module.** `src/intent/` (~436 lines) is
  types-only; `IntentResult` / `ExecuteMetadata` / `DetectionLayer` /
  `TaskCategory` have zero consumers. The only live symbol, `DirectToolSource`,
  is used in `command_handler.rs` purely as a 4-value string tag. Fix: inline
  the 4 string literals, delete `src/intent/`, drop `pub mod intent` from
  `lib.rs`.

## Deferred (noted for follow-up cycles)

- Parallel tool execution + cancellation threading into `act()` (HIGH-3 / M6).
- `ScopedToolService` health-gate wiring into `build_request_tool_service`
  (CRITICAL-2 — decide wire vs delete).
- `dispatcher/tool_index/` deletion (~125 KB dead, R7-borderline).
- `routing/` task-router pipeline R7 cleanup (`llm_classifier` /
  `composite_router` / `rules` / `task_router`, ~1000 lines).
- Unpaired `ToolCallRequested` on session-store emit failure (M7).

## Testing

Each fix ships with a focused unit test. Existing harness tests
(`max_iterations_stops_runaway_loop` in particular) are updated to reflect the
new grace-turn call count. `cargo test -p alephcore --lib` for harness/tools;
gateway tests for H1.
