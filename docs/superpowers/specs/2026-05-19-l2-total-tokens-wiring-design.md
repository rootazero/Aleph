# L2 — `total_tokens` Wiring Design

**Date:** 2026-05-19
**Status:** Approved (design)
**Scope:** Long-task reliability backlog item L2 (final cleanup cycle, M4/L1/L2)

## Problem

`RunSummary.total_tokens` and several sibling structs report a hardcoded `0`
instead of real provider-reported token usage. A grep of `total_tokens: 0`
across `src/` found 9 literal sites. The audit filed this as L2 ("low",
observability/correctness gap — not dead code; the field exists and is meant
to carry a value).

## Investigation Findings

### Real token data exists; no runtime accumulator does

Every LLM call returns `ProviderResponse.usage: Option<TokenUsage>`
(`src/providers/adapter.rs:210`). `TokenUsage` carries five components:
`input_tokens`, `output_tokens`, `cache_read_tokens`,
`cache_creation_tokens`, `thinking_tokens` (all `u32`/`Option<u32>`).

`MeteringProvider` (`src/providers/metering.rs`) wraps the provider and emits
a `LoopTraceEvent::ProviderUsage` per call into the `TraceSink`. Those events
are **emitted one-by-one and never summed at runtime**. `context/budget/`
tracks *estimated* context-window pressure (text-length heuristics), not
provider-reported actuals — so there is no existing accumulator to reuse.

### The 9 sites split into two clusters

**Cluster A — harness / orchestrator path (the genuine gap):**

| Site | What it is |
|------|-----------|
| `harness/agent/think.rs:278,329` | `LoopTraceTurnMetrics.total_tokens` — per-turn |
| `harness/agent.rs:297,327` | `LoopTraceEvent::SessionCompleted.total_tokens` — cumulative |
| `orchestrator/harness_bridge.rs` | `FlowOutcome` built via `..Default::default()` → `total_tokens = 0` |
| `agents/subagent_spawner/mod.rs:477` | `LoopRunResult.total_tokens` — cumulative for a subagent run |

`event_drain.rs:195` already reads `u64::from(outcome.total_tokens)` — it is
**not** a literal-`0` site; it auto-fills once `FlowOutcome` is populated.

**Cluster B — gateway `ExecutionEngine` finalize (NOT a token gap):**

| Site | Verdict |
|------|---------|
| `gateway/execution_engine/simple.rs:139` | `SimpleExecutionEngine` is the **simulated/fallback** engine (`agent_init.rs:1729`, the `Mode: Simulated` branch when no API key). No real LLM → `0` is semantically correct. |
| `gateway/execution_engine/fast_path.rs:49,130` | `finalize_fast_path_*` serves the **L0 slash-command fast path** that bypasses the agent loop. `execute_slash_command_fast_path` (`slash_command.rs:73`) makes **zero provider calls**; skills/custom commands that need an LLM `Fallthrough` to the agent loop instead. Deterministic → `0` is correct. |
| `gateway/execution_engine/execute.rs:325` | Reached *after* `run_agent_loop`. But the drain inside `run_agent_loop` (`helpers.rs:182-188`) already emits a `RunComplete` from `FlowOutcome` via `event_drain::emit_complete` when it sees `FlowStreamEvent::Complete`. So `execute.rs:321`'s `RunComplete` is a **redundant second emission** — a pre-existing double-emit, out of L2 scope. |

**Conclusion:** the only real token-wiring gap is Cluster A. Cluster B needs
**no token plumbing** — 3 sites are correct-by-design (no LLM), 1 is a
redundant emission. Cluster B gets clarifying comments only.

## Approach

### Chosen: harness token accumulator, mirroring `hit_limit`

`AgentHarness` already has `hit_limit: AtomicBool` + `hit_limit()` accessor +
`reset_hit_limit()`. After a run, `harness_bridge.rs:381` reads
`harness.hit_limit()` into `FlowOutcome`, and `subagent_spawner/mod.rs:365`
reads it into `LoopRunResult` (both retain an `Arc<AgentHarness>` handle past
the run for exactly this). Token accounting copies this proven pattern.

### Rejected alternatives

- **Post-hoc replay from the session log.** `harness_bridge`'s
  `extract_run_result` already replays `SessionEvent`s for
  `iterations`/`tool_calls_made`. But `ProviderUsage` goes to the `TraceSink`,
  not the session event log — summing it post-hoc would require persisting
  usage as session events first. Larger change, rejected.
- **Accumulate inside `MeteringProvider`.** It already sees every usage, but
  has no handle to a per-run summary; threading one in couples the provider
  layer to the harness summary. Rejected.

## Design

### Cluster A changes (6 logical edits)

1. **`harness/agent.rs`** — add `total_tokens: AtomicU64` field to
   `AgentHarness`; add `total_tokens()` accessor; reset it wherever
   `reset_hit_limit()` is already called (fold into that reset path so a
   fresh run starts from 0).

2. **`harness/agent/think.rs`** — the provider `response` is already in
   scope. Compute the turn's token total and:
   - `fetch_add` it into the harness accumulator (`Ordering::Relaxed`);
   - fill `LoopTraceTurnMetrics.total_tokens` at both construction sites
     (`:278` `zero_metrics`, `:329` the tool-calls branch) — compute once,
     use in all three branches.

3. **`harness/agent.rs`** — `LoopTraceEvent::SessionCompleted.total_tokens`
   (`:297`, `:327`) = `self.total_tokens.load(Relaxed)`.

4. **`orchestrator/harness_bridge.rs`** — `FlowOutcome.total_tokens =
   u32::try_from(harness.total_tokens()).unwrap_or(u32::MAX)` (saturating —
   note plain `as u32` truncates, so `try_from` is required). Mirrors `:381
   hit_limit: harness.hit_limit()`. Set the field explicitly in the
   `FlowOutcome { .. }` literal instead of leaving it to `..Default::default()`.

5. **`agents/subagent_spawner/mod.rs`** — after the run, read
   `harness.total_tokens()`, thread it into `extract_run_result` alongside
   `hit_limit`, and fill `LoopRunResult.total_tokens`. Mirrors `:365`.

6. **`gateway/execution_engine/event_drain.rs:195`** — **no change**.
   Auto-fills once #4 lands.

### Token-total semantics

`total_tokens` = `input_tokens + output_tokens + cache_read_tokens +
cache_creation_tokens` ("all tokens processed/billed for this run"). A pure
helper carries this:

```rust
fn turn_token_total(usage: &Option<TokenUsage>) -> u64
```

`None` usage → 0; `None` cache components → 0. `thinking_tokens` is excluded
because Anthropic's `output_tokens` already includes thinking tokens (adding
it would double-count).

### Type boundaries (no type cascade)

- Harness accumulator: `AtomicU64` (a long run's cumulative total can exceed
  `u32::MAX`).
- `FlowOutcome.total_tokens` stays `u32` — saturating `u32::try_from(..)`
  at the boundary.
- `LoopRunResult.total_tokens` (`usize`) and `RunSummary.total_tokens`
  (`u64`) unchanged.

### Cluster B changes (comments only)

Add a one-line clarifying comment at `simple.rs:139` and `fast_path.rs:49,130`
explaining that `total_tokens: 0` is correct (no LLM call on that path), so a
future reader does not "fix" a non-bug. `execute.rs:325` left as-is.

## Scope Boundaries

- **No subagent token roll-up.** Each harness reports only its own provider
  calls. Subagent tokens surface in `LoopRunResult.total_tokens`; they are
  **not** added into the parent harness's `total_tokens`. Tree aggregation is
  a separate feature, explicitly out of scope. (Approved: this changes the
  meaning of a parent run's `RunSummary.total_tokens` to "this harness's
  direct LLM usage".)
- **Verifiers** (`StopHook`, `ToolLoop`) are deterministic and make no LLM
  calls — no token leakage.
- **The `execute.rs` double-emit** is a pre-existing bug, not fixed here.

## Architectural Compliance (R10)

Changes touch only existing files in `src/harness/` (`agent.rs`,
`agent/think.rs`) — **no new files**, the 9-file / ~1500-line harness budget
is untouched. Token accumulation is metrics/state bookkeeping, not reasoning:
it does not touch any of the loop's five "don'ts" (intent classification,
tool filtering, completion judgement, content review, error-recovery
strategy). It passes the Future-Proof Test — a stronger model still just has
its provider-reported tokens summed.

## Testing (TDD)

1. **`turn_token_total` pure helper** — `None` usage → 0; `None` cache
   components → 0; four-component sum; `thinking_tokens` excluded.
2. **`AgentHarness` unit test** — mirrors the existing `hit_limit` tests
   (`agent.rs:654`/`695`): a stub provider returning known `usage` across
   multiple turns; assert `harness.total_tokens()` equals the expected sum.
3. **`harness_bridge` integration** — `FlowOutcome.total_tokens` is non-zero
   after a run with a usage-reporting provider.
4. **`subagent_spawner`** — `LoopRunResult.total_tokens` is non-zero after a
   subagent run.

Failing tests are written before implementation (TDD red→green). Verification
uses targeted suites; known baseline noise (see
`project_baseline_test_failures.md`) is not a blocker.
