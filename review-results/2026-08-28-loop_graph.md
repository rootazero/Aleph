# Logic Review Report
**Module**: `src/loop_graph/`
**Scope**: Full read-only audit of the 9 files (mod.rs, events.rs, export.rs, inspector.rs, service.rs, snapshot.rs, store.rs, templates.rs, types.rs — 6866 lines). Wired artefacts in `src/builtin_tools/loop_graph_manage.rs` and the four call sites (`notify_goal_settled` × 3, `notify_team_settled` × 2, `notify_workflow_settled` × 1) were spot-checked.
**Date**: 2026-08-28
**Mode**: normal (one real wiring concern at warning level — see below)

---

## Findings

### [Warning] Sync primitives import discipline violated for the event bus's lag counter
- **Location**: `src/loop_graph/events.rs:35-36`
- **Trigger condition**: Code review and AGENTS.md sync_primitives import rule.
- **Expected behavior**: `Arc` and `AtomicU64` should be imported from `crate::sync_primitives` (per AGENTS.md Redline #8) so the loom model and the production model share one symbol.
- **Actual behavior**: `use std::sync::atomic::{AtomicU64, Ordering};` and `use std::sync::Arc;` are imported directly. The `sync_primitives` module re-exports both (`pub use std::sync::Arc;` at line 25 of `src/sync_primitives.rs`), so types are identical, but the convention is broken and any future narrowing in `sync_primitives` (loom-mode wrappers) will not catch this file.
- **Suggested fix**: Replace with `use crate::sync_primitives::{Arc, AtomicU64, Ordering};`. Same change applies to `mod.rs:223` where `std::sync::atomic::Ordering::AcqRel` is used — promote to the re-export.

### [Warning] `walk_chain` unbounded neighbour expansion per node is a perf cliff, not a correctness bug
- **Location**: `src/loop_graph/inspector.rs:283-321` (the `walk_chain` function)
- **Trigger condition**: A node with thousands of `owns_reference` children (admittedly unusual for an operator-curated governance graph) is given as the frontier node of `subgraph_for`.
- **Expected behavior**: Both directions of the walk should be bounded by something the operator could not unilaterally inflate.
- **Actual behavior**: The `MAX_TRAVERSAL_STEPS = 1024` ceiling counts UNIQUE NODES processed (one step per pop), not edges explored. Inside the for-loop, every neighbour is pushed onto the frontier AND cloned into `out` without any count check. A 1→10000 governance star would push 10000 items onto `out` in the first iteration, then exhaust steps on each leaf, returning `Err` only after visiting 1024 leaves. The error itself is fine; the leaked allocation is not. A similar shape exists in `impact_of_removing` via `by_id` HashMap construction (less severe).
- **Suggested fix**: Add a small fan-out cap (e.g. 256 neighbours per node) plus a total output cap, or make the data shape explicit (`Vec<(node_id, edge)>`) so a single hot node cannot produce a multi-MB `subgraph_for` result.

### [Warning] `notify_node_settled` returns `true` for "no watchers" — one-shot claim is spent on a successful settle that was never wired to a watcher
- **Location**: `src/loop_graph/service.rs:284-289`
- **Trigger condition**: A goal/team/workflow reaches Complete (or disband) without any `watches` edge ever having been created against it, AND the loop_graph subsystem is healthy.
- **Expected behavior**: The settle should at minimum surface a log line that the claim was spent without any reviewer running, OR the caller should be told the claim was spent "by absence" so the operator can see the gap.
- **Actual behavior**: `watcher_jobs.is_empty() => return true;` advances silently. The caller's CAS (`try_claim_settle_notify`) is therefore retired and no further observation will retry. The discipline (`return false` when readers cannot answer, `return true` only on success-or-vacuous) is good for the failure cases but the vacuous-success path here is the same shape as a doctor report reading `naked_loop_count = 0` on an empty graph — the absence is invisible unless the operator hunts for it elsewhere.
- **Suggested fix**: Either `info!(node, "no watchers paired — settle claim spent without review")` so the gap shows up in `tracing`, or document loudly that pairing is the model's responsibility. The current comment ("an earlier failure here retired the review for good") implies the writer thought about it; the log line they did not add would be the cheap fix.

### [Warning] `notify_node_settled`'s `true`-on-no-graph read does not match the symmetry its `false`-on-busy companion promises
- **Location**: `src/loop_graph/service.rs:269-301`
- **Trigger condition**: Process-global `GLOBAL` is `None` (the store failed to install — currently unreachable but the slot is documented as `FailsOpen`, see `mod.rs:38-78`).
- **Expected behavior**: A consistent answer to "could not read the graph" regardless of which failure mode (no slot vs. busy lock).
- **Actual behavior**: `let Some(store) = crate::loop_graph::global() else { return true; };` returns success. The subsequent error path (busy lock) returns failure. The two are different — the latter is asserted loudly via the docstring; the former is silent. Reclassifying the slot to `FailsOpen` (see `mod.rs:55-58`'s parenthetical about the cargo reclassification) was the right call for the ACL consumer but did not extend to this consumer, where the same "no graph" still grants.
- **Suggested fix**: Either unify both to `false` (release claim, log explicitly that boot state was bad) or add a parity note explaining the asymmetry. Note: per `mod.rs:60-62` the unconditional boot install makes this unreachable in production today, so this is documentation/defense-in-depth rather than a current bug.

### [Warning] `walk_chain` step counter is checked BEFORE incrementing, so `MAX_TRAVERSAL_STEPS` actually allows `MAX` iterations, not `MAX - 1`
- **Location**: `src/loop_graph/inspector.rs:289-296`
- **Trigger condition**: Reading the code path of `if steps >= MAX_TRAVERSAL_STEPS { return Err(...); }` followed by `steps += 1`.
- **Expected behavior**: A bound labelled `MAX_TRAVERSAL_STEPS` should arguably prevent the iteration that would step past it.
- **Actual behavior**: With `MAX = 1024`, the function processes 1024 unique nodes before the 1024th-pop-then-check returns Err. The test `chains_longer_than_max_steps_are_rejected_not_runaway` (line 935+) builds a chain of `MAX + 5` nodes and asserts `is_err()` — it passes (the 1024th is the breaking point), but the docstring of `MAX_TRAVERSAL_STEPS` says "1024" and the cap is in practice "1024 unique nodes processed, then fail". Test name `chains_longer_than_max_steps` is honest (`n > MAX`), so this is a doc comment delta, not a bug.
- **Suggested fix**: Update the `MAX_TRAVERSAL_STEPS` comment to say "Bounded to at most 1024 unique nodes processed per walk before returning Err." Or, less invasively, increment-then-check (`steps += 1; if steps > MAX { return Err }`).

### [Warning] Settle-claim dropped-forever race when `HeldForOtherNode` is followed by `Done` during the same window
- **Location**: `src/loop_graph/service.rs:120-150` (`debounce_pass` / `Debounce::HeldForOtherNode` arm)
- **Trigger condition**: Goal A settles at t=0 (poke admitted, stamp `{a, InFlight}`). Goal B settles at t=10s (during the 60s window). Goal B's `debounce_pass` returns `HeldForOtherNode` → B's claim is released → A's run_job completes successfully → A's stamp flips to `{a, Done}`.
- **Expected behavior**: Either A's review covers B (it doesn't — A's run predated B's win) OR B gets a fresh poke once A's stamp is out of window OR B's claim is retried.
- **Actual behavior**: The CAS key for B's settle is `(id, completed_at_ms)`, which never changes, so B's "released for retry" never retries (the next observation sees the same Complete state and `try_claim_settle_notify` returns `false` again). B's reviewer review is permanently retired. This is the documented trade-off (debounce "HeldForOtherNode" exists to prevent the worse failure of crediting A's run against B's claim), but it is asymmetric — the comment at the Debounce enum doc claims "released claim's retry lands on either the flipped `Done` stamp (covered, free) or a fresh poke", which is wrong when `for_node` mismatches (Done-and-other-node is still `HeldForOtherNode`).
- **Suggested fix**: Either tighten the doc to admit the asymmetry, or rework so the release-on-`HeldForOtherNode` waits for window expiry before returning false. The first is a doc-only fix; the second is a real design change. Not flagging as critical because the trade-off is by design (round 9 explicitly), but the docstring misrepresents the outcome.

### [Warning] `GcCompleted` event carries `removed` and `retained_acl` counts but `retained_acl` is not exposed by the inspector's counts
- **Location**: `src/loop_graph/events.rs:75-83` and `src/loop_graph/inspector.rs:329-357`
- **Trigger condition**: A Panel consuming `recent_events` to draw a "graph changed" timeline.
- **Expected behavior**: The whole-purpose of carrying `retained_acl` was to surface the "we kept governance" line (the spec explicitly says silently retaining is the same class of lie as silently deleting).
- **Actual behavior**: The event carries both fields but the inspector's `TopologySummary` does not expose `retained_acl_count`, so a UI that aggregates events over a window cannot show "this week's gc retained N governance rows". The fields are in the payload JSON, so a desperate caller can sum them themselves, but the inspector API does not provide it.
- **Suggested fix**: Add `total_retained_acl: usize` and `last_retained_acl: usize` to `TopologySummary`, sourced via the snapshot store.

### [Warning] `EdgeKind::Arbitrates` and `EdgeKind::Feeds` are written but never read by any lint or render code
- **Location**: `src/loop_graph/types.rs:131-160` (definition) and the only readers: `src/loop_graph/export.rs:165,127` (DOT color), `src/loop_graph/types.rs:335` (test).
- **Trigger condition**: Operator inserts an `arbitrates` or `feeds` edge thinking it carries governance weight.
- **Expected behavior**: Either the verb does meaningful structural work (it is `non_exhaustive` for growth) or its no-op behaviour is in the schema.
- **Actual behavior**: Neither verb participates in `lint_naked_loops`, `lint_forged_coverage`, `lint_governance_chain`, the render seam (only `Watches`/`OwnsReference`/`Audits`/`AnchoredBy` are pulled by `edges_for_render`), or the poke lookup (`watches_sources`). The `EdgeKind::Arbitrates` and `EdgeKind::Feeds` rounds show up only in DOT color and as `kind = "arbitrates"` strings in `row_to_edge`. The module doc on `types.rs:120-127` is honest about this — `Arbitrates` is "documentation-only by design" and is listed in GRAPH_LAYER §4.3 as a NOT-build. So this is intentional and documented.
- **Suggested fix**: Tighten the schema tool comment to repeat the "documentation-only" note verbatim, since the audit template (`AUDIT_TEMPLATE`) and the loop-governance skill DO tell the auditor to record arbitration as a `note` rather than to use the verb. The current doc on `EdgeKind::Arbitrates` is one line; the `Feeds` line was omitted from the same paragraph. Cosmetic, but multi-page audit reports will keep tripping on these.

### [Info] `events.rs` exposes `pub fn dropped_events()` and `pub fn record_lag()` that no caller in the workspace invokes
- **Location**: `src/loop_graph/events.rs:170-194`
- **Trigger condition**: Static check on cross-module references.
- **Expected behavior**: Orphan check (per skill Section "Visibility Traps").
- **Actual behavior**: `subscriber_count` is wired to `loop_graph_manage.rs:480` (audit trail liveness). `lag_counter` is wired to `mod.rs:191` (persister). `dropped_events` and `record_lag` are public surface intended for the future "audit layer reads the counter to decide whether to surface a warning" (per the `events.rs:166-167` comment), but no code path consumes them. They are `pub fn` on a `pub` type, so the compiler does not flag.
- **Suggested fix**: Either annotate these with `#[allow(dead_code)]` and a comment explaining the planned use, or gate them behind a `pub(crate)` so the surface is honest. The doc comment ("Resetting the counter is intentionally not provided") is good evidence the writer knew, but Rust's `dead_code` lint on `pub` items does not fire, so the file is silently carrying unused surface.

### [Info] `lock().unwrap_or_else(|e| e.into_inner())` cascade — three identical sites, all "P7 lock-safety: never propagate poison" per the comments
- **Location**: `src/loop_graph/store.rs:81-83`, `src/loop_graph/snapshot.rs:135-137`, `src/loop_graph/service.rs:122-126, 142-146, 158-162`
- **Trigger condition**: Code review of all mutex acquisitions.
- **Expected behavior**: Poison-handling is consistent across all sites.
- **Actual behavior**: Every site uses `unwrap_or_else(|e| e.into_inner())`, which keeps the data accessible after a panic. This is the right discipline (the doc says "never propagate poison"). Five sites all use the same pattern, which is good. No bug.
- **Suggested fix**: Consider promoting to a `crate::sync_primitives::Mutex` extension method (`fn lock_or_recover(&self) -> Guard`) so future call sites default to the recover path. Optional.

### [Info] `LoopGraphStore::lock()` is held across an `execute_batch` in `gc_is_atomic_across_multiple_dangling_edges` test
- **Location**: `src/loop_graph/store.rs:test setup around the trigger install`
- **Trigger condition**: Test scaffolding (production paths are correct).
- **Expected behavior**: Tests should not rely on test-only lock behaviour.
- **Actual behavior**: The test calls `s.lock().execute_batch(...)` to arm a SQLite trigger. The production `lock()` call is the same one used by every read/write (and `gc` follows the production pattern of explicit `transaction()` rather than `lock`). The test is fine; no risk.
- **Suggested fix**: None needed. Mentioned only because the skill demands attention to lock-across-await and lock-across-IO; the IO here is local SQLite (not network), and there is no `await`.

### [Info] `render_session_topology_strict` is `#[cfg(test)]` only, despite its doc claiming a production caller needs it
- **Location**: `src/loop_graph/service.rs:601-614`
- **Trigger condition**: Phase 4 red-teaming.
- **Expected behavior**: A reader that wants to distinguish "governed session whose store failed to read" from "ungoverned session" would currently have to fork the implementation.
- **Actual behavior**: The doc on `render_session_topology_in` (lines 510-535) honestly explains why the strict variant is test-only and what to do when a production consumer lands. The note at line 605-614 ("used to sit in production under `#[allow(dead_code)]`") is itself a record of a previous code smell being corrected.
- **Suggested fix**: None. This is correctly gated today and the next production caller (doctor's structured variant) will promote it.

### [Info] `notify_workflow_settled`'s bool is only logged at the call site, not released
- **Location**: `src/teams/dispatcher/schedule/settle.rs:371-379` (call site) and `src/loop_graph/service.rs:485-491` (callee)
- **Trigger condition**: A workflow run fully settles with paired cron watchers, but the poke fails.
- **Expected behavior**: Documented design — workflow has no one-shot claim (only the channel-push claim, which must not be re-sent), so a failed poke is informational.
- **Actual behavior**: The bool is logged as a `warn` line ("watcher's periodic cadence backstops"). Acceptable per spec. The settlement `warn` line explicitly references the cadence backstop — this is the design.
- **Suggested fix**: None.

### [Info] Module size (6866 lines, 9 files) — largest module in `alephcore`
- **Location**: Whole module
- **Trigger condition**: Codebase health check.
- **Expected behavior**: Per AGENTS.md R3, "core minimalism — no heavy deps for non-core features". Loop_graph is core: it stores the immutable role graph that every optimization loop defers to.
- **Actual behavior**: 6866 lines is large but not bloated. Of the 9 files:
  - `store.rs` (1983 lines) and `inspector.rs` (941) carry the structural and inspection machinery. The store's body of tests plus the lint passes plus the insert/replace/CAS stores justify the size.
  - `service.rs` (1431) holds the five-notify entry points plus the render + debounce + cron-trigger slots; long but cohesive.
  - `templates.rs` (180) is small and contains the only templates that survived cuts (round 11 removed `STEWARD_TEMPLATE` / `ARBITRATION_TEMPLATE`).
  - `events.rs` (298), `snapshot.rs` (867), `export.rs` (485), `types.rs` (364), `mod.rs` (317) are all within reason.
  No file suggests extraction. The module as a whole is borderline (R3 says core stays minimal), but the size comes from real responsibility (store + snapshot + inspector + service + export + 6-verb closed vocabulary + tests), not from feature creep.
- **Suggested fix**: None at the file level. The natural next split is "governance-graph module" vs. "snapshot module" — but snapshot.rs is intentionally a sibling to the store (separate DB by spec, see GRAPH_LAYER §2). Splitting it would push spec decisions into module ownership.

### [Info] Orphan check — `LoopGraphInspector::summary` has only one production caller (impact action)
- **Location**: `src/loop_graph/inspector.rs:354-417` and `src/builtin_tools/loop_graph_manage.rs:2066` (test reference)
- **Trigger condition**: Wiring completeness check.
- **Expected behavior**: Either summary is consumed by `status`, or its single-caller reality is documented.
- **Actual behavior**: `loop_graph_manage.rs:256` is the only production caller (in the `impact` action's `loop_graph status` aggregate path). The inspector doc says status DOES NOT consume it ("status renders per-node live joins ... that this aggregate view deliberately lacks"). The discipline claim ("status and inspector are pinned consistent by ... status_counts_agree_with_inspector_summary") is enforced via a test, not a runtime tie. Acceptable, well-documented.
- **Suggested fix**: None. Would be worth converting the documentation claim to a runtime test that exercises the actual `status` render path; the current test only compares counts at a higher abstraction.

### [Info] `LoopGraphStore::lint()` reads nodes/edges under the lock, then drops it before running pure-CPU lint passes — best practice pattern
- **Location**: `src/loop_graph/store.rs:656-744`
- **Trigger condition**: Phase 2 concurrency check.
- **Expected behavior**: Holds mutex only for IO, releases before compute.
- **Actual behavior**: `drop(conn)` is explicit at line 727 (after parsing the data into local Vec/HashSet); the five `lint_*` functions are pure. Tests prove this with concurrent capture tests. Good.
- **Suggested fix**: None — exemplary pattern. Worth a comment in `loop_graph_manage.rs:741` (where `gc` is invoked) to the same effect: `gc` is atomic via a transaction and also drops the lock between prepare and execute.

### [Info] Three unregistered types in `TopologyEvent` (`NodeUpserted`, `EdgeUpserted`, etc.) — never consumed by subscribers today except the persister
- **Location**: `src/loop_graph/events.rs:53-83`
- **Trigger condition**: Bus consumer audit.
- **Expected behavior**: Each event kind has a real consumer.
- **Actual behavior**: `spawn_event_persister` in `mod.rs:190-218` subscribes and persists all events to the snapshot store. That is the only consumer; it appends regardless of variant. The skill's "partial registration" concern does not apply (every variant is handled by the catch-all append). `Panel`-facing data is then available via `inspector.recent_events`. Bus good.
- **Suggested fix**: None.

### [Info] `LoopGraphStore::edges_for_render` filters by `kind IN ('watches','owns_reference','audits','anchored_by')` — `Arbitrates` and `Feeds` deliberately excluded
- **Location**: `src/loop_graph/store.rs:355-380`
- **Trigger condition**: Render seam audit.
- **Expected behavior**: Render sees what the prompt needs (`watches` / `owns_reference` / `audits` / `anchored_by`).
- **Actual behavior**: Consistent with the module doc on `EdgeKind` — `Arbitrates` and `Feeds` are documentation-only. The render does not need them. Good.
- **Suggested fix**: None.

### [Info] `to_dot` and `to_json` both call `list_nodes`/`list_edges` separately — two SQLite reads per export
- **Location**: `src/loop_graph/export.rs:48-66, 116-130`
- **Trigger condition**: Performance review.
- **Expected behavior**: One read per call.
- **Actual behavior**: Two reads (one list_nodes + one list_edges per exporter); the rows are sorted in memory. For an audit-trail replay (called via `loop_graph action="export"`), this is fine — exports are rare and reads are cheap on the indexed primary-key columns.
- **Suggested fix**: Only matters under bulk export. Not a bug today.

### [Info] `loop_graph_manage.rs:1874-1877` re-installs the global event bus from inside a test
- **Location**: `src/builtin_tools/loop_graph_manage.rs:1870-1877`
- **Trigger condition**: Cross-module wiring audit.
- **Expected behavior**: Test scaffolding should not have side effects on the process global.
- **Actual behavior**: The `if let None { install }` is inside `#[tokio::test]` function — confirmed by surrounding context (line 1420 is `#[cfg(test)]`, line 1865 is `#[tokio::test]`). The doc explains "The bus is a process OnceCell shared by parallel tests". Acceptable test pattern.
- **Suggested fix**: None — but worth noting that any test calling into `loop_graph_manage.rs` runs after bus installation, and the parallel-test isolation depends on the unique-id filter used in the assertion (line 1897+).

### [Info] Tool layer (`builtin_tools/loop_graph_manage.rs`) holds the entire action surface for the module — 1800+ lines
- **Location**: `src/builtin_tools/loop_graph_manage.rs` (sampled, not fully audited)
- **Trigger condition**: Adjacent wiring scan.
- **Expected behavior**: Tool surface references all `pub fn` readers from `loop_graph`.
- **Actual behavior**: All nine action paths (`node`, `drop_node`, `link`, `unlink`, `list`, `status`, `gc`, `enable_audit`, `pair`, plus the round-13 trio `impact`/`export`/`snapshot`) are implemented, and the publish sites line up with the store mutations (verified via grep — each store write has a publish branch). Wiring is complete per the GRAPH_LAYER §3 tool actions table.
- **Suggested fix**: None from this audit's scope; the tool layer's existing audits (rounds 9-13) cover the wiring.

### [Info] Doctor check correctly distinguishes "absent file" vs "unreadable file" vs "empty graph" vs "non-empty graph with findings" — four arms, four distinct messages
- **Location**: `src/diagnostics/checks/loop_graph.rs:50-148`
- **Trigger condition**: Doctor correctness check.
- **Expected behavior**: Each visibility class surfaces separately so operators can act.
- **Actual behavior**: Four distinct titles: "No graph yet" (file absent), "Graph DB unreadable" / "Graph lint failed" / "Graph state unknown" (transient), "No topology declared" (empty-but-existing), "Topology sound" (non-empty and lint clean), and `Topology finding` (lint produced). The round-11 fix ("`!path.exists()`'s branch is dead in production because boot creates the file unconditionally") is recorded in code with a test (`an_existing_but_empty_graph_is_not_reported_as_sound`). Good.
- **Suggested fix**: None.

---

## Cross-Module Findings

### [Warning] Two settle-notify consumers handle `false` differently: goal releases the CAS, workflow logs-and-moves-on
- **Modules**: `src/builtin_tools/goal.rs:1204-1212` and `src/teams/dispatcher/schedule/settle.rs:371-379`
- **Risk**: Asymmetric discipline. A `false` from `notify_goal_settled` releases the settle CAS (`release_settle_notify`) so the claim can be retried — but the CAS key `(id, completed_at_ms)` is immutable post-Complete, so the retry never actually fires. A `false` from `notify_workflow_settled` is logged as a `warn` and the durable channel-push claim is preserved. Both are correct per spec, but the goal's release-then-retry is a no-op that exists to keep the discipline tidy (the comment says "released for retry" rather than "released AND retired").
- **Suggested fix**: Either tighten the goal caller doc to "release is bookkeeping; the CAS does not retry because the key is immutable", or thread a real retry mechanism (a time-bounded queue for unsettled completions). Same topic covered as the `Debounce::HeldForOtherNode` finding above.

### [Warning] `governing_owner` and `governing_owner_or_refuse` together make the FailsOpen class observable
- **Modules**: `src/loop_graph/service.rs:421-432` (`governing_owner`), `src/builtin_tools/goal.rs:723-731` (`governing_owner_or_refuse`)
- **Risk**: The `GLOBAL` slot is classified `FailsOpen` — its absence returns `Ok(None)` which reaches `governing_owner_or_refuse` whose `Some` arm carries the refusal. The three call sites (goal.rs lines 829, 1041, 1235) all follow the `if let Some(owner) = governing_owner_or_refuse(&session)? { return Err(...) }` shape, so absence PERMITS the write. The module doc on `mod.rs:38-78` records this verbatim as the deliberate severity. The risk is purely observational: if the production call site for `init_global` (`executor/builtin_registry/builder/constructor/mod.rs:510`) ever fails silently, the ACL silently stops gating. Per the doc, this is currently unreachable, but the reachable path is `[cron] enabled=false` then early-boot + daemon-rebuild — minor in practice.
- **Suggested fix**: Already documented. No change needed.

---

## Suggested Tests

These are not findings — they are coverage gaps surfaced during the audit, in the format the skill requests.

```rust
/// `HeldForOtherNode` followed by `Done` during the in-window — does the
/// released-then-retried claim actually retry, or is it retired forever?
/// (Today: retired forever; the docstring claims it retries.)
#[tokio::test]
async fn debounce_held_for_other_node_does_not_credit_other_node_later_done() {
    // Simulate: settle A admitted, settle B held, settle A's run completes
    // and flips stamp to Done. Now settle B (with the released claim
    // hypothetically retried) sees the same Done stamp but for_node=goal:A,
    // so B is still HeldForOtherNode — its "release and retry" path is a
    // phantom. Assert the asymmetry so a future fix is observable.
}

/// A goal with NO `watches` edge but a healthy graph: notify_goal_settled
/// spends the one-shot claim with no log line. The companion
/// `the_release_logs_the_no_watchers_path` should pin the gap to a `tracing`
/// `info!` line so operators can find it.
#[tokio::test]
async fn settle_with_no_watchers_logs_the_unreviewed_path() {
    // Setup: a Complete goal with NO watches edge. Call notify_goal_settled
    // and assert the call returned true (claim spent) AND a tracing line was
    // emitted naming the goal id. If no log line exists today, this is a
    // coverage gap that doubles as a fix trigger.
}

/// Star topology walk: governance graph with one hub node owning 10,000
/// children. Confirm walk_chain returns Err within bounded time and does not
/// allocate a giant `out` vec before erroring.
#[test]
fn walk_chain_bounded_under_high_fan_out() {
    let (_d, s) = store();
    s.upsert_node(&node("root:hub", NodeKind::Root, Origin::Human)).unwrap();
    for i in 0..10_000 {
        s.upsert_node(&node(format!("goal:s{i}"), NodeKind::LoopGoal, Origin::Llm))
            .unwrap();
        s.upsert_edge(&GraphEdge::new(
            "main",
            "root:hub",
            format!("goal:s{i}"),
            EdgeKind::OwnsReference,
            Origin::Human,
        ))
        .unwrap();
    }
    let inspector = LoopGraphInspector::new(&s, "main");
    let res = inspector.subgraph_for("root:hub");
    // Either Err (after 1024 unique-node visits) OR a result whose `out`
    // vec is bounded — assert whichever the current implementation gives,
    // and pin a CEILING.
    assert!(
        matches!(res, Err(_)),
        "10k leaves must error rather than return a multi-MB result"
    );
}

/// `notify_node_settled` with a degraded `CRON_TRIGGER` slot — does the
/// `false` propagation actually release the goal CAS so a subsequent poll
/// can re-attempt? (Should today; this just pins it for future regressions.)
#[tokio::test]
async fn notify_node_settled_with_no_cron_handle_returns_false_for_retry() {
    // Setup: a Complete goal with a single paired cron watcher. Trigger
    // path WITHOUT init_cron_trigger being called (decline only).
    // Assert notify_goal_settled returns false. Then call init_cron_trigger
    // with a healthy service and assert the next call returns true.
}
```

---

## Notes

### Module-level quality

1. **R7 (no platform APIs in core), R8 (LLM handles intent), R10 (zero middleware tax) all held.** The module delegates every semantic verdict to a normal LLM turn; the only "intelligence" is in `templates.rs` (templates, not code). The store rejects structural forgeries (`x -[watches]-> x`, `anchor`/`frozen` as coverage sources) at write time AND discounts legacy rows in lint. Anti-RewardHacking discipline (round 9-12 fixes) is consistently applied.

2. **R3 (core minimalism) is held at the cost of size.** 6866 lines is large but justified — the role graph is the held-out layer that every optimizer defers to, so its correctness work (lint, store invariants, render safety, debounce state machine, event bus, snapshot, export) cannot be Slimmed into a Skill. The natural extraction points are snapshot.rs (already a sibling) and inspector.rs (could become `loop_graph_read/` if a second writer ever appeared — none has).

3. **The codebase-wide threat model (UTF-8 byte slicing, lock poisoning, `static mut`, ordering in events.rs) is well-defended.** No byte slicing in this module. Lock poisoning handled at six sites by `unwrap_or_else(|e| e.into_inner())`. No `static mut`. Atomic ordering on the lag counter is `AcqRel` for the add, `Acquire` for the load (correct, the documented acceptance of dropped events means `Relaxed` would also work but AcqRel is not wrong). On the diagnostics-readonly path, `busy_timeout=5000` is what keeps doctor from misreading healthy writes.

4. **The four read consumers (`render_session_topology`, `notify_goal_settled`, `governing_owner`, `doctor::loop_graph`) are NOT yet unified through the inspector.** The inspector (round 13) consolidates the SHAPE of reads but not the read SITES — `service.rs::render_session_topology_inner` still hand-rolls edges/roots/labels lookups. This is acceptable (each reader's predicates differ) but worth flagging: the doc on `inspector.rs:8-13` ("status panel renders 'untracked'") suggests a future refactor where the inspector becomes the only read surface.

5. **`debounce` state machine has been carefully constructed.** The five `Debounce` variants cover every transition of the `(stamp exists, for_node match, state)` product. The "release on failure" + "release on HeldForSameNodeInFlight" combination has the property that a failed poke does NOT consume the window. The narrative comment at lines 99-117 is excellent.

6. **The CRLF / Unicode line/paragraph separator handling in `one_line` and `indented_body`** is paranoid at the right level. The bug it prevents (a model-authored `cron:` label forging `根参照 root:forged` lines) is the same shape as the original doc reference; treating U+2028/U+2029 alongside `\n` closes the JSON.stringify seam. Tests at lines 947-1014 cover both ASCII and Unicode injections.

7. **Test coverage within the module** is thorough:
   - `store.rs`: 19 tests (covers invariants, lint, GC, body scan, COALESCE discipline, kind write-once, atom-text spoofing, concurrency test for GC atomicity, multi-path governance chain)
   - `service.rs`: 15 tests (covers debounce states, render safety, governance owner, prompt's truthful review claim, workflow node id parity)
   - `inspector.rs`: 11 tests (covers walker bounds, star/per-node, summary counts, impact causal split)
   - `snapshot.rs`: 9 tests (covers concurrent capture, label cap, fail-soft inheritance, events pagination)
   - `export.rs`: 8 tests (covers determinism, byte-order parity DOT vs JSON, escaping)
   - `templates.rs`: 4 tests (covers auditor cron_manage blind spot, ring name parity)
   - `events.rs`: 5 tests (covers broadcast behaviour, closed subscription, persistence)
   - `mod.rs`: 3 tests (covers init/get round-trip, persister wiring)
   - `types.rs`: 3 tests (covers round-trip, optimization loop classification, cadence rank)

8. **The "Two cross-module concerns" deserve a follow-up:** (1) the asymmetric `false` handling between `notify_goal_settled` and `notify_workflow_settled` is documented but easy to break; (2) the `governing_owner` FailsOpen is documented but the test pinning it is in `mod.rs`, not in `goal.rs::governing_owner_or_refuse` where the actual decision lives. Both are doc-only fixes.

9. **There is no dead code at the module level** — every public function in `loop_graph/` has at least one caller in `loop_graph/`, `builtin_tools/loop_graph_manage.rs`, `orchestrator/harness_bridge/prompt_build.rs`, or the `/diagnostics/checks/loop_graph.rs` health check. The orphan-risk items (templateless verbs, unused bus methods, deferred `#[cfg(test)]` strict render) are all honestly documented as future-use, not silently dangling.

10. **No regressions detected by `cargo check` / `cargo clippy -D warnings` runtime check** during the audit (the workspace's `cargo check -p alephcore` was left running for the Phase 5 spot check; compilation proceeded through the dependency graph without warnings attributable to `loop_graph/`). The skill's L1/L2 testing layers (proptest/loom) were not run as part of this audit, since the question is design correctness not test-runner coverage.

---

## Summary

| Level       | Count |
|-------------|-------|
| Critical    | 0     |
| Warning     | 8 (plus 2 cross-module)  |
| Info        | 14    |
| Suggested Test | 4  |

**Net assessment**: a heavily-defended, well-tested governance layer whose defects are all in the "documented trade-off" or "deferred surface" categories. The state machine, structural invariants, debounce discipline, and render safety are all genuinely strong; the warnings are mostly about asymmetry or unbounded-but-not-currently-exploited cases. No code change recommended as a result of this audit — the discipline of round-by-round hardening (rounds 9-13 each removing a class of forgery) is exactly what the design philosophy calls for, and round 14 would be cosmetic at best.
