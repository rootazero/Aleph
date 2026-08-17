# Orchestrator Module — Static Audit Report

**Module:** `src/orchestrator/` (36 files, ~11,447 lines)
**Date:** 2026-08-16
**Lenses:** seam/wire, logic, architecture
**Scope:** read-only audit — no fixes applied
**Excluded commits (already fixed):** `ab545a685`, `8a6401f89`, `64592202b`

---

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High | 2 |
| Medium | 5 |
| Low | 6 |
| **Total** | **13** |

| Category | Count |
|----------|-------|
| logic | 5 |
| architecture | 5 |
| quality | 3 |
| security | 0 |

---

## Findings

### [High] src/orchestrator/resolver.rs:69-114 + src/orchestrator/dispatch.rs:834-840 — `SessionStrategy::Child` resolves parent but never persists it
**Category:** logic
**Confidence:** High

`resolve_session` correctly computes `SessionResolution.parent_session_key` for the `Child` strategy (`resolver.rs:100-113`), but `Orchestrator::dispatch` only reads `session_res.session_key` and discards `session_res.parent_session_key`. The session is then created in the session store via `session_seed::seed_session` without ever calling a `parent_session_key` writer — and the rest of the codebase confirms this column stays unset on the orchestrator path: `src/gateway/session_store/file_backend/mod.rs:362` returns `parent_session_key: None` for the create path.

Meanwhile the schema, query handlers, and `orphan_notice.rs` all read `parent_session_key` (`session_store/types.rs:105`, `session_manager/ops/mod.rs:48`, `orphan_notice.rs:37`). The column exists, the back-end reads it, and the gateway scheduler's `task_submit` writes it (see `task_submit` and `tasks/teams` paths), but the orchestrator's main chat path — the dominant writer — drops the value on the floor. Net effect: a `Child` strategy flow spawned through the orchestrator creates a session that is invisible to every parent-indexed query (`idx_sessions_parent`, parent-scoped compaction, room/team orchestration, `chat.history` parent rollups). The `SessionStrategy::Child` arm of the resolver is therefore load-bearing on computation only.

**Suggested fix:** Thread `session_res.parent_session_key` from `dispatch` into either (a) `FlowInput` (an explicit `parent_session` already exists on `FlowRequest`, so the cleaner choice is to set it on the `SessionService::create`/`upsert` call inside `session_seed::seed_session`) or (b) an explicit `SessionEvent::SessionStarted { parent: ... }` event seeded before the user turn. Add a regression test that asserts `parent_session_key` is non-`None` after dispatching a `Child` flow.

---

### [High] src/orchestrator/dispatch.rs:813-820 — `UnknownFlow` after successful routing leaves no graceful fallback (registration/agent drift)
**Category:** logic
**Confidence:** High

`Orchestrator::dispatch` step 1 has a graceful fallback ONLY for `FlowError::UnknownAgent` (it resolves to `default-agent` and overrides `spec.agent`). For `FlowError::UnknownFlow` — raised when a flow id is in `routing_overrides`/`default_routing` but absent from the `FlowRegistry` — the function returns the hard error and the gateway surfaces `flow: unknown flow id: <name>` to the user.

The bootstrap path that builds `default_routing` (`src/bin/aleph-server/commands/start/orchestrator_init.rs:142-148`) iterates `agent_registry.list_ids()` and maps every id to a same-named flow id, except `main` → `default-agent`. If the registry ever holds an agent whose id is NOT a preset flow id (e.g. `loop-auditor`, present in `src/agents/registry.rs:395` but absent from `src/orchestrator/presets/default_flows.toml`), every dispatch that resolves to it hard-fails. The `default_flows.toml` header comment claims "7 preset FlowSpecs, 1:1 with AgentDef builtins", but the registry now ships 8 builtins — the comment is stale and the registry/preset pairing has drifted (form 5 / form 1 split: the wire is built but no producer).

Latent today (the gateway always sends `agent_id: "main"`, so the broken entries are never looked up), but a single line change anywhere — a plugin registering `loop-auditor`, a session created with a SubAgent `agent_id`, a future override table that mentions it — turns it into a hard user-visible failure with no recovery path.

**Suggested fix:** Either (a) add a `loop-auditor` preset to `default_flows.toml` (and tighten the header comment to be a real invariant, asserted by a test), or (b) make step 1 treat `UnknownFlow` exactly like `UnknownAgent` — fall back to the `default-agent` flow, override `spec.agent` with the requested id, and warn-log the misconfiguration. Pair with a test that constructs `default_routing` from `agent_registry.list_ids()` and asserts every entry resolves.

---

### [Medium] src/orchestrator/dispatch.rs:810-822 — Every gateway chat dispatch hits `FlowError::InvalidConfig` for any SubAgent agent_id
**Category:** logic
**Confidence:** Medium

Combined effect of two pre-existing facts:
1. `default_routing` (binary bootstrap) maps every builtin SubAgent id (`explore`, `coder`, `researcher`, `default`, `plan`, `verify`) to its same-named preset flow.
2. Those six preset flows all declare `session_strategy = { kind = "child" }` with `parent_session_key` unset in TOML (`default_flows.toml:14-66`), AND `FlowRequest::parent_session` is hardcoded `None` in the gateway chat path (`src/gateway/execution_engine/run_loop/inner.rs:1321`).

`SessionStrategy::Child` requires a non-empty `parent_session` at runtime (`resolver.rs:108-110`), so any `Orchestrator::dispatch` that resolves to one of these six flows returns `InvalidConfig`. Latent today (gateway sends `agent_id: "main"`), but the routing table promises a path that the dispatcher cannot honor — a stale `default_routing` entry will hard-fail the moment anything (UI, session row repair, a plugin) sends one of those ids. Pairs with the High finding above; same fix shape.

**Suggested fix:** Either pre-resolve the routing table to skip non-Primary builtins (`if id == "main" || mode == AgentMode::Primary`), or seed `parent_session` from the session store when the current session has a parent (defensive), or make the `default` preset (`session_strategy = fresh`) the routing target for every SubAgent id (since the gateway never spawns SubAgents through `Orchestrator::dispatch` anyway — they go through `subagent_spawner::spawn`).

---

### [Medium] src/orchestrator/harness_bridge/runner_impl.rs:342-360 — MoA one-shot pref is consumed before the failure points it is documented to restore on
**Category:** logic
**Confidence:** Medium

`take_for_run` (line ~346) atomically removes the one-shot MoA preference from the session-keyed handle. The surrounding doc comment (line ~316) and the in-code comment at ~360 say "success, error and cancel paths all leave no state" — implying every error path restores the pref via `restore_one_shot`. But the restore call only happens inside the `try_build_for_run` error arm (line ~378). Between `take_for_run` and `harness.run` (~line 605), the code performs ~10 failure-capable steps: cancel pre-check (`is_cancelled()` early-return at line 213), agent verification (`UnknownAgent` return at line 233), `seed_session` (~315), model directive resolution (~265-300), `MeteringProvider` construction, `session_id` derivation, `routing_text` recall, `build_system_prompt`, context budget construction, and `HarnessDeps` assembly. Any error from these consumes the one-shot pref and never restores it — a one-shot MoA activation set by the user is lost without engaging.

This is a real correctness gap (user-facing feature: a one-shot MoA directive silently disappears on any of these transient failures). The "all paths leave no state" claim is false by inspection.

**Suggested fix:** Refactor to a guard-based pattern: take the pref into a local `Option`, and on every early-return path before `harness.run` either restore it or wrap it in a guard whose `Drop` restores. Simplest: move `take_for_run` to immediately before `harness.run`, OR introduce `MoAOneShotGuard` with `Drop` that restores when not consumed.

---

### [Medium] src/orchestrator/dispatch.rs:541-548 — `Orchestrator::reload_flows` only replaces the registry; the cached `default_routing` and `sandbox_factory` are immutable after construction
**Category:** architecture
**Confidence:** High

`Orchestrator` exposes `reload_flows(new_set)` (line 1005) for hot-swap of the `FlowRegistry`, but no equivalent for `routing_overrides`, `default_routing`, `sandbox_factory`, or the per-flow `SessionStrategy` semantics. `default_routing` is built once at boot from `agent_registry.list_ids()` (`orchestrator_init.rs:142-148`) — when a plugin registers a new SubAgent after boot, or a project loads a new tier of agents, the routing table has no entry for them; dispatch falls through to the `UnknownAgent` arm and uses the canonical default. That arm rewrites `spec.agent` correctly, but the user-facing agent they asked for is silently replaced.

`SessionLockGuard` ordering and the `Spec` mutation at line 810-820 (`s.agent = req.agent_id.clone()`) show the design *can* swap identities; but the routing table is fixed for the lifetime of the `Orchestrator`.

**Suggested fix:** Either (a) add `reload_routing`/`reload_agent_registry` methods that re-key `default_routing` from a passed-in `AgentRegistry`, with the same `ArcSwap` semantics as `FlowRegistry`, or (b) make `default_routing` a `Arc<ArcSwap<HashMap<...>>>` that the gateway's agent registry watcher hot-swaps. Pair with a test that registers a SubAgent after construction and dispatches against the new id.

---

### [Medium] src/orchestrator/mod.rs:39 — Dead re-export: `MAX_FLOW_DEPTH` has zero non-test consumers
**Category:** architecture
**Confidence:** High

`pub use resolver::{RoutingOverrides, MAX_FLOW_DEPTH}` at `mod.rs:39` re-exports `MAX_FLOW_DEPTH`, but the only consumers across `src/`, `tests/`, `interfaces/`, `shared/`, `desktop/` are inside `src/orchestrator/tests/{resolver,dispatch}.rs`. `resolver::depth_guard` is `pub(crate)` and is called from `dispatch.rs:813` directly. The re-export exists for no caller; it pollutes the public surface and misleads external readers into thinking external crates may tune the depth cap (they cannot — it's `pub` only through this re-export). Recent commit `8cb10a0ce` ("routing: cut dead re-exports…") caught similar dead re-exports but missed this one.

`ContextBudgetRefiner` and `ProviderChain` are in the same category — re-exported at `mod.rs:18-19`, but every consumer that exists imports them through the deeper `crate::orchestrator::deps_builder::*` path (verified: zero external references to `orchestrator::ProviderChain` or `orchestrator::ContextBudgetRefiner` exist outside the module itself).

**Suggested fix:** Remove the `MAX_FLOW_DEPTH` re-export from `mod.rs`. Either keep `ContextBudgetRefiner` and `ProviderChain` re-exported (they appear in the public doc surface for some consumers) or — if the grep confirms they are unreachable from outside — drop them too. Pin the cut with a `#[deny(dead_code)]` and a test that constructs a downstream crate-style import.

---

### [Medium] src/orchestrator/dispatch.rs:873-958 — `FlowStreamEvent::Complete` is NOT emitted on error/cancel paths from `AgentHarnessRunner::run`
**Category:** logic
**Confidence:** High

`BroadcastCallback::on_complete_with_outcome` (the ONLY place `FlowStreamEvent::Complete` is emitted, per its single-source comment at `runner_impl.rs:1170-1175`) is only called on the `Ok` arm of `run_result` (`runner_impl.rs:1181-1182`). The error paths (cancelled, transient, internal) reach `Err(map_flow_error)` via the `?` operator at `runner_impl.rs:1067`, and `cb.on_complete_with_outcome(&outcome)` is skipped.

The gateway drain handles this with a safety-net fallback (`execution_engine/helpers.rs:329-358`) that constructs `FlowOutcome::default()`-like summary from the `Outcome` field. But:
- the safety-net summary is built from `outcome` which does not exist on the error path (it's the `Err` arm — no `FlowOutcome` was constructed);
- the `RunComplete` event uses `total_duration_ms = 0` because `outcome.duration_ms` is unset on the error path;
- any subscriber that expects `FlowStreamEvent::Complete` (per the `#[non_exhaustive]` contract) to be the last frame will hang waiting on a frame that never arrives on error.

The drain itself terminates cleanly via `RecvError::Closed` once the spawn task drops the `event_tx` sender, but downstream subscribers (channel adapters, telemetry, replay tools) waiting on the broadcast channel are starved of the terminal signal.

**Suggested fix:** Either emit `FlowStreamEvent::Complete(error_outcome)` on every error path with a populated synthetic `FlowOutcome` (zero tokens, empty final_text, `terminate_reason` from the error), or document that subscribers MUST also wait on `handle.completion` rather than the broadcast channel. Add a test that asserts `Complete` is emitted for both `FlowError::Cancelled` and `FlowError::Transient`.

---

### [Low] src/orchestrator/dispatch.rs:963-977 — `Orchestrator` constructor exposes 6 `Arc` fields as `pub`, leaking the internal shape
**Category:** architecture
**Confidence:** High

All six fields of `Orchestrator` are `pub` (`flow_registry`, `routing_overrides`, `default_routing`, `session_service`, `sandbox_factory`, `harness`, plus `subagent_routing`, `agent_registry`, `active_sessions`). External callers (and tests) mutate them after construction by going through `with_*` builders, but there is no compile-time enforcement that those builders are used; `Orchestrator::new(...)` returns a struct where any field can be re-assigned later by anyone holding the `Arc<Orchestrator>`. The two `with_*` builders (`with_subagent_routing`, `with_agent_registry`) thus only function as documentation; the language allows direct field writes.

Inconsistent with `AgentHarnessRunner` (which exposes fields `pub` too, but the runner has no `with_*` builders), but both leak the same abstraction. The "real" cost is that any `Arc<Orchestrator>` holder can race on `active_sessions` writes (the lock guards the HashSet, but a `mem::replace` on the `Arc<Mutex<...>>` itself would orphan locks).

**Suggested fix:** Make the non-mutable fields `pub(crate)` and keep only the read-only handles (`flow_registry`, `routing_overrides`) as `pub` for diagnostics. For `active_sessions`, ensure no external code path assigns it.

---

### [Low] src/orchestrator/resolver.rs:69-115 + src/orchestrator/flow_spec.rs:114-126 — `SessionResolution::is_new` is computed but never read
**Category:** architecture
**Confidence:** High

`SessionResolution { session_key, parent_session_key, is_new }` (resolver.rs:69-74) computes `is_new` on every `resolve_session` call. The only consumers of the struct are the test files (`orchestrator/tests/resolver.rs:98,123,140`), where it's asserted on. `Orchestrator::dispatch` never reads `is_new`, and no other production code path uses it.

If `is_new` was meant to drive a "create" vs "open" branch in the session service, that branch is missing; the session store has only a single `upsert` shape. Dead field, no functional impact.

**Suggested fix:** Either remove `is_new` (the resolver tests assert on it, so update those too), or wire it through to `session_seed::seed_session` so an existing session skips the UserMessage emission when the request is a continuation (currently `seed_history` already does this with its own "log already non-empty" probe — `session_seed.rs:97-102`, so `is_new` is doubly redundant).

---

### [Low] src/orchestrator/harness_bridge/prompt_build.rs:191-204 — Per-turn `memory_injection_headroom` does a full session log read on the hot path
**Category:** architecture
**Confidence:** Medium

`memory_injection_headroom` calls `self.session_service.get_events(session_id, None, None)` on every run (every user turn) to compute the history token sum. For a long-lived session with N events, this is a full table scan + tokenize loop on the prompt-assembly hot path. The `context_pressure_reminder` sibling (line 220-256) repeats the same read on the same call.

The harness `ContextCompactor` already keeps the post-compaction history token sum in memory (via `ContextBudget::current_pressure()`); the harness `prompt` layer already calls `build_prompt` (which reads events again, lines 1169-1170 of `runner_impl.rs`). A single long-lived session therefore reads its event log three times per turn for three different budget calculations.

**Suggested fix:** Memoize the per-session history token count inside `AgentHarnessRunner` (LRU keyed by session_id, invalidated on `RunFinished`). One read per turn, shared across headroom / pressure / prompt builders. Out of scope for a config-only fix; flag for the perf team.

---

### [Low] src/orchestrator/loader.rs:54-87 — `load_user_flows_from_dir` reads every `*.toml` synchronously with no error tolerance
**Category:** quality
**Confidence:** Medium

A single malformed TOML in `~/.aleph/flows/` aborts the entire load (`parse_flow_file` returns `Err` propagated through `?` at line 78). A typo in one user file prevents ALL other user flows from being loaded — including any that came before it lexicographically. The catalog ends up with whatever loaded before the failure. This is intentional for duplicate detection (must read all files to detect cross-file dupes), but it is silent — there is no per-file warning log on partial failure, no continuation past recoverable errors.

For an operator who adds one broken file, the symptom is "my other custom flows disappeared" with no diagnostic pointing at the offending file (the error message does include the path, but only if they grep logs).

**Suggested fix:** Wrap the `parse_flow_file` call in a `match` that logs the per-file error at WARN and continues. Keep duplicate detection intact. Add a test that puts a malformed file alongside a valid file and asserts the valid one still loads.

---

### [Low] src/orchestrator/harness_bridge/prompt_build.rs:258-308 — `load_prompt_extra_files` reads files synchronously on the prompt-build hot path with no size cap on initial read
**Category:** quality
**Confidence:** Medium

`std::fs::read_to_string` is called for every path in `[prompt.extra_files]` (line 287) with no upper bound on the file size before reading. The cap (`per_file_max_chars`) is applied AFTER the full read via `truncate_chars`. A multi-GB file at one of the configured paths blocks the prompt-assembly task for the duration of the read + full string allocation, on every turn.

The window-scaled cap is sized correctly for content that already loaded, but the read itself is unbounded. An operator who accidentally points the config at `/var/log/syslog` or a large CSV brings the harness loop to a halt on every turn until they remove the path.

**Suggested fix:** Use `File::metadata()` first to check size, skip+warn if above a hard ceiling (e.g. 10 MB), then read. Or use `tokio::fs::read_to_string` to avoid blocking the executor.

---

### [Low] src/orchestrator/dispatch.rs:534-548, 722-742 — `Orchestrator::new` takes 6 `Arc` parameters with no builder; easy to mis-order
**Category:** architecture
**Confidence:** Medium

The 6-argument `Orchestrator::new(flow_registry, routing_overrides, default_routing, session_service, sandbox_factory, harness)` plus the 2 mutator builders (`with_subagent_routing`, `with_agent_registry`) form an implicit builder, but the constructor order is positional and several arguments are `Arc<dyn Trait>` of similar shape. The boot site (`src/bin/aleph-server/commands/start/orchestrator_init.rs:399-414`) calls it with positional args; a swap of two adjacent `Arc<dyn SessionService>`-shaped parameters would compile silently and route the wrong service.

The companion trait `HarnessRunner` defaults expose 7 capability getters (lines 414-462 of `dispatch.rs`), each with a default `None`. Production gateways override 9 fields, test mocks override 0-2 — calling sites must keep up with the addition/removal of optional collaborators without compile-time enforcement.

**Suggested fix:** Replace `Orchestrator::new` with an explicit `OrchestratorBuilder { ... }` struct (`pub fn builder() -> Self`). The current 8 `with_*` getters on `HarnessRunner` could similarly become a single `HarnessRunner::capabilities() -> HarnessCapabilities` struct (immutable after construction, `Copy`).

---

## Not-Reported (Out of Scope / Verified Safe)

- **`dispatch.rs:541-548` `Orchestrator::new` field count** — verified that `dispatch_happy_path_returns_handle_and_completes` exercises the wiring end-to-end (test passes today, the registry/factory shape is correct).
- **`harness_bridge/error.rs::is_transient_harness_message`** — message-based classification is fragile but pinned by tests (`rate_limit_429_is_transient`, `plain_internal_error_is_not_transient`); document as future structural refactor (the existing `TODO(phase6c)` comment already says this).
- **`sandbox_factory.rs` `DenyAllSandbox`** — verified deny-all behavior with regression tests at `tests/sandbox_factory.rs:42-55`; the `CapabilityDenied` mapping is intentional.
- **`flow_registry.rs::ArcSwap` semantics** — verified that `replace_swaps_atomically` correctly preserves in-flight snapshots; no race.
- **`prompt_build.rs::build_system_prompt` `instrument!` style logging** — well-factored into per-phase elapsed-ms logs; useful for ops, not a defect.
- **`runner_impl.rs::CALIBRATION_CARRYOVER`** — single-slot, keyed by model id, with regression tests; not a contention hot path.
- **`harness_bridge/backfill.rs::backfill_events_from_messages`** — only caller verified (`execution_engine/run_loop/inner.rs:1257`), idempotency tests pass.
- **`context_estimate.rs::OverheadCache` (LRU-bounded)** — correctly bounded and tested; the previous unbounded map is fixed.
- **`escalate_partial_result` (dispatch.rs:274-296)** — well-tested with 4 dedicated tests covering the pass-through / upgrade / no-partial branches.

---

## Confidence Methodology

A finding is **High** confidence when:
- The defect is reachable in production (or has a clear 1-line change that makes it reachable), AND
- A direct test or git history pinpoints the breakage.

A finding is **Medium** confidence when:
- The defect is reachable but the symptom path requires multiple conditions, OR
- The fix path has more than one valid shape.

A finding is **Low** confidence when:
- The defect is a smell or latent risk rather than a current breakage, OR
- The fix is non-trivial and the cost-benefit is ambiguous.