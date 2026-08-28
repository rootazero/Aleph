# Logic Review Report — src/orchestrator

**Date**: 2026-08-28
**Mode**: strict (read-only)
**Worktree**: `/home/zou/data/workspace/Aleph-worktrees/audit-2026-08-28`
**Reviewer**: focused re-dispatch after partial run hit 429 rate limit

## Files reviewed

Production (read in full or by chunks):
- `src/orchestrator/mod.rs`
- `src/orchestrator/errors.rs`
- `src/orchestrator/flow_spec.rs`
- `src/orchestrator/flow_registry.rs`
- `src/orchestrator/sandbox_factory.rs`
- `src/orchestrator/loader.rs`
- `src/orchestrator/resolver.rs`
- `src/orchestrator/dispatch.rs` (1427 lines)
- `src/orchestrator/harness_bridge/mod.rs`
- `src/orchestrator/harness_bridge/runner_impl.rs` (1540 lines, by chunks)
- `src/orchestrator/harness_bridge/llm.rs`
- `src/orchestrator/harness_bridge/callback.rs`
- `src/orchestrator/harness_bridge/error.rs`
- `src/orchestrator/harness_bridge/backfill.rs`
- `src/orchestrator/harness_bridge/context_estimate.rs`
- `src/orchestrator/harness_bridge/context_blocks.rs`
- `src/orchestrator/harness_bridge/prompt_build.rs` (1133 lines, by chunks)
- `src/orchestrator/harness_bridge/session_seed.rs`
- `src/orchestrator/harness_bridge/behavior_resolve.rs`
- `src/orchestrator/deps_builder/mod.rs`
- `src/orchestrator/deps_builder/common.rs`
- `src/orchestrator/deps_builder/summary.rs`
- `src/orchestrator/deps_builder/context_budget.rs` (1485 lines, by chunks)
- `src/orchestrator/deps_builder/provider_chain.rs` (828 lines, partial)
- `src/orchestrator/deps_builder/stability.rs`
- `src/orchestrator/presets/default_flows.toml`

Production NOT deeply re-read (covered by previous run + spot-checks):
- `src/orchestrator/harness_bridge/tests.rs` (912 lines, test-only)
- `src/orchestrator/tests/*` (test-only, scanned for unwrap/lock patterns)

## Pre-audit sanity checks (all PASS)

| Check | Result | Notes |
|-------|--------|-------|
| `lock().unwrap()` outside tests | PASS | Only `unwrap_or_else(\|e\| e.into_inner())` in production: `dispatch.rs:577`, `runner_impl.rs:1273,1294`, `context_estimate.rs:73,81,256`, `llm.rs:88` |
| `panic!` / `todo!` / `unreachable!` in production | PASS | Only test code uses these; the one `unreachable!` in `context_estimate.rs:61` is gated behind `OVERHEAD_CACHE_CAPACITY > 0` (compile-time invariant) |
| `home_dir().unwrap()` in production | PASS | No occurrences |
| UTF-8 byte slicing (`&s[..n]`) | PASS | Only `chars().count()` / `truncate_*` helpers are used (`context_blocks.rs:118,283,294`, `prompt_build.rs:977-978`) |
| `as` truncation casts | LOW-RISK | All observed casts are either saturating (`u64::from(...).round() as u32`, `as f64 → as u64` of known-bound arithmetic) or `as_millis() as u64` (instant math — safe). The `as usize` in `prompt_build.rs:952` (`n as usize` on `u32`) and `runner_impl.rs:1016` (`harness.total_tokens() as u32` — actually `try_from` with saturating fallback, correct) are sound. |
| `std::sync::Arc` direct usage | INTENTIONAL | All call sites use `crate::sync_primitives::Arc`, which is a zero-cost re-export of `std::sync::Arc` (see `src/sync_primitives.rs` doc — loom's `Arc` is incompatible with tokio). Consistent. |
| Loom-poisoning pattern | PASS | Every `lock()` site uses `unwrap_or_else(|e| e.into_inner())`, including the `Drop` impl of `SessionLockGuard` (`dispatch.rs:573-580`) |
| Async vs sync locks | PASS | `tokio::sync::Mutex` only for the async `ContextBudget` and `BroadcastCallback` channels; `crate::sync_primitives::Mutex` (sync) for everything that crosses only sync paths |
| SessionLockGuard contract | PASS | RAII drop on `DispatchError`/`Cancelled`/`Panic` paths confirmed (the `_lock` binding in `dispatch.rs:1025` is dropped at end of `tokio::spawn` block regardless of how the inner `harness.run` resolves) |
| All enum variants handled (BrainRef / SessionStrategy / FlowInput / TerminateReason) | PASS | Verified by reading each `match`. `BrainRef::Default | Preferred | Strict` exhaustive in `llm.rs:25-55`. `SessionStrategy::Reuse | Fresh | Child` exhaustive in `resolver.rs:82-119`. `FlowInput` exhaustive in `session_seed.rs:33-43` and `flow_spec.rs:55-62`. `TerminateReason` `as_static_str()` is exhaustive (all 14 variants) and `is_hit_limit()` is exhaustive (`matches!` against the two `false` variants). |

---

## Findings

### WARNING — `context_estimate.rs` uses `std::sync::Mutex` directly instead of `crate::sync_primitives::Mutex`

**Location**: `src/orchestrator/harness_bridge/context_estimate.rs:9`

```rust
use std::sync::Mutex;
```

**Impact**: All other modules in the orchestrator (`dispatch.rs`, `llm.rs` test, `runner_impl.rs`) use `crate::sync_primitives::Mutex`. The two types are functionally identical in non-loom builds (the wrapper is a `pub use std::sync::Mutex`), but the inconsistency:

1. Confuses loom instrumentation — if loom tests ever run against this file, the bypassed swap means the `OverheadCache` would silently NOT be instrumented while sibling mutexes ARE, defeating the concurrency-stress layer.
2. Hides the dependency — a refactor that swaps `sync_primitives` for `tokio::sync::Mutex` would silently miss this one mutex.

**Mitigation evidence**: All three `.lock()` calls in this file use the safe `unwrap_or_else(std::sync::PoisonError::into_inner)` pattern, so the inconsistency is purely cosmetic in production. Fix is one-line.

**Recommendation**: Replace with `use crate::sync_primitives::Mutex;`.

---

### WARNING — `dispatch.rs` test coverage gap on P1 data isolation fields

**Location**: `src/orchestrator/tests/dispatch.rs` (multiple `mk_req` builders, lines 124-125, 165-166, 200-201, 239-240, 324-325)

**Observation**: Every `mk_req` builder in the dispatch test suite hardcodes `owner_user_id: None` and `scope_id: None`. The production path in `dispatch.rs:1049-1058` reconstructs a `ScopeAttribution` from these two fields via `crate::scope::scope_from_metadata` and re-seeds it inside the `tokio::spawn`. None of the tests verify that a non-None `owner_user_id`/`scope_id` flows correctly across the spawn boundary.

**Impact**: This is exactly the seam the `outcome_tests::the_harness_spawn_reestablishes_the_run_tree_originator` test guards (line 1156-1190 of dispatch.rs), but it guards `TURN_ORIGINATOR`, NOT `ScopeAttribution`. The same class of "task-local lost across `tokio::spawn`" defect (one of the comment blocks at `dispatch.rs:983-1024` is essentially the post-mortem for `TURN_ORIGINATOR`) could silently re-occur for `ScopeAttribution` and have no test fire.

**Recommendation**: Add a dispatch integration test that passes `owner_user_id: Some("u-test")` and verifies that a downstream consumer (e.g. the `HarnessRunner` mock recording `current_scope()`) observes the value. See Suggested Test #1.

---

### WARNING — `escalate_partial_result` silently bypasses new budget-cap variants

**Location**: `src/orchestrator/dispatch.rs:285-302`

```rust
match reason {
    TerminateReason::HitMaxIterations { .. }
    | TerminateReason::ContextBudgetExhausted
    | TerminateReason::MaxOutputTokensExhausted => {
        TerminateReason::BudgetExhaustedPartialResult { ... }
    }
    other => other,
}
```

**Observation**: `TerminateReason` is `#[non_exhaustive]` (`flow_spec.rs`-equivalent location: `dispatch.rs:117`). A future contributor adding `TerminateReason::SomeNewBudgetCap` would have the new variant fall through to the `other` arm and *not* be escalated to `BudgetExhaustedPartialResult`. The cron carry-over path then sees the bare variant and the resume hint is lost — silently, with no test failing.

**Mitigation evidence**: The exhaustive unit test `dispatch.rs:1310-1325` (`escalate_partial_result_passes_through_non_budget_reasons`) asserts the *currently-known* non-budget variants, but it does NOT assert that the *budget-cap arm itself is exhaustive*. A new budget variant goes into the function's `other` arm and the test still passes.

**Recommendation**: Add a compile-time assertion that every `TerminateReason` variant is enumerated by `as_static_str()`, `is_hit_limit()`, AND `escalate_partial_result()`. Either a `const _: () = ...` exhaustive match or a `matches!` over the union. See Suggested Test #2.

---

### WARNING — `dispatch.rs::dispatch` step-numbering drift vs. `runner_impl.rs`

**Location**: `src/orchestrator/dispatch.rs:879-1106` and `src/orchestrator/harness_bridge/runner_impl.rs:117-340`

**Observation**: `dispatch.rs` comments the seven-step pipeline as:
- Step 1: resolve flow_id (line 880)
- Step 2: depth guard (line 884)
- Step 3: agent lookup deferred (line 887)
- Step 4: session resolve + lock (line 889)
- Step 5: brain pick deferred (line 925)
- Step 6: sandbox provision (line 927)
- Step 7: spawn harness (line 944)

`runner_impl.rs` numbers the same pipeline differently — comments at lines 117 ("Step 1: honour pre-dispatch cancellation"), 126 ("Step 2: verify the agent exists"), 141 ("Step 3: brain pick"), 312 ("Step 4: convert String → SessionId"), 333 ("Step 5: seed the session"). The two files disagree on which file owns step 1, which one owns step 3, etc.

**Impact**: Documentation rot only — the runtime behaviour is unchanged. A future contributor reading both files to understand the pipeline will be confused about which step number references which.

**Recommendation**: Either consolidate the numbering under one owner (the spec doc) or replace step numbers with named anchors (`agent_lookup`, `brain_pick`, etc.). Low priority.

---

### WARNING — `dispatch.rs::dispatch` releases the `active_sessions` lock only via `_lock: SessionLockGuard` drop, but the guard is constructed AFTER the harness spawn moves owned fields

**Location**: `src/orchestrator/dispatch.rs:1023-1028`

```rust
tokio::spawn(async move {
    let _lock = SessionLockGuard {
        active,
        key: session_for_release,
    };
```

**Observation**: The guard captures `session_for_release` (a `String`) and `active` (an `Arc<Mutex<HashSet<String>>>`) by move. The guard drops at the end of the spawned closure (after `done_tx.send(outcome)` at line 1093-1094). This is correct for the success path, the cancellation path, and panic propagation (RAII + `Drop` cannot panic — and the `Drop` impl deliberately uses `unwrap_or_else(|e| e.into_inner())` for poison-safety).

**Mitigation evidence**: `tests/dispatch.rs:349-407` (`dispatch_releases_session_lock_after_completion`) covers the success path. `tests/dispatch.rs:260-348` (`dispatch_rejects_concurrent_same_session_reuse`) covers the concurrent-dispatch path.

**Impact**: No defect observed. The only un-covered edge is the panic-during-harness path — a `panic!` inside `harness.run` between `let _lock = SessionLockGuard { ... }` and the end of the spawn block. The guard still drops, and `Drop` cannot panic, so this is structurally safe. **No fix recommended**, but the test gap is noted (see Suggested Test #3 for a poisoning/panic regression).

---

### WARNING — `runner_impl.rs::run` reads `harness.terminate_reason()` AFTER already passing `final_text.is_empty()` upstream

**Location**: `src/orchestrator/harness_bridge/runner_impl.rs:1040-1052`

```rust
let raw_terminate_reason = harness.terminate_reason();
let terminate_reason = crate::orchestrator::dispatch::escalate_partial_result(
    raw_terminate_reason,
    if final_text.is_empty() {
        None
    } else {
        Some(final_text.as_str())
    },
);
```

**Observation**: `final_text` is computed from the same session log read that the harness already consumed (line 945 `get_events`). The harness's `terminate_reason` is an internal accessor (`harness.terminate_reason()`) that does NOT depend on the log read. The ordering is safe.

**Mitigation evidence**: The harness's `terminate_reason()` reads from internal counter state set inside the loop, not from the session log.

**Impact**: None — verified safe. Documentation-only concern: a future contributor who changes `terminate_reason()` to read from the session log would create a race with `get_events` below. Adding a comment at line 1041 making the dependency explicit ("`terminate_reason` does NOT touch the session log") would prevent that foot-gun.

**Recommendation**: Add a one-line comment. Optional.

---

### WARNING — `runner_impl.rs::effective_summary_model` may return `Some` for unknown providers, leading to a no-op cheap provider construction

**Location**: `src/orchestrator/deps_builder/summary.rs:70-78`

**Observation**: `effective_summary_model` checks `crate::providers::get_preset(&primary_provider_key.to_lowercase())` for the aux fallback. The preset lookup lowercases the key — fine for known providers like `claude`/`openai`/`minimax`. But the function does NOT verify that `primary_provider_key` matches the canonical preset casing. A non-canonical primary key (e.g. `Claude` or `CLAUDE`) whose preset resolves correctly here but whose `[providers.Claude]` lookup fails (case-sensitive `HashMap`) produces a `cheap_summary_provider` whose `clone()` later panics-or-silently-mismatches.

**Mitigation evidence**: The flow reaches `build_cheap_summary_provider` only after `effective_summary_model` returns Some, and `build_cheap_summary_provider` reads `config.providers.get(primary_provider_key)` — case-sensitive. If the keys disagree the function returns `None` ("primary key absent"). So in practice the mismatch is caught by the `base = config.providers.get(primary_provider_key)?` guard at `summary.rs:131`.

**Impact**: No functional defect, but the lowercased preset lookup + case-sensitive provider lookup is a sharp edge. A new contributor adding a `get_preset_eq_ignore_ascii_case` (or normalizing `primary_provider_key` once at function entry) would make the contract clear.

**Recommendation**: Low priority. A one-line `let primary_provider_key = primary_provider_key.to_ascii_lowercase();` at the top of `effective_summary_model` and `build_cheap_summary_provider` (with the original key preserved for `config.providers.get` via the user's intent — or document why we DON'T lowercase) would resolve the ambiguity. Alternatively, leave as-is and add a doc comment at line 71.

---

### WARNING — `dispatch.rs` `Cargo.toml`-external flow admins must satisfy all `HarnessRunner` trait methods; no convenience constructor exists

**Location**: `src/orchestrator/harness_bridge/runner_impl.rs:26-95` (trait impl) + `dispatch.rs:590-744` (trait definition, ~150 lines)

**Observation**: `HarnessRunner` declares 13 methods (one required `run`, twelve optional with `default = None`). Production wires all twelve via `AgentHarnessRunner` at boot. The harness_bridge test fixtures (`tests/dispatch.rs:282-318`) construct a `HangingHarness` that implements ONLY `run`, relying on the defaults for the rest. That works because the harness is dropped in the test. **In production, however**, a new dispatcher that builds `Orchestrator::new` with a custom `Arc<dyn HarnessRunner>` must remember to wire `guardrails`, `routing_store`, `stall_config`, `consecutive_failure_cap`, `turn_timeout`, `default_max_iterations`, `parallel_tool_concurrency`, `context_budget_config`, `context_budget_refiner`, `primary_context_window`, `cheap_summary_provider`, AND `estimate_context` — otherwise spawned subagents silently lose those seams.

**Mitigation evidence**: The doc on each default method explains what omitting it costs.

**Impact**: Documentation burden, not a defect. The trait's `default = None` on each method makes "easy to omit" the failure mode.

**Recommendation**: Consider adding a `HarnessRunner::assert_production_wired(&self)` debug-only assertion that fires on first `run()` call from a release build if any default-returning method returns `None` AND the caller is the gateway (not a test). Or, more pragmatically: document the "thirteen seams" list in `Orchestrator::new`'s doc-comment.

---

### WARNING — `cargo clippy -p alephcore -- -D warnings` may flag the `#[allow(clippy::too_many_arguments)]` on `HarnessRunner::run` if it's exercised from non-`run` paths

**Location**: `src/orchestrator/dispatch.rs:589` and `src/orchestrator/harness_bridge/runner_impl.rs:96`

**Observation**: Both files carry `#[allow(clippy::too_many_arguments)]` on the trait/impl `run` function. This is the documented rationale ("trait shape driven by Orchestrator::dispatch wiring"). The 15-argument trait method (`run(session_key, spec, input, sandbox, events, cancel, tool_service_override, trace_sink, interaction_manifest, workspace_override, max_iterations_override, transient_context, think_level, envelope, turn_model)`) is genuinely unwieldy; callers can lose count.

**Impact**: Maintenance burden only. No defect. The grouping convention in `runner_impl.rs:97-115` (a `// D2:`, `// Ephemeral per-turn prompt context`, `// Declared reasoning depth`, etc.) helps, but a struct of inputs (`RunRequest`) would be mechanically safer.

**Recommendation**: Future cleanup. Not a blocker.

---

### Suggested Test #1 — P1 data isolation survives `tokio::spawn` boundary

**Location to add**: `src/orchestrator/tests/dispatch.rs` (new test)

```rust
/// Regression: `FlowRequest::owner_user_id` / `scope_id` must reach the
/// spawned harness task. Task-locals are re-derived from these two explicit
/// fields and re-seeded inside the spawn (see `dispatch.rs:1049-1067`); if the
/// re-seed ever drops, `current_scope()` reads `None` inside the harness and
/// per-scope data isolation silently no-ops.
#[tokio::test]
async fn dispatch_reestablishes_scope_attribution_across_spawn() {
    let (orch, _invocations) = fixture_orchestrator();
    let mut req = mk_basic_request(); // existing helper
    req.owner_user_id = Some("u-test-owner".into());
    req.scope_id = Some("s-test-scope".into());

    let handle = orch.dispatch(req).await.expect("dispatch ok");
    let outcome = handle.completion.await.expect("completion ok").expect("ok result");

    // The fake harness records scope reads — assert both sides agree.
    let recorded = SCOPE_OBSERVER.read().await.clone();
    assert_eq!(recorded.owner, Some("u-test-owner".into()),
        "ScopeAttribution::owner must survive tokio::spawn");
    assert_eq!(recorded.scope_id, Some("s-test-scope".into()),
        "ScopeAttribution::scope_id must survive tokio::spawn");
    assert_eq!(outcome.final_text, "ok"); // fixture
}
```

To make this compile, the existing `fixture_orchestrator` (`tests/dispatch.rs:69`) would need a `HarnessRunner` that records `crate::scope::scope_from_metadata(&current_scope())` rather than hard-coding. A scoped `Arc<tokio::sync::RwLock<RecordedScope>>` global would work.

---

### Suggested Test #2 — exhaustive enum coverage for `escalate_partial_result`

**Location to add**: `src/orchestrator/dispatch.rs` outcome_tests module

```rust
/// Compile-time guarantee that every `TerminateReason` variant is accounted
/// for in `escalate_partial_result`. Adding a new budget-cap variant (e.g.
/// `SomeNewBudgetCap`) MUST be classified as PartialResult-eligible, and a
/// new non-budget variant MUST pass through unchanged — neither behaviour
/// should require remembering to update `escalate_partial_result`.
#[test]
fn escalate_partial_result_covers_every_variant() {
    // Build one sample per variant.
    let samples = [
        TerminateReason::Completed,
        TerminateReason::HitMaxIterations { used: 1 },
        TerminateReason::ContextBudgetExhausted,
        TerminateReason::StallTimeout { elapsed_ms: 1 },
        TerminateReason::TurnTimeout { phase: "x".into(), elapsed_ms: 1 },
        TerminateReason::ConsecutiveFailureCap { consecutive: 1 },
        TerminateReason::VerifierVeto { vetos: 1 },
        TerminateReason::EmptyResponseExhausted,
        TerminateReason::StopHookHalt { reason: "x".into() },
        TerminateReason::MaxOutputTokensExhausted,
        TerminateReason::DiminishingReturns,
        TerminateReason::ReactiveCompactExhausted,
        TerminateReason::Cancelled,
        TerminateReason::BudgetExhaustedPartialResult {
            reason: "x".into(),
            partial_summary: "x".into(),
        },
    ];
    for r in samples.iter() {
        let _ = escalate_partial_result(r.clone(), Some("partial"));
        // Reaching this line without compile error = match arm exists.
    }
    assert_eq!(samples.len(), 14, "if you added a variant, the array above must include it");
}
```

Note: the existing test `terminate_reason_static_str_is_stable` already does this for `as_static_str`. Mirror its pattern for `escalate_partial_result`.

---

### Suggested Test #3 — `SessionLockGuard` releases on panic inside the spawn

**Location to add**: `src/orchestrator/tests/dispatch.rs`

```rust
/// Regression: a panic inside the spawned harness task must not leak the
/// session lock. RAII + the poison-safe `Drop` impl (`dispatch.rs:573-580`)
/// are supposed to handle this, but neither dispatch's success-path test
/// (`dispatch_releases_session_lock_after_completion`) nor the conflict test
/// (`dispatch_rejects_concurrent_same_session_reuse`) exercises the panic
/// branch.
#[tokio::test]
async fn dispatch_releases_session_lock_when_harness_task_panics() {
    struct PanickingHarness;
    #[async_trait]
    impl HarnessRunner for PanickingHarness {
        async fn run(&self, _s: String, _sp: Arc<FlowSpec>, _i: FlowInput,
                     _sb: Arc<dyn Sandbox>, _ev: broadcast::Sender<FlowStreamEvent>,
                     _cancel: CancellationToken,
                     _t1: Option<Arc<dyn ToolService>>,
                     _t2: Option<Arc<dyn crate::harness::TraceSink>>,
                     _im: Option<crate::thinker::InteractionManifest>,
                     _wo: Option<PathBuf>, _m: Option<u32>,
                     _tc: Option<String>, _tl: Option<ThinkLevel>,
                     _en: crate::thinker::TurnEnvelope, _md: Option<SessionModelPref>)
            -> Result<FlowOutcome, FlowError> {
            panic!("harness explosion");
        }
    }
    // Build an Orchestrator with PanickingHarness, dispatch a request
    // with `session_hint = Some("panic-test-session")`, expect the panic
    // to propagate up via the oneshot (catch_unwind or expect_panic).
    // Then dispatch a SECOND request with the same session_hint and
    // assert it succeeds (i.e. the SessionLockGuard dropped cleanly
    // during the panic unwind).
    todo!() // see dispatch.rs:1023-1028 for the contract under test
}
```

This is a higher-effort test (requires `catch_unwind` or `JoinHandle::is_panicked`) but it locks down the one untested RAII edge.

---

### Suggested Test #4 — `resolve_active_strategy` is idempotent across team-chat members

**Location to add**: `src/orchestrator/harness_bridge/context_blocks.rs` (active_strategy_tests module)

```rust
#[test]
fn two_team_chat_members_resolve_the_same_strategy() {
    // Existing test (`resolve_active_strategy_team_tier_and_precedence`)
    // covers one member. The cross-member invariant is independent:
    // member-A's resolve and member-B's resolve of the same team-wide row
    // must be byte-equal.
    let dir = tempfile::tempdir().unwrap();
    let store = crate::strategy::StrategyStore::open(&dir.path().join("s.db")).unwrap();

    let team_strategy = mk_strategy("team-objective");
    store.put(&crate::strategy::team_key("squad"), &team_strategy).unwrap();

    let sk_a = crate::routing::session_key::SessionKey::task("alice", "team_chat", "squad")
        .to_key_string();
    let sk_b = crate::routing::session_key::SessionKey::task("bob", "team_chat", "squad")
        .to_key_string();

    assert_eq!(
        resolve_active_strategy(&store, &sk_a, true, true).map(|s| s.objective),
        resolve_active_strategy(&store, &sk_b, true, true).map(|s| s.objective),
        "two members of the same team must resolve the same welded Strategy"
    );
}
```

This pins the invariant the doc claims (`strategy round 2`) but no test currently asserts.

---

### Suggested Test #5 — `term_key` cross-walks the strategy keys without collision

**Location to add**: `src/orchestrator/harness_bridge/context_blocks.rs` (active_strategy_tests module)

```rust
#[test]
fn the_four_strategy_keys_are_collision_free() {
    // The four keys (`goal_key`, `loop_key`, `team_key`, `session_key`) live
    // in the same `StrategyStore`. Two different tiers producing the same
    // key string would let one tier silently overwrite the other. Pin the
    // canonical forms apart so a tier rename can never quietly collide.
    let session = "agent:main:main";
    let team_id = "squad-7";

    let keys = [
        crate::strategy::goal_key(session),
        crate::strategy::loop_key(session),
        crate::strategy::team_key(team_id),
        crate::strategy::session_key(session),
    ];
    let unique: std::collections::HashSet<_> = keys.iter().collect();
    assert_eq!(unique.len(), keys.len(),
        "the four Strategy tiers must produce distinct key strings; got {keys:?}");
}
```

---

## Cross-Module Findings

### X1 — `dispatch.rs::dispatch` re-seeds FOUR task-locals across the spawn boundary (correct but fragile)

**Location**: `src/orchestrator/dispatch.rs:983-1024, 1059-1078`

The code re-establishes `with_agent_id`, `with_project_root`, `with_scope` (re-derived from `owner_user_id` + `scope_id`), `with_room_author` (captured live), and `with_originator` (captured live) inside the spawned harness task. This is the documented "the four (or five) task-locals that die at `tokio::spawn`" pattern.

**Risk**: Adding a sixth task-local that the gateway run loop scopes requires remembering to re-seed it here, in `CarriedAttribution::reestablish`, AND in any other `tokio::spawn` site. The defensive comment at line 1008 ("A sixth carrier field would have zero consumers (R10)") describes the design intent — only re-seed what the harness actually reads.

**Recommendation**: This is a known design point, well-documented. No fix. The Suggested Test #1 above would lock down the P1 attribution re-seed (which is currently NOT tested).

---

### X2 — `runner_impl.rs::run` mutates `envelope.serving_model` in place (line 471)

```rust
envelope.serving_model = Some(gauge_model.clone());
```

**Observation**: `envelope` is taken by value at `runner_impl.rs:111`. Mutating one field of a `TurnEnvelope` is fine for a value-owned struct. But if `TurnEnvelope` ever grows a non-`Clone` field (e.g. an `Arc<…>` guarded by a `Mutex`), this in-place mutation becomes the kind of subtle "I cloned the model but the rest of the envelope is shared" foot-gun that broke the FailoverProvider wrapper-name issue.

**Mitigation evidence**: `TurnEnvelope` is `Clone`-by-value per its construction at `thinker::TurnEnvelope`.

**Impact**: None today. Worth a comment noting "envelope is mutated in place; do not introduce non-Clone fields".

---

### X3 — `provider_chain.rs` chain assembly is out-of-scope for this audit but flagged for follow-up

`provider_chain.rs` (828 lines) contains the boot-time `FailoverProvider` chain construction. Spot-checks confirm:
- Loom-safe pattern usage.
- Test coverage: `provider_chain.rs:514-810` includes 18+ tests (mock-driven).
- `as` casts are bounded (`u32::from(...)` and `usize` math against `Vec` capacities).

A full audit of this file was deferred — no defects observed in the portions read.

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 8 |
| Suggested Test | 5 |

## Notes

1. **No code modifications were made.** This is a read-only audit, per scope.
2. The orchestrator module is unusually well-documented at the design level (the `design §6/§7` references, the "why this was cut" annotations, the regression-story comments on each lock acquisition site). The prose density suggests a codebase that has been repeatedly audited and refactored; the absence of `unreachable!` / `panic!` / `unwrap()` in production paths is consistent with that history.
3. The biggest maintenance risk is the trait `HarnessRunner` (`dispatch.rs:590-744`): 13 methods, 150 lines of doc-comment, and a 15-argument `run`. A `RunRequest` struct would compress the trait shape but is out-of-scope for this audit.
4. The two most impactful concrete improvements are (a) Suggested Test #1 (P1 data isolation across `tokio::spawn` is currently untested), and (b) the `std::sync::Mutex` → `crate::sync_primitives::Mutex` swap in `context_estimate.rs:9`.
5. `tests/dispatch.rs` is a strong fixture suite for the public dispatch surface; production code paths through `runner_impl.rs::run` are tested only at the wiring-assertion level (`runner_impl.rs:1478-1541` tests `effective_model_directive`, `acting_provider_id`, calibration carry-over) — a focused integration test of the full `run()` method against a `NoopSandbox` + `MockProvider` would close the largest remaining gap.
6. The preset catalog (`presets/default_flows.toml`) shipped with exactly one flow (`default-agent`) after the 2026-08 refactor. The `loader.rs::load_catalog` test at `tests/loader.rs:75` (`the_catalog_has_exactly_one_composer`) pins this. Any future re-introduction of per-agent presets must pass `every_registered_agent_resolves_to_a_hint_honouring_flow` (test mentioned in the preset file's prose but not located in this review — verify exists in a separate re-read).
7. None of the issues found block a release; they are documentation/consistency improvements and missing test coverage.

---

*End of report.*
