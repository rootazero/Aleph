---
title: Subagent Hardening — Production Wiring Closure + Bug Fixes
status: draft
date: 2026-05-19
authors: ["claude-opus-4-7"]
scope: design-only — no code, no plan
follows: 2026-05-09-subagent-uplift-p3-design.md
branch: subagent-hardening (worktree /Volumes/TBU4/Workspace/Aleph-wt-subagent)
---

# Subagent Hardening — Production Wiring Closure + Bug Fixes

> **One-line thesis**: The subagent uplift roadmap (stages A–I, all "✅ Shipped")
> built every feature down to the `subagent_spawner` / `AgentRuntime` *builder*
> layer — but the final hop, from the production construction site
> (`run_loop.rs` → `SubagentTool`) into `AgentRuntime`, was never closed. Every
> stage's "≥1 真实消费者" criterion was satisfied by tests or by `AgentRuntime`
> itself, never by the gateway path. This spec closes that hop and fixes three
> genuine bugs surfaced by the same investigation.

## 0. Context

### 0.1 How this was found

A comparison study against `hermes-agent`'s `delegate_task` (a mature Python
subagent implementation) was used as a yardstick. hermes-agent is not
architecturally richer than Aleph — it simply does not leave its wires
dangling. The investigation found Aleph's infrastructure is **complete and
well-built**, but disconnected at the production boundary.

### 0.2 The wiring-gap pattern (verified in code)

| Stage (roadmap) | Built | Production reality |
|---|---|---|
| A — HarnessDeps sync (`fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout` / `trace_sink`) | `AgentRuntime::with_*` builders exist (`runtime.rs:191-218`) | `run_loop.rs:330` `SubagentTool::new(...)` never calls them. `runtime.rs:177-188` is a self-documented confession: *"Production wiring … lives at the SubagentTool construction site in run_loop.rs (currently passes None defaults) … tracked in roadmap as a P2 deliverable."* — never finished. |
| F — Streaming progress | `ForwardingTraceSink` exists, installed when `SubagentTool.trace_sink` is `Some` | `trace_sink` is never `Some` in production → background `check_status` `progress` array is always empty. |
| H — Worktree isolation | `SpawnRequest.isolation`, `WorktreeSandbox`, `WorktreeHandle` all exist and work | `runtime.rs:369` hardcodes `isolation: None`. Never triggered in production. |
| I — Per-agent MCP scope | `McpScope::provision`, `McpScopedToolService` all exist | `runtime.rs:360` hardcodes `plugin_registry: None`. An agent declaring `mcp_servers` fails loud (`subagent_spawner/mod.rs:167`). |
| C — LaneScheduler | shipped `5f9f155f1`, then **deleted** (commits `ae4f05532` + `e0e29d886`) | `with_lane_scheduler` had zero call sites outside `runtime.rs`. Correct YAGNI removal — there is now **no concurrency cap at all**. |

### 0.3 Three genuine bugs (not just unwired)

- **`total_tokens` is hardcoded `0`** (`subagent_spawner/mod.rs:477`). `MeteringProvider` emits `ProviderUsage` events with full token splits but keeps no running total. `LoopRunResult.total_tokens`, `SubagentTranscript.tokens_used`, and the tool's logged `tokens` are all permanently `0`.
- **No concurrency cap.** Post-`LaneScheduler` deletion, the sync-batch path (`loop_tool.rs:408`) and `spawn_background` (`subagent_tool.rs:236`) issue one unbounded `tokio::spawn` per task. A single `subagent` call with a large `batch_tasks` array fans out arbitrarily many concurrent harness+LLM runs.
- **Foreground / sync-batch subagents are not parent-cancellable.** `loop_tool.rs:412` and `loop_tool.rs:581` mint a fresh `CancellationToken::new()`. A cancelled parent leaves foreground children running to their `timeout_secs`.

## 1. Goals / Non-goals

### Goals
- Close the `run_loop.rs` → `SubagentTool` → `AgentRuntime` production wiring hop for roadmap stages A / F / H / I.
- Fix the three bugs in §0.3.
- Make `AgentDef.context_mode` authoritative (currently decorative).
- Do all of the above without resurrecting the deleted `LaneScheduler` and without touching `src/harness/` (R10).

### Non-goals
- `swarm/` dead-code removal (separate concern, user-confirmed out of scope).
- Full `AgentRuntime` builder-chain refactor beyond the one helper extraction needed to apply the new wiring (§3.C).
- Recursion depth-policy changes — Aleph's `SubAgent`-mode `subagent`-tool deny (`types.rs:246`) is a deliberate R7/R10 choice; kept as-is.
- hermes-style live-registry RPCs, timeout stack-dump diagnostics, recursive cost rollup — deferred (YAGNI; revisit only with evidence).
- Stage J (fork-subagent prompt cache) — remains deferred per P3 design Q3.

### Architectural redline compliance
- **R10** — zero changes to `src/harness/*`. The concurrency primitive is a `tokio::sync::Semaphore` (resource governance, not cognition); it lives on `SpawnerBase`, not in the loop.
- **R3** — no new dependencies (`tokio` already present).
- **R7** — `isolation` / `mcp_servers` / `context_mode` are schema fields the LLM triggers via tool calls; no rule engine, no inference replacement.
- Roadmap §0.4 budgets: `subagent_spawner/mod.rs` 553 → ~585 lines (< 600 cap). `src/agents/` net increment well under +600 (the helper extraction removes duplication).

## 2. Worktree & branch

All work happens in worktree `/Volumes/TBU4/Workspace/Aleph-wt-subagent`, branch
`subagent-hardening`, already created from `e0e29d886` — which **already
includes** the `src/scheduler/` deletion (`main` committed it between sessions
as `ae4f05532` + `e0e29d886`). No deletion replication needed. `main` is
untouched.

## 3. The changes

Grouped: **A** = bug fixes, **B** = wiring closure, **C** = enabling cleanup.

---

### A1 — Token accounting

**Problem**: `LoopRunResult.total_tokens` hardcoded `0` (`subagent_spawner/mod.rs:477`).

**Solution**: Give `MeteringProvider` an optional shared accumulator.
- `src/providers/metering.rs`: add field `total_tokens: Option<Arc<AtomicU64>>` + builder `with_accumulator(Arc<AtomicU64>)`. In `process()`, after a successful response with `usage`, `fetch_add(input_tokens + output_tokens)`.
- `subagent_spawner/mod.rs`: `spawn()` creates `let token_counter = Arc::new(AtomicU64::new(0));`, passes it via `.with_accumulator(token_counter.clone())` at the `MeteringProvider::new` site (line 262). After the harness run, `extract_run_result` receives the loaded count (pass it as a parameter, mirroring the existing `hit_limit` parameter).
- `LoopRunResult.total_tokens` ← `token_counter.load(Relaxed) as usize`.

**Semantics**: `total_tokens` = Σ(`input_tokens` + `output_tokens`) across all LLM calls in the child run. Matches `SubagentTranscript.tokens_used` doc ("Total tokens consumed").

**Test**: child run with a fake provider returning `usage` → `total_tokens > 0` and equals the injected sum.

---

### A2 — Concurrency cap (Semaphore)

**Problem**: No bound on concurrent subagent spawns post-`LaneScheduler` deletion.

**Solution**: A `tokio::sync::Semaphore` — the thin-harness replacement for the
1737-line `LaneScheduler`. It occupies the exact wiring slot the deleted
`lane_scheduler: Option<Arc<LaneScheduler>>` field vacated on `SpawnerBase`.

- `subagent_spawner/mod.rs`: `SpawnerBase` gains `subagent_semaphore: Option<Arc<Semaphore>>`. `spawn()` — immediately after the `child()` depth check — does `let _permit = match &base.subagent_semaphore { Some(s) => Some(s.clone().acquire_owned().await.map_err(|e| format!("sub-agent failed: semaphore closed: {e}"))?), None => None };` held to end of `spawn()`. `None` = no cap (direct test callers — keeps `subagent_spawner/tests.rs` compatible).
- `runtime.rs`: `AgentRuntime` gains `subagent_semaphore: Option<Arc<Semaphore>>` + `with_subagent_semaphore(...)`; threaded into the `SpawnerBase` it builds.
- `subagent_tool.rs`: `SubagentTool::new` creates `Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_SUBAGENTS))` once, stored on the struct, passed into every `AgentRuntime` it builds (foreground, sync-batch, background). `const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4` (matches the deleted `Lane::Subagent` default).

**Behaviour**: batch over-fan-out **queues** on the semaphore (`acquire_owned` awaits) rather than hard-rejecting (gentler than hermes-agent's reject-on-overflow). Permits cover all spawn paths via one shared `Arc<Semaphore>` per `SubagentTool` (= per top-level agent run).

**Test**: spawn N+1 with the semaphore at N permits → the (N+1)th observably waits until one completes (timing/ordering assertion).

---

### A3 — Parent cancellation propagation

**Problem**: foreground (`loop_tool.rs:581`), sync-batch (`loop_tool.rs:412`),
and background (`subagent_tool.rs:214`) all use `CancellationToken::new()`.

**Solution**: `SubagentTool` holds the parent run's token; each spawn path
derives a child token.
- `subagent_tool.rs`: `SubagentTool` gains `parent_cancel: Option<CancellationToken>` + `with_cancel_token(...)`.
- `run_loop.rs`: the construction block calls `.with_cancel_token(cancel_token.clone())` — `cancel_token` is already in scope (used at line 413 for `run_dispatch_and_drain_classified`).
- foreground / sync-batch: use `parent_cancel.as_ref().map(|t| t.child_token()).unwrap_or_default()` instead of `CancellationToken::new()`.
- background: register `parent_cancel…child_token()` with the tracker. Parent-cancel propagates **and** the tracker can still cancel that one child independently (`child_token()` is one-directional).

`None` → `CancellationToken::new()` fallback (tests / direct callers).

**Test**: foreground subagent blocked on a slow provider → parent token cancel → child stops before `timeout_secs`. (Extends `tests/cancellation_chain.rs`.)

---

### B1 — Worktree isolation wiring (closes roadmap H)

**Problem**: `runtime.rs:369` hardcodes `SpawnRequest.isolation: None`.

**Solution**: declarative per-agent, via `AgentDef` (user-confirmed approach).
- `src/agents/types.rs`: `AgentDef` gains `isolation: Option<IsolationMode>` with `#[serde(default)]`. `IsolationMode` already exists (`types.rs:45`).
- `src/agents/loader.rs`: frontmatter struct gains `isolation: Option<IsolationMode>`; applied in `parse_file` (mirrors the existing `context_mode` handling at `loader.rs:152`). It is **not** in the forbidden-field set (`mode` / `source`), so user/project agents may declare it.
- `runtime.rs`: `execute_via_harness` sets `isolation: config.agent_def.isolation` instead of `None`.
- **All builtin agents stay default-off.** Worktree isolation changes *where* a child's file writes land — surprising for `coder`-type agents whose edits the parent expects to see. Capability is wired; opt-in is explicit and only via user/project agent files.

**Test**: an `AgentDef` with `isolation: Some(Worktree)` spawned through `AgentRuntime` → a worktree is provisioned and cleaned up. (Extends `tests/worktree_isolation.rs`, which currently only calls `spawn` directly.)

---

### B2 — Per-agent MCP scope wiring (closes roadmap I)

**Problem**: `runtime.rs:360` hardcodes `SpawnerBase.plugin_registry: None`; an
agent declaring `mcp_servers` fails loud.

**Solution**: pure wiring — thread the global plugin registry through.
- `runtime.rs`: `AgentRuntime` gains `plugin_registry: Option<Arc<PluginRegistry>>` + `with_plugin_registry(...)`; sets `SpawnerBase.plugin_registry` from it.
- `subagent_tool.rs`: `SubagentTool` gains `plugin_registry: Option<Arc<PluginRegistry>>` + `with_plugin_registry(...)`; passed into every `AgentRuntime`.
- `run_loop.rs`: construction block calls `.with_plugin_registry(...)`. Source: the `ExtensionManager` / plugin registry already available at the construction site (`extension_manager` is in scope at `run_loop.rs:284`). The plan resolves the exact accessor.

**Test**: an `AgentDef` with one inline `mcp_servers` entry spawned through `AgentRuntime` with a registry wired → no fail-loud error. (Reuses `tests/mcp_scope.rs` patterns.)

---

### B3 — Stage A safety-feature wiring (closes roadmap A)

**Problem**: `fallback_llm` / `stall_config` / `consecutive_failure_cap` /
`turn_timeout` / `trace_sink` builders on `AgentRuntime` are never called in
production. `runtime.rs:177-188` documents this as the unfinished P2 hop.

**Solution**: thread the values from the construction site.
- `subagent_tool.rs`: `SubagentTool` gains `fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout` fields + `with_*` builders. (`trace_sink` field already exists, `subagent_tool.rs:110`.)
- `run_loop.rs`: construction block calls the new `with_*` builders. **`trace_sink` is the easy win** — the `GatewayTraceSink` is already built at `run_loop.rs:366`, just below the `SubagentTool` block; move that construction *above* the block and call `.with_trace_sink(trace_sink.clone())`.
- The four resilience values reuse the existing `build_fallback_llm` + `build_stability_triple` builders from `orchestrator_init.rs` (roadmap Stage A "Solution sketch"). The plan determines whether they are called directly at `run_loop.rs` or extracted to a shared `deps_builder` — **this is the one genuine open question**; both satisfy the goal. Source values come from the same `aleph.toml` `[stability]` / `[fallback_provider]` config the main runner uses.
- All threaded into every `AgentRuntime` via the §3.C helper.

**Test**: integration — a subagent spawned through the production-style path inherits a non-`None` `trace_sink` (verified by a captured event). Unit — `SubagentTool` `with_*` builders propagate into `AgentRuntimeConfig`/`SpawnerBase`.

---

### B4 — Background progress (closes roadmap F — consequence of B3)

Once B3 makes `SubagentTool.trace_sink` `Some` in production, `spawn_background`
already installs `ForwardingTraceSink` (`subagent_tool.rs:263-272`), so
`check_status` `progress` becomes non-empty. **No new code** — B4 is a
verification item.

**Test**: background subagent doing ≥1 tool call → `check_status` returns a
non-empty `progress` array (production-style wiring, not the unit-level
`check_status_returns_progress_array_when_running` which pushes progress
manually).

---

### B5 — `context_mode` made authoritative

**Problem**: `AgentDef.context_mode` (`Fresh` / `Summary`) is set on builtins
(`registry.rs:151/167/176`) and parsed from frontmatter (`loader.rs:152`) but
**never read by the spawner** — the field lies.

**Solution**: `subagent_spawner/mod.rs` — when building `effective_task`
(line 207), if `req.agent_def.context_mode == ContextMode::Fresh`, ignore
`context_summary` even if supplied. `Summary` keeps current behaviour.

**Test**: spawn a `Fresh`-mode agent with a non-empty `context_summary` → the
child's seeded `UserMessage` does not contain the "Context from parent agent"
header.

---

### C — Enabling cleanup

**C-helper (de-dup)** — B2 / B3 / A3 each add ~4 builder calls to **four**
`AgentRuntime` construction sites (foreground `loop_tool.rs:577`, sync-batch
`loop_tool.rs:409`, background `subagent_tool.rs:245`). Without consolidation
that is 4× duplication of a growing chain. Extract one private
`SubagentTool::build_runtime(&self, child_chain, cancel) -> AgentRuntime` that
applies every `with_*` from the tool's own fields. This is *part of* the wiring
work — not the broader "comprehensive" refactor (`swarm/` stays untouched).

**C1 — `BackgroundAgentTracker::cleanup` caller** — `cleanup(ttl)` exists
(`background_tracker.rs:130`) with only a test caller. Completed background
results accumulate forever. Call `cleanup` opportunistically inside `register`
(every new background spawn), with a fixed TTL (e.g. 1h — plan picks the exact
value). Cheap, bounds growth.

## 4. File-change summary

| File | Change | ~lines |
|---|---|---|
| `src/providers/metering.rs` | A1 — accumulator field + builder | +15 |
| `src/agents/subagent_spawner/mod.rs` | A1 read, A2 semaphore acquire, B5 context_mode | +32 |
| `src/agents/runtime.rs` | A2/B1/B2 fields + builders, set `isolation`/`plugin_registry`/`semaphore` | +40 |
| `src/agents/subagent_tool.rs` | A2/A3/B2/B3 fields + builders, C-helper, C1 | +70 |
| `src/agents/subagent_tool/loop_tool.rs` | use C-helper at 3 sites (net −duplication) | −10 |
| `src/agents/types.rs` | B1 — `AgentDef.isolation` field | +6 |
| `src/agents/loader.rs` | B1 — frontmatter `isolation` | +6 |
| `src/agents/background_tracker.rs` | C1 — call `cleanup` in `register` | +3 |
| `src/gateway/execution_engine/run_loop.rs` | B1/B2/B3/A3 — construction-site wiring | +25 |
| `tests/*` | new + extended tests (§5) | +250 |
| **net** | | **~+460** |

Within roadmap §0.4 budgets (`src/agents/` increment ≤ +600; `mod.rs` < 600).

## 5. Testing (TDD — red first)

| ID | Type | Asserts |
|---|---|---|
| T-A1 | unit | child run → `LoopRunResult.total_tokens` == injected token sum |
| T-A2 | integration | N+1 spawns at N permits → (N+1)th waits for a slot |
| T-A3 | integration | parent token cancel → foreground child stops < `timeout_secs` |
| T-B1 | integration | `AgentDef.isolation=Worktree` via `AgentRuntime` → worktree provisioned + cleaned |
| T-B2 | integration | `AgentDef.mcp_servers` + wired registry → no fail-loud |
| T-B3 | unit + integration | `SubagentTool.with_*` propagate; child inherits non-`None` `trace_sink` |
| T-B4 | integration | background subagent w/ tool call → non-empty `check_status` `progress` |
| T-B5 | unit | `Fresh`-mode agent ignores `context_summary` |
| T-C1 | unit | `register` evicts entries older than TTL |

**Must stay green**: `tests/cancellation_chain.rs`, `tests/worktree_isolation.rs`,
`tests/subagent_deps_inherit.rs`, `src/agents/subagent_spawner/tests.rs`, all
`subagent_tool` unit tests. Per memory `project_baseline_test_failures`, the 8
lib + 4 integration pre-existing baseline failures on `main` are unrelated and
do not block.

## 6. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| B3 — resilience config not in scope at `run_loop.rs` construction site | medium | medium | Plan-phase spike: locate `[stability]`/`[fallback_provider]` access on `ExecutionEngine`; `trace_sink` is unconditionally available so B3 partially lands regardless |
| A2 — semaphore deadlock if a child spawns a child while holding a permit | low | high | `SubAgent`-mode agents cannot call `subagent` (`types.rs:246`) → no nested acquire in practice; documented as the invariant the cap relies on |
| B1 — a user enabling `isolation` on a file-mutating agent loses the child's edits | low | medium | Builtins default-off; doc note in `MULTI_AGENT_SYSTEM.md` |
| Helper extraction changes behaviour of one of the 3 sites | low | medium | TDD: existing `subagent_tool` tests pin behaviour before/after |

## 7. Roadmap reconciliation

- Roadmap Stage C (`LaneScheduler`) is **reverted** — note this in the master
  roadmap (`2026-05-08-subagent-uplift-roadmap-design.md`) per its §4.1
  light-revision rule; A2's `Semaphore` is the lightweight replacement.
- Stages A / F / H / I get a follow-up note: "production wiring closed by
  2026-05-19-subagent-hardening".
- Roadmap closure condition "文档与代码事实一致" — update `MULTI_AGENT_SYSTEM.md`
  and remove the stale `runtime.rs:177-188` "P2 deliverable" comment block.

## 8. Out of scope (deferred)

`swarm/` dead code · full `AgentRuntime` refactor · recursion depth-policy ·
hermes-style live-registry RPCs / timeout diagnostics / cost rollup · Stage J.

## 9. Closure

This design is the input for `superpowers:writing-plans`. One plan,
`docs/superpowers/plans/2026-05-19-subagent-hardening-plan.md`, executed TDD on
the `subagent-hardening` worktree branch.
