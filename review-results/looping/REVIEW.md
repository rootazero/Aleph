# Looping Module Review (2026-08-22)

**Module:** `src/looping/` (`mod.rs` 1156L, `types.rs` 822L, `pursuit.rs` 481L)
**Reviewer:** rust-doctor subagent
**Methodology:** Static code review + cross-reference to `src/looping/{mod,types,pursuit}.rs`, `graphify-out/GRAPH_REPORT.md`, and direct callers in `gateway/execution_engine/execute.rs`, `builtin_tools/loop_manage.rs`, `gateway/handlers/{users,session/db_handlers/modify}.rs`, `scope/mod.rs`. No compile/test runs (per directive).

## Summary

- Total findings: 12
- critical: 0, high: 2, medium: 5, low: 5

The module is unusually well-instrumented for a control-plane: every lifecycle decision flows through one of three atomic registry calls (`transition`, `try_claim_tick`, `confirm_fire`/`commit_field_update`/`rearm_after_interruption`), the docs are unusually honest about edge cases (clock-unavailable, counter-reset, race windows), and the unit-test density is high (82 tests inside the module). No swallowed errors, no `expect()`/`unwrap()` in production paths, no platform-specific code (R1 holds). The findings below are mostly small inconsistencies and a couple of hidden gaps.

## Findings

### [high] `stop_all` does not refund iteration on Active→Stopped when a tick is sleeping

- **Location:** `src/looping/mod.rs:203-220`
- **Category:** correctness
- **Description:** `stop_all` writes `with_status(Stopped).with_stop_reason(...).with_pending_tick(None)` directly, while the sibling `transition` (mod.rs:148-191) explicitly calls `refund_iteration()` when `from == Active && pending_tick_wake_ms.is_some()`. So a manual `transition(id, Stopped)` on an Active loop with a claimed-but-unfired tick correctly refunds the bump (`refund_iteration` is in `types.rs:203`), but `stop_all` does not — five kill-switch hits on a 5-tick loop can leave a Stopped row with `iterations_used: 5/5` having executed nothing, matching exactly the bug the type-level comment at `types.rs:184-201` was written to prevent.
- **Impact:** `stop_reason` and `iterations_used` are the two fields a user-facing `loop(action='status')` reports after a kill switch. The "iteration 5/5, reached cap" reading is wrong on a kill switch, and it also makes per-session cap accounting non-additive across lifecycle moves.
- **Suggested fix:** either route the per-row write through `transition(session, Stopped, Some(reason.clone()))` (taking the legality matrix + refund for free), or inline the same `refund_iteration` step inside the loop. Test: claim a tick on a 1-cap loop, then call `stop_all`; assert `iterations_used == 0` and `pending_tick_wake_ms.is_none()`.
- **Related:** `src/looping/types.rs:184-201` (`spent_iteration`/`refund_iteration` rationale); `pause_all_owned_by` (mod.rs:236-261) already routes through `transition` correctly — that is the pattern to mirror.

### [high] `try_claim_tick` fails CLOSED when clock is unavailable AND a tick is in flight

- **Location:** `src/looping/mod.rs:299-306`
- **Category:** correctness / concurrency
- **Description:** The in-flight gate reads `now_ms < wake.saturating_add(PENDING_TICK_STALE_GRACE_MS)`. With `now_ms == 0` (SystemTime unavailable — the documented "clock unavailable" sentinel) and any `wake > 0`, `0 < wake + 60_000` is always true, so the function returns `TickDecision::Idle` forever. The rest of the module treats `now_ms == 0` as "fail open" (`pursuit::exhausted:18-20`, `pursuit::fires_out_of_bounds:54-61`, `pursuit::is_final_deadline_tick:80-93`, `LoopState::live_status:439-462` all bail with `false`). Only this one gate fails closed.
- **Impact:** A clock blip between claim and fire strands a previously-healthy loop as a dormant Active. There is no UI path to recover except `loop(action='stop')` + `start` — and `start` does not necessarily preserve the cadence (a fresh `LoopState::new` at `loop_manage.rs:354-359`). In practice `now_ms == 0` is rare (only when `SystemTime::now()` fails), but the divergence from the documented "fail open" contract is a correctness gap, not just a stylistic one.
- **Suggested fix:** Treat `now_ms == 0` as "cannot tell if pending is fresh" → fall through to the regular claim path (no grace check, just the rest of the function). This matches the fail-open pattern of `exhausted` and `fires_out_of_bounds`. Test: `put(state.with_pending_tick(Some(1))); try_claim_tick(id, None, 0);` should return `Fire`, not `Idle`.
- **Related:** Same failure mode exists in `rearm_after_interruption`'s pending-marker check (`mod.rs:393-401`) — it requires `pending_tick_wake_ms.is_none()` to proceed, so a `now_ms == 0` + pending-marker pair returns `Drop`. Consistent failure mode; both should fail open together.

### [medium] `pause_all_owned_by` does N+1 lock acquisitions and is not atomic across the batch

- **Location:** `src/looping/mod.rs:236-261`
- **Category:** concurrency / perf
- **Description:** `list_all()` releases the lock, then each target `transition()` re-acquires it. Between the scan and any individual transition, a concurrent `start` could insert a new loop belonging to the same user; that loop is silently missed. Symmetrically, a concurrent stop on a target loop just makes one transition return `Refused`, which is correctly filtered by the `matches!(...Applied)` arm, but only by accident.
- **Impact:** For a user with many active loops, the function also contends with normal traffic N times. The correctness gap is small (a loop started after the snapshot will simply be paused by the next caller, if any), but the pattern is the only place in the module that batches work across the lock.
- **Suggested fix:** Acquire the lock once, snapshot targets, transition each under the same guard, and return the count. The single-locked variant matches `stop_all`'s design (mod.rs:203-220) exactly. Cost: ~5 lines; benefit: atomicity and 1 lock acquisition per call.
- **Related:** `stop_all` is the model to follow — `mod.rs:203-220`.

### [medium] Token budget can be permanently unenforced when the session token counter is unavailable

- **Location:** `src/looping/types.rs:278-284`, `src/looping/mod.rs:283-295`
- **Category:** correctness / error-handling
- **Description:** Baseline is only seeded when `(token_budget=Some(_), baseline_captured=false, tokens_total=Some(_))` all hold. `over_budget` returns false unconditionally when `baseline_captured == false`. If `tokens_total = None` on every claim (caller path: `execute.rs:825-836` — `session_manager` is `None` or `get_total_tokens` errors), the baseline never seeds and the budget is silently inert forever. There is no "budget declared but never enforced" state surfaced to `loop(action='status')`.
- **Impact:** A user who sets `token_budget=50000` sees a real number in status; the loop runs unbounded. Worse, the `human_summary` line at `types.rs:354-407` reports "token budget: 50000" without any indicator that enforcement is dormant.
- **Suggested fix:** Either (a) treat a budget with no captured baseline as "unenforced" and surface that fact in `human_summary` / `stable_summary`, or (b) require baseline capture as part of start-up before the first claim (which is the cleaner invariant but requires plumbing the counter into the `loop` tool's start path). Option (a) is local; option (b) is principled.
- **Related:** `types.rs:255-275` (`with_baseline`/`over_budget`); `goal/store.rs` has the same shape — compare the parallel decision there.

### [medium] `tick_prompt` quota arithmetic can produce a negative "left" for iteration cap when `n == max`

- **Location:** `src/looping/pursuit.rs:144-149`
- **Category:** correctness
- **Description:** `tick_prompt` returns the "LAST tick" branch when `n >= max` (line 130-141). If the caller is the LAST-tick branch, it is fine — but the early-return is gated only by `capped || is_final_deadline_tick`. If `is_final_deadline_tick` is true but iteration cap is unset, the function returns the LAST-tick branch with `bound = "time limit"` — fine. The risk is in the OTHER branch (non-last): if `max_iterations = Some(5)` and `iterations_used = 4`, then `n = 5` and `max - n = 0`, producing "0 tick(s) left before the cap." That sentence is awkward but not wrong. The actual bug would be triggered if `is_final_deadline_tick` is false but `max - n == 0` — currently impossible because `is_final_deadline_tick` is checked first AND `n >= max` triggers capped-first. The code is correct today; the brittleness is in the implicit ordering.
- **Impact:** Low (no observed misbehavior). The risk is a future refactor that re-orders the conditions.
- **Suggested fix:** Replace `format!(" {} tick(s) left before the cap.", max - n)` with `format!(" {} tick(s) left before the cap.", max.saturating_sub(n).saturating_sub(1))` (i.e., "ticks remaining after this one"). Make the condition order explicit in a comment.

### [medium] `tick_prompt` finality projection uses worst-case cadence, may be over-conservative for ModelPaced

- **Location:** `src/looping/pursuit.rs:63-72`, `src/looping/pursuit.rs:80-93`
- **Category:** correctness
- **Description:** `is_final_deadline_tick` calls `cadence_default_ms(state)` (line 63-72), which for ModelPaced returns `fallback_ms`. The semantics is "would the NEXT claim's wake still be in bounds?", but a ModelPaced loop can re-pace `next_wake` after this tick runs. So the projection assumes the worst-case (default) cadence and may mark a tick as final when one slightly later would also fit. Result: the loop gets a "LAST tick" warning one cycle earlier than strictly necessary.
- **Impact:** The user sees a "LAST tick" prompt one tick too early, then the loop continues. Confusing but safe (over-conservative, not unsafe).
- **Suggested fix:** Either accept the over-conservatism (the doc at line 74-78 makes it sound deliberate), or pass the current `next_wake_ms` when present and use it as the projected cadence for the next claim. Trade-off: the latter is less pessimistic but more brittle (model-paced `next_wake` could itself go past the deadline, requiring the next claim to bail — but that bail is already handled by `fires_out_of_bounds`).
- **Related:** `tick_prompt` "deadline_time_left" branch at `pursuit.rs:146-150` shares the same logic.

### [medium] `LoopRegistry` has no eviction of `Stopped` loops

- **Location:** `src/looping/mod.rs:73-130`, `mod.rs:203-220`
- **Category:** perf
- **Description:** `stop_all` and `transition(_, Stopped, _)` both leave the entry in the map. There is no `purge_stopped`, no TTL, no `with_capacity` reservation. Over a long-lived daemon, every session that ever had a loop accumulates a `LoopState` row (each carrying a `String prompt` and `Option<String> stop_reason`). One row per session — bounded, but not zero.
- **Impact:** Memory growth is bounded by session count, not time. For a daemon with N=10k sessions over its lifetime, the registry holds 10k entries after a year. Each is small (~200B), so the absolute footprint is small, but `list_all()` becomes O(N) on every `loop(action='list')` call and the snapshot is `Vec<LoopState>` with cloned `String`s (`mod.rs:121-130`).
- **Suggested fix:** Add `purge_stopped()` (or a cap on `Stopped` rows) and call it on `transition(_, Stopped, _)`. Document the retention window in the module doc. Alternatively, split the map into `active: HashMap<String, LoopState>` and `recent_stopped: VecDeque<(String, LoopState)>` with a max length.
- **Related:** `list_all` at `mod.rs:121-130` is the only consumer that touches stopped rows.

### [low] `with_owner_scope` allocates on every call; `personal("u-alice")` builds two `String`s

- **Location:** `src/looping/types.rs:227-232`
- **Category:** perf / API
- **Description:** `attr.map(|a| a.owner_user_id.clone())` and `attr.map(|a| a.scope.render())` each clone/allocate. `scope.render()` (called by `with_owner_scope` at types.rs:231) is itself a `String` allocation (per `src/scope/mod.rs:56`). Called at every `loop(action='start')`, this is two allocations per start — not a hot path, but the immutable builder style throughout the module makes it cheap to leave as-is.
- **Impact:** Negligible (one start = one pair of allocations).
- **Suggested fix:** None required; document the cost if perf ever becomes a concern.

### [low] `LoopState::stop_reason` is unbounded `Option<String>`

- **Location:** `src/looping/types.rs:166-170`
- **Category:** perf / API
- **Description:** `stop_reason` is populated by user-facing messages from three sources: the cap (`pursuit::cap_reached_note:176-183` etc.), failure paths, and `transition(reason)` from external callers (`builtin_tools/loop_manage.rs`). None length-bounds it. A malicious or buggy caller could pass a multi-MB string.
- **Impact:** Memory exhaustion in pathological cases only. Internal callers all use fixed short strings.
- **Suggested fix:** Cap at 4 KB on the way in. Either truncate in `with_stop_reason`, or wrap in a `BoundedString` newtype.

### [low] `tick_prompt` is computed under the registry lock — String formatting under contention

- **Location:** `src/looping/mod.rs:329`
- **Category:** perf
- **Description:** `let prompt = pursuit::tick_prompt(&state, tokens_now, now_ms)` is called inside the lock guard. `tick_prompt` (`pursuit.rs:113-170`) builds several `format!` strings, allocates, and concatenates — all while the registry mutex is held. Every other concurrent claim/transition is blocked.
- **Impact:** For a single-user, single-loop session, irrelevant. For a daemon with many active loops claimed concurrently, the formatted prompt length matters. In practice `tick_prompt` output is ~200 bytes, allocation is microseconds — fine for current cadence.
- **Suggested fix:** Compute the prompt AFTER releasing the lock, by holding a clone of the relevant state fields. Sketch: `let snapshot = state.clone_for_prompt(); drop(map); let prompt = pursuit::tick_prompt(&snapshot, ...); map.lock().insert(...);`. Note: this breaks the atomicity argument if the loop can be mutated between snapshot and insert — for the prompt the answer is fine because `tick_prompt` only reads immutable fields.
- **Related:** `tick_delay_ms`, `exhausted`, `fires_out_of_bounds`, `stop_reason_note` are all called under the same lock — those are cheaper (no allocation) so the same concern is weaker.

### [low] Test coverage gap: no concurrency tests, no `stop_all`-refund test, no clock-unavailable-Idle test

- **Location:** `src/looping/mod.rs` test module
- **Category:** testing
- **Description:** 82 tests in-module, all single-threaded. Notable gaps:
  - **`stop_all` iteration refund** (the inconsistency flagged above as `high`): no test asserts that `stop_all` refunds iteration on Active→Stopped with a pending marker.
  - **`now_ms == 0` + pending tick** interaction (the failure mode flagged above as `high`): no test exercises this combination.
  - **Concurrent claims / rearm races**: no `std::thread` or `tokio::join` test. The race scenarios documented in the comments (mod.rs:420-452 for `commit_field_update`, mod.rs:141 for the pause/dead-zone scenario) are described but not enforced by tests.
  - **`tick_prompt` quota clause under near-cap**: covered (`tick_prompt_surfaces_remaining_ticks_before_cap`), but the `max - n == 0` boundary is not.
  - **Failure path → `stop_loop_on_failure` / `park_loop_on_transient_failure` integration**: live in `execute.rs:1407-1660`, no test in the registry directly.
- **Impact:** Future refactors can silently break the inconsistency without test failure. The race contracts in the docs (`commit_field_update_preserves_live_pending_not_stale` etc.) rely on `commit_field_update`'s careful implementation; a regression there would only be caught by manual integration.
- **Suggested fix:** Add three small tests: (a) `stop_all_refunds_iteration_on_active_with_pending`, (b) `claim_with_clock_unavailable_proceeds_past_stale_pending`, (c) a multi-threaded stress test using `Arc<LoopRegistry>` shared across 4 threads doing random transitions/claims.

### [low] `init_global` is a fire-and-forget; lazy-init in tests can leak `Arc`s on race

- **Location:** `src/looping/mod.rs:517-525`, callers in `src/gateway/handlers/users.rs:1077-1079` and `src/gateway/handlers/session/db_handlers/modify.rs:1020-1024`
- **Category:** API / perf
- **Description:** The lazy-init pattern `global().unwrap_or_else(|| { init_global(Arc::new(...)); global().expect(...) })` allocates a fresh `LoopRegistry` and wraps it in `Arc` BEFORE attempting `init_global`. If two threads race the unwrap_or_else, both may allocate; only one wins `OnceCell::set` but both drop their `Arc` immediately on the next read. Result: at most one wasted `LoopRegistry` (a `Mutex<HashMap>` — two `usize`s of cost). Not a real bug.
- **Impact:** Negligible. The `expect(...)` calls will panic only if `init_global` fails to install, which is unreachable in practice (`OnceCell::set` returns `Err` only when called twice; the lazy-init pattern only calls it once per closure invocation).
- **Suggested fix:** Either document the pattern as "tolerates startup race by allocating one extra" or add a `try_init_global` that returns `Option<Arc<LoopRegistry>>` and use that everywhere.

## Cross-Module Notes

- **`src/looping/` ↔ `src/loop_graph/`**: clean separation. `loop_graph` is the topology/governance layer (graph nodes for `goal`/`cron`/`heartbeat`/`daemons`), `looping` is the in-memory executor for the `loop` tool. `loop_graph::service::notify_goal_settled` is called from `gateway/execution_engine/goal_continuation.rs:224`, but there is no analogous `notify_loop_settled` — by design, looping is too transient (one-session, "dies with the session" per `mod.rs:6-7`) to participate in the topology graph. The boundary is clean.
- **`src/looping/` ↔ `src/goal/`**: heavily cross-referenced, intentionally parallel. `goal::store::try_claim_continuation` (goal/store.rs:235-260) mirrors `looping::mod::try_claim_tick` exactly (same in-flight gate via `pending_*_ms`, same exhausted/stop branches). Both go through the same `gateway::execution_engine::execute.rs` continuation hook. The 1:1 symmetry is documented in the doc comments at goal/store.rs:40 and goal/store.rs:482. The two subsystems are sibling siblings, not duplicates.
- **`src/looping/` ↔ `src/builtin_tools/loop_manage.rs`**: the `loop` tool is the only mutator path that does NOT go through `commit_field_update` / `transition` — `start` calls `put()` directly (loop_manage.rs:390), bypassing the atomic lifecycle rules. This is correct because `start` replaces any prior loop wholesale, but it means the comment at `mod.rs:6-7` ("process memory only") is load-bearing: any other code path that re-loaded the registry from disk would diverge. No such code exists today.
- **`src/looping/` ↔ `src/scope/`**: `with_owner_scope` is the only consumer of `crate::scope::ScopeAttribution` here. `commit_field_update` (mod.rs:469-475) preserves `owner_user_id`/`scope_id` across field updates, which is the documented P1 data-isolation invariant. Note: `gateway/visibility.rs:85-102` documents that `looping::LoopState::scope_id` is "carried forward and read by nobody" — that is intentional and explicitly preserved by the comments. The dead wire is a deliberate one.
- **GLOBAL registry initialization** at `src/executor/builtin_registry/builder/constructor/mod.rs:536-537` happens at boot; the lazy-init pattern in tests is a workaround for unit tests that bypass boot. Not a real concern but worth documenting in `init_global`'s doc comment.

## Out of Scope / Skipped

- **R1 (platform-specific code)**: confirmed clean — `rg -n "cfg\(target_os|core_graphics|app_kit|objc"` over `src/looping/` returns zero matches. No platform imports.
- **Live run of `cargo check`/`clippy`/`test`**: explicitly skipped per directive. All findings are static.
- **Stress / concurrency testing**: no `std::thread` / `tokio::spawn` test in the module. Cannot verify the contract without runtime tests.
- **Memory leak / long-running daemon behavior**: `LoopRegistry` has no eviction, but the bound (one row per session ever) is small. Without a real daemon load test, cannot quantify.
- **`execute.rs` failure-routing paths** (`stop_loop_on_failure`, `park_loop_on_transient_failure`, `clear_loop_welded_strategy`): referenced and assumed correct. The looping side's contract is satisfied (`rearm_after_interruption`, `transition`); the execute.rs-side orchestration was not reviewed here.
- **`builtin_tools/loop_manage.rs` full audit**: only the `start` path's interaction with `put` was checked. The `update`/`pause`/`resume`/`status`/`list` paths were not.
- **`gateway/handlers/users.rs:1077-1079`** and **`gateway/handlers/session/db_handlers/modify.rs:1020-1024`** lazy-init patterns: flagged under `init_global` (low) but not deeply analyzed.
- **Whether `tick_prompt` ever leaks `state` snapshots to the caller**: pure function, no SideEffects — confirmed by reading the body. No data leak.
- **Whether `next_wake_ms` clearing at `mod.rs:337`** can cause a model-paced loop to busy-loop: comment at mod.rs:331-333 explicitly addresses this — model-paced loops fall back to `fallback_ms` after the first tick, which is a fixed large delay (10m default per `loop_manage.rs:341` `MODEL_PACED_FALLBACK_MS`). Not a busy-loop risk.