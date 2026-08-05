# Goal (Standing Goal) Round 10 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a standing goal's wall-clock bound hold at the instant a continuation *executes* (not merely when it is claimed), stop treating transient provider failures as a permanent verdict, and close the retirement / render / validation gaps around the wait barrier.

**Architecture:** Everything is wiring existing machinery. The bound predicate mirrors `looping::pursuit::fires_out_of_bounds` verbatim in shape; the transient-failure park reuses the existing wait barrier plus the existing `ExecutionError::receipt_kind()` classifier and `llm_retry::extract_retry_after_str`; the retirement fix widens one store predicate. Zero new subsystems, zero new persisted fields, zero new enum values on `GoalStatus`.

**Tech Stack:** Rust (workspace crate `alephcore`), rusqlite (`GoalStore` is a `Mutex<Connection>` keyed by session string), tokio, `#[cfg(test)] mod tests` unit tests with `tempfile`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-05-goal-standing-goal-round10-design.md`. Read it before Task 1.
- Branch: work only in the current worktree (`worktree-goal-round10`). Never touch `main`.
- R10: nothing may be added to `src/harness/` (12-file redline). All of this lives in `src/goal/`, `src/gateway/`, `src/builtin_tools/`, `src/orchestrator/`.
- R7: no judgment in code. Every new predicate is a pure clock/enum comparison.
- Commit messages: `<scope>: <description>`, English, conventional style (e.g. `goal: bound a timer wake at fire time`). No attribution trailer.
- Minimum verification set after any task that changes a signature: `cargo test -p alephcore --lib --no-run` (`cargo check` does NOT compile `#[cfg(test)]` code — a deleted/renamed API leaves tests rotting invisibly).
- Full verification before the final commit: `cargo test -p alephcore --lib`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` (run `rustfmt --check <file>` per file; never bare `cargo fmt` in this repo).
- Comments in English; user-facing tool/prompt strings in English (matching the surrounding `goal` module).
- Never use bare `git stash` / `git stash pop` (the stash stack is shared across worktrees).

## File Structure

| File | Responsibility after this plan |
|---|---|
| `src/goal/pursuit.rs` | Pure decisions. Gains `fires_out_of_bounds` (the fire-time bound) and `transient_park_note`. `objective_block` delegates escaping to `xml_util`. |
| `src/goal/store.rs` | Atomic claim/commit. Timer arm gains the projected bound; `confirm_fire` returns `FireDecision`; gains `park_if_active` and `block_if_not_terminal`. |
| `src/goal/mod.rs` | Re-exports (`FireDecision` added). |
| `src/gateway/execution_engine/goal_continuation.rs` | Driver. `block_goal_on_failure` becomes classification-driven. |
| `src/gateway/execution_engine/goal_wait.rs` | Wake service. Boot re-arm evaluates bounds; wake claims carry the tree-token total. |
| `src/gateway/execution_engine/execute.rs` | Routes `FireDecision::OutOfBounds` to the same origin notice the `Exhausted` arm uses. |
| `src/gateway/continuation_lifecycle.rs` | Retirement seam retires Paused goals too. |
| `src/builtin_tools/goal.rs` | Tool surface. Rejects `update(objective=…)`; validates wait args against the post-update goal. |
| `src/orchestrator/harness_bridge/context_blocks.rs` | Per-turn prompt fragments. Parked *fact* into the cacheable summary, parked *countdown* into the transient tail. |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` | Boot wiring: injects `session_store` into `GoalWakeService`. |
| `docs/reference/FEATURE_LOCATOR.md`, `CLAUDE.md` | Documentation of the round + the two cross-cutting lessons. |

---

### Task 1: The fire-time bound predicate + claim-side enforcement (B1a, B7)

**Files:**
- Modify: `src/goal/pursuit.rs` (add `fires_out_of_bounds` after `should_continue`, ~line 61)
- Modify: `src/goal/store.rs:334-376` (the `wait_parked` Timer arm inside `try_claim_continuation`)
- Test: `src/goal/pursuit.rs` `#[cfg(test)] mod tests`, `src/goal/store.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub fn fires_out_of_bounds(goal: &Goal, wake_ms: u64, now_ms: u64) -> bool` in `crate::goal::pursuit`. Task 2 and Task 3 both call it.

- [ ] **Step 1: Write the failing tests in `src/goal/pursuit.rs`**

Add inside the existing `mod tests` (it already has `use super::*;` and an `active_goal(max_iter)` helper):

```rust
#[test]
fn a_wake_past_the_deadline_is_out_of_bounds() {
    let g = active_goal(5).with_deadline_ms(Some(10_000));
    // Claimed at 1_000 (inside the deadline) but waking at 20_000 (outside).
    assert!(fires_out_of_bounds(&g, 20_000, 1_000));
    // A wake that lands before the deadline is fine.
    assert!(!fires_out_of_bounds(&g, 9_000, 1_000));
    // Exactly at the deadline is still in bounds (`>` not `>=`, matching
    // `should_continue`'s `now_ms > deadline`).
    assert!(!fires_out_of_bounds(&g, 10_000, 1_000));
}

#[test]
fn no_deadline_or_no_clock_never_reads_out_of_bounds() {
    let no_deadline = active_goal(5);
    assert!(!fires_out_of_bounds(&no_deadline, u64::MAX, 1_000));
    // now_ms == 0 means "clock unavailable" — fail open, same convention as
    // `should_continue`, so clock-less callers stay behavior-identical.
    let bounded = active_goal(5).with_deadline_ms(Some(10_000));
    assert!(!fires_out_of_bounds(&bounded, 20_000, 0));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p alephcore --lib goal::pursuit::tests::a_wake_past 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'fires_out_of_bounds' in this scope`

- [ ] **Step 3: Implement `fires_out_of_bounds` in `src/goal/pursuit.rs`**

Insert immediately after `should_continue` (which ends at line 61):

```rust
/// Would a continuation claimed now — waking at `wake_ms` — still be inside
/// the goal's wall-clock bound when it actually EXECUTES?
///
/// [`should_continue`] answers "may this goal be claimed", which is a
/// different question. A deadline wait barrier is claimed at one `post_run`
/// and its wake executes up to hours later, and nothing in between re-reads
/// the clock: `confirm_fire` matches the pending marker, not the deadline.
/// Without this projection `timeout_minutes` bounds only when a continuation
/// may be *scheduled*, so `wait_minutes=180` under a 30-minute deadline runs a
/// full LLM turn 150 minutes past the limit the user set.
///
/// The tradeoff is deliberate and identical to the loop's
/// (`looping::pursuit::fires_out_of_bounds`): the pursuit now stops at the
/// last claim whose wake lands in-bounds — up to one park EARLY rather than
/// arbitrarily late. A safety cap is an upper bound, not an approximation.
///
/// `now_ms == 0` (clock unavailable) fails open, matching [`should_continue`].
#[must_use]
pub fn fires_out_of_bounds(goal: &Goal, wake_ms: u64, now_ms: u64) -> bool {
    goal.deadline_ms
        .is_some_and(|deadline| now_ms != 0 && wake_ms > deadline)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib goal::pursuit::tests:: 2>&1 | tail -20`
Expected: PASS (all pursuit tests)

- [ ] **Step 5: Write the failing store test for the claim-side bound**

Add inside `src/goal/store.rs` `mod tests`:

```rust
#[test]
fn a_timer_park_whose_wake_lands_past_the_deadline_is_arbitrated_not_armed() {
    use crate::goal::PursuitMode;
    let (store, _d) = temp_store();
    // Deadline at 30_000; the model parks until 200_000. Claiming at 1_000
    // the goal is still inside its deadline, so `should_continue` alone said
    // "arm the timer" — and the wake would have executed 170s past the bound.
    let g = Goal::new("sess-oob", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_deadline_ms(Some(30_000))
        .with_wait_until(200_000, Some("long cooldown".into()), 1_000);
    store.put(&g).unwrap();

    let ContinuationDecision::Exhausted { note } = store
        .try_claim_continuation("sess-oob", None, 1_000, false, None)
        .unwrap()
    else {
        panic!("a wake landing past the deadline must be arbitrated, not armed");
    };
    assert!(note.contains("wall-clock"), "got: {note}");

    let live = store.get("sess-oob").unwrap().unwrap();
    assert_eq!(live.status, GoalStatus::Blocked);
    assert!(!live.has_wait_barrier(), "the barrier is dropped with the block");
    assert_eq!(live.pending_continuation_ms, None, "no timer left armed");
    assert_eq!(
        live.continuations_used, 0,
        "arbitration spends no iteration — nothing ran"
    );
}

#[test]
fn a_timer_park_inside_the_deadline_still_arms_normally() {
    use crate::goal::PursuitMode;
    let (store, _d) = temp_store();
    let g = Goal::new("sess-ib", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_deadline_ms(Some(300_000))
        .with_wait_until(61_000, None, 1_000);
    store.put(&g).unwrap();
    let ContinuationDecision::Fire { wake_ms, .. } = store
        .try_claim_continuation("sess-ib", None, 1_000, false, None)
        .unwrap()
    else {
        panic!("an in-bounds wake must still arm");
    };
    assert_eq!(wake_ms, 61_000);
}
```

- [ ] **Step 6: Run to verify the first fails**

Run: `cargo test -p alephcore --lib goal::store::tests::a_timer_park 2>&1 | tail -25`
Expected: `a_timer_park_whose_wake_lands_past_the_deadline_is_arbitrated_not_armed` FAILS with "a wake landing past the deadline must be arbitrated, not armed"; `a_timer_park_inside_the_deadline_still_arms_normally` PASSES.

- [ ] **Step 7: Enforce the bound in the Timer arm and delete the unreachable branch**

In `src/goal/store.rs`, replace the whole `if let Some(park) = pursuit::wait_parked(&goal, now_ms) { … }` block (currently lines 334-376) with:

```rust
        if let Some(park) = pursuit::wait_parked(&goal, now_ms) {
            if let pursuit::WaitPark::Timer { wake_ms } = park {
                // The bound must hold at the instant the wake EXECUTES, not
                // merely when it is claimed: this claim arms a timer that
                // fires up to hours from now, and `confirm_fire` matches the
                // marker, not the clock. A wake landing past the deadline is
                // arbitrated here instead of running out of bounds.
                if pursuit::should_continue(&goal, tokens_now, now_ms)
                    && !pursuit::fires_out_of_bounds(&goal, wake_ms, now_ms)
                {
                    let prompt = pursuit::wait_resume_prompt(&goal, "the wait elapsed");
                    Self::put_locked(
                        &conn,
                        &goal
                            .spent_continuation(now_ms)
                            .with_pending_continuation(Some(wake_ms)),
                    )?;
                    return Ok(ContinuationDecision::Fire {
                        delay_ms: wake_ms.saturating_sub(now_ms),
                        wake_ms,
                        prompt,
                    });
                }
                // No runway left — either already spent (iteration / deadline
                // / token cap) or the wake itself lands out of bounds. Arming
                // the timer would only wake into an immediate Block (or, for
                // the out-of-bounds case, run past the user's limit), so
                // arbitrate now. `wait_parked` already established Active
                // status + Active pursuit, so `exhausted_while_active` holds
                // whenever `should_continue` does not; the out-of-bounds case
                // is reported with the deadline note for the same reason.
                let note = if pursuit::fires_out_of_bounds(&goal, wake_ms, now_ms) {
                    pursuit::deadline_reached_note(&goal)
                } else {
                    pursuit::stop_reason_note(&goal, tokens_now, now_ms)
                };
                Self::put_locked(
                    &conn,
                    &goal
                        .without_wait(now_ms)
                        .with_status(GoalStatus::Blocked, now_ms)
                        .with_note(Some(note.clone()), now_ms)
                        .with_pending_continuation(None),
                )?;
                return Ok(ContinuationDecision::Exhausted { note });
            }
            // Task barrier: woken externally by `GoalWakeService` on the
            // task's settle event. Exhaustion is arbitrated when the barrier
            // clears, so a goal legitimately waiting is never Blocked here.
            if dirty {
                Self::put_locked(&conn, &goal)?;
            }
            return Ok(ContinuationDecision::Idle);
        }
```

Note what changed besides the bound: the old code had a fall-through arm for a Timer that was "neither runnable nor exhausted", which `wait_parked`'s own preconditions (Active status **and** Active pursuit) make unreachable — `exhausted_while_active` is definitionally true whenever `should_continue` is false there. The Timer arm now always returns.

- [ ] **Step 8: Add the unreachability regression test**

Add to `src/goal/store.rs` `mod tests`:

```rust
#[test]
fn a_parked_timer_is_always_either_runnable_or_exhausted() {
    use crate::goal::pursuit;
    use crate::goal::PursuitMode;
    // Locks the invariant the Timer arm relies on: `wait_parked` already
    // requires Active status + Active pursuit, which are exactly the two
    // extra conditions `exhausted_while_active` adds on top of
    // `!should_continue`. If a future edit relaxes `wait_parked`, this fails
    // instead of silently resurrecting a dead fall-through branch.
    let g = Goal::new("s", "obj", 0, 0)
        .with_pursuit(PursuitMode::Active { max_iterations: 1 })
        .with_wait_until(90_000, None, 0)
        .spent_continuation(0); // iteration cap now spent
    let Some(pursuit::WaitPark::Timer { .. }) = pursuit::wait_parked(&g, 1_000) else {
        panic!("expected a timer park");
    };
    assert!(!pursuit::should_continue(&g, 0, 1_000));
    assert!(
        pursuit::exhausted_while_active(&g, 0, 1_000),
        "a parked timer with no runway must read as exhausted"
    );
}
```

- [ ] **Step 9: Run the store tests**

Run: `cargo test -p alephcore --lib goal::store::tests:: 2>&1 | tail -25`
Expected: PASS, all of them (the pre-existing `timer_barrier_*` tests must still pass — they use goals with no deadline, so the new bound is a no-op for them).

- [ ] **Step 10: Commit**

```bash
rustfmt --check src/goal/pursuit.rs src/goal/store.rs
git add src/goal/pursuit.rs src/goal/store.rs
git commit -m "goal: evaluate the wall-clock bound at the wake's execution instant, not its claim"
```

---

### Task 2: `confirm_fire` becomes a three-way decision (B1b)

**Files:**
- Modify: `src/goal/store.rs:745-754` (`confirm_fire`), plus the `ContinuationDecision` enum region (~line 42) for the new enum
- Modify: `src/goal/mod.rs:9` (re-export)
- Modify: `src/gateway/execution_engine/execute.rs:1125-1136` (the Goal fire arm)
- Test: `src/goal/store.rs` `mod tests`

**Interfaces:**
- Consumes: `pursuit::fires_out_of_bounds` (Task 1).
- Produces: `pub enum FireDecision { Proceed, Superseded, OutOfBounds { note: String } }` in `crate::goal::store`, re-exported as `crate::goal::FireDecision`; `GoalStore::confirm_fire(&self, session_id: &str, wake_ms: u64, now_ms: u64) -> Result<FireDecision>`. Task 3 calls it indirectly through `spawn_continuation_run`.

- [ ] **Step 1: Write the failing test**

Add to `src/goal/store.rs` `mod tests`:

```rust
#[test]
fn confirm_fire_refuses_and_arbitrates_a_wake_that_is_now_out_of_bounds() {
    use crate::goal::PursuitMode;
    let (store, _d) = temp_store();
    // A claimed timer whose deadline passed while it slept — the shape a
    // daemon restart produces, where the boot re-arm replays the stored
    // marker directly. `is_active()` + marker match alone would fire it.
    let g = Goal::new("sess-late", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_deadline_ms(Some(50_000))
        .with_wait_until(200_000, None, 1_000)
        .with_pending_continuation(Some(200_000));
    store.put(&g).unwrap();

    let FireDecision::OutOfBounds { note } =
        store.confirm_fire("sess-late", 200_000, 200_000).unwrap()
    else {
        panic!("a wake past the deadline must not proceed");
    };
    assert!(note.contains("wall-clock"), "got: {note}");
    let live = store.get("sess-late").unwrap().unwrap();
    assert_eq!(live.status, GoalStatus::Blocked);
    assert_eq!(live.pending_continuation_ms, None);
    assert!(!live.has_wait_barrier());
}

#[test]
fn confirm_fire_still_proceeds_and_still_reports_supersession() {
    use crate::goal::PursuitMode;
    let (store, _d) = temp_store();
    let g = Goal::new("sess-ok", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_pending_continuation(Some(7_000));
    store.put(&g).unwrap();
    assert!(matches!(
        store.confirm_fire("sess-ok", 7_000, 7_000).unwrap(),
        FireDecision::Proceed
    ));
    // The marker was consumed, so a second confirm is superseded.
    assert!(matches!(
        store.confirm_fire("sess-ok", 7_000, 7_100).unwrap(),
        FireDecision::Superseded
    ));
    // A goal with no marker at all is likewise superseded, never OutOfBounds.
    assert!(matches!(
        store.confirm_fire("sess-missing", 7_000, 7_100).unwrap(),
        FireDecision::Superseded
    ));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib goal::store::tests::confirm_fire 2>&1 | tail -20`
Expected: FAIL — `cannot find type 'FireDecision' in this scope`

- [ ] **Step 3: Add the enum next to `ContinuationDecision` in `src/goal/store.rs`**

Insert after the `RearmDecision` enum (which ends at line 75):

```rust
/// Fire-time verdict for a claimed continuation.
///
/// `confirm_fire` used to return a bare `bool`, whose `false` meant "this
/// continuation was superseded — skip silently". That is the right handling
/// for a supersession and exactly the WRONG handling for a wake that is now
/// out of bounds: skipping silently would trade "runs past the user's limit"
/// for "stalls forever with the goal still Active and nothing left to claim
/// it". The out-of-bounds case therefore carries the note its caller must
/// push to the origin channel, exactly as the `Exhausted` claim decision does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireDecision {
    /// Still the continuation on the books, still in bounds — execute it.
    Proceed,
    /// Superseded (goal cleared / completed / re-claimed) — skip silently.
    Superseded,
    /// The wall-clock bound elapsed while this continuation waited. The goal
    /// has been Blocked with `note` in the same guard; the caller notifies.
    OutOfBounds { note: String },
}
```

- [ ] **Step 4: Rewrite `confirm_fire`**

Replace `src/goal/store.rs:738-754` (doc comment and body) with:

```rust
    /// Fire-time gate for a claimed continuation: proceed only if the goal is
    /// still `Active`, THIS continuation is still the one on the books
    /// (`wake_ms` matches the pending marker), AND the wall-clock bound has
    /// not elapsed while it waited. The marker is cleared in the same guard.
    ///
    /// [`FireDecision::Superseded`] (the user cleared/completed the goal, or a
    /// stale-grace re-claim replaced this one) keeps the run from burning a
    /// stale LLM turn. [`FireDecision::OutOfBounds`] additionally Blocks the
    /// goal with the deadline note, because a bound that merely refuses to
    /// fire — with no arbitration — leaves an `Active` goal that nothing will
    /// ever claim again. This is the only bound the boot re-arm path
    /// (`GoalWakeService::rearm_parked_goals`, which replays a stored marker
    /// without a fresh claim) passes through.
    pub fn confirm_fire(
        &self,
        session_id: &str,
        wake_ms: u64,
        now_ms: u64,
    ) -> Result<FireDecision> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(g) if g.is_active() && g.pending_continuation_ms == Some(wake_ms) => {
                if pursuit::fires_out_of_bounds(&g, wake_ms, now_ms) {
                    let note = pursuit::deadline_reached_note(&g);
                    Self::put_locked(
                        &conn,
                        &g.without_wait(now_ms)
                            .with_status(GoalStatus::Blocked, now_ms)
                            .with_note(Some(note.clone()), now_ms)
                            .with_pending_continuation(None),
                    )?;
                    return Ok(FireDecision::OutOfBounds { note });
                }
                Self::put_locked(&conn, &g.with_pending_continuation(None))?;
                Ok(FireDecision::Proceed)
            }
            _ => Ok(FireDecision::Superseded),
        }
    }
```

- [ ] **Step 5: Re-export in `src/goal/mod.rs`**

Change line 9 to:

```rust
pub use store::{ContinuationDecision, FieldUpdate, FireDecision, GoalStore, RearmDecision};
```

- [ ] **Step 6: Update the five existing `confirm_fire` call sites inside `src/goal/store.rs` tests**

They are at lines ~946, ~1221, ~1251, ~1267, ~1282, ~1563 (find them with `grep -n 'confirm_fire' src/goal/store.rs`). Each currently reads `store.confirm_fire(X, Y).unwrap()` returning `bool`. Rewrite each as follows — pass a `now_ms` equal to the wake (these goals carry no deadline, so the bound is inert and the assertion meaning is unchanged):

- `assert!(store.confirm_fire(s, w).unwrap());` → `assert_eq!(store.confirm_fire(s, w, w).unwrap(), FireDecision::Proceed);`
- `assert!(!store.confirm_fire(s, w).unwrap());` → `assert_eq!(store.confirm_fire(s, w, w).unwrap(), FireDecision::Superseded);`
- `assert!(!store.confirm_fire(s, w).unwrap(), "msg");` → `assert_eq!(store.confirm_fire(s, w, w).unwrap(), FireDecision::Superseded, "msg");`

The site at ~1563 is `.confirm_fire("s", live.pending_continuation_ms.unwrap())` — add `, live.pending_continuation_ms.unwrap()` as the third argument, then adapt the surrounding assertion the same way.

- [ ] **Step 7: Route the new decision in `src/gateway/execution_engine/execute.rs`**

Replace the `ContinuationKind::Goal { wake_ms }` arm at lines 1125-1136 with:

```rust
            ContinuationKind::Goal { wake_ms } => {
                let decision = crate::goal::global().map(|store| {
                    store.confirm_fire(
                        &session_key_str,
                        wake_ms,
                        super::goal_continuation::now_ms(),
                    )
                });
                match decision {
                    Some(Ok(crate::goal::FireDecision::Proceed)) => {}
                    Some(Ok(crate::goal::FireDecision::OutOfBounds { note })) => {
                        // The bound elapsed while this continuation waited.
                        // The store already Blocked the goal; push the same
                        // notice the `Exhausted` claim decision would (R5 —
                        // an autonomous ending must never be silent), and
                        // drop the welded plan like every other terminal end.
                        super::goal_continuation::clear_goal_welded_strategy(&session_key_str);
                        info!(session = %session_key_str, note = %note,
                            "goal pursuit: wake landed past the wall-clock bound; goal blocked");
                        notify_origin(origin.as_ref(), format!("⏹ {note}")).await;
                        return;
                    }
                    _ => {
                        info!(session = %session_key_str,
                            "goal pursuit: continuation superseded (goal cleared, completed or re-claimed); skipping");
                        return;
                    }
                }
            }
```

Note: `origin` must be resolved before this point. Check the surrounding function — if `origin` is currently resolved *after* the fire gate (the `let Some(cont_agent) = registry.get(...)` block sits between them), move the `origin` resolution above the `match kind` block so the `OutOfBounds` arm can use it. If moving it is not possible without restructuring, resolve it inline in this arm with `super::goal_continuation::origin_of(&agent, &session_key).await` once `agent` is available, and place the whole `OutOfBounds` handling after the agent lookup instead. **Read the function before editing and pick whichever keeps the diff smallest; do not duplicate the origin resolution.**

- [ ] **Step 8: Verify it compiles and the tests pass**

Run: `cargo test -p alephcore --lib goal:: 2>&1 | tail -25`
Expected: PASS

Run: `cargo test -p alephcore --lib --no-run 2>&1 | tail -10`
Expected: builds clean (this is the check `cargo check` cannot do)

- [ ] **Step 9: Commit**

```bash
rustfmt --check src/goal/store.rs src/goal/mod.rs src/gateway/execution_engine/execute.rs
git add src/goal/store.rs src/goal/mod.rs src/gateway/execution_engine/execute.rs
git commit -m "goal: confirm_fire returns a three-way decision so an out-of-bounds wake is arbitrated, not skipped"
```

---

### Task 3: Boot re-arm stops bypassing the bounds (B1c)

**Files:**
- Modify: `src/gateway/execution_engine/goal_wait.rs:150-188` (`rearm_parked_goals`)
- Test: manual reasoning only — this path needs a live registry/adapter; the bound itself is covered by Task 1 and Task 2 unit tests. Add the doc comment that names why.

**Interfaces:**
- Consumes: `pursuit::fires_out_of_bounds` (Task 1), `crate::goal::FireDecision` (Task 2, indirectly via `spawn_continuation_run`).

- [ ] **Step 1: Replace the claimed-timer branch in `rearm_parked_goals`**

In `src/gateway/execution_engine/goal_wait.rs`, replace the `} else if let Some(wake_ms) = goal.pending_continuation_ms {` branch body (lines ~160-179) with:

```rust
            } else if let Some(wake_ms) = goal.pending_continuation_ms {
                // A CLAIMED deadline timer died with the previous process.
                // Re-spawn its wake run directly using the STORED marker as
                // the confirm_fire key — never route it back through a fresh
                // claim, because the pending gate (`now < wake + 60s grace`)
                // would reject a claim for a marker whose wake already passed
                // or is imminent, silently stranding the pursuit until the
                // next run in the session. Restricted to barrier-carrying
                // goals — a generic crashed pending marker keeps the
                // pre-existing stale-grace recovery (its prompt is
                // unrecoverable). delay 0 when the wake already elapsed.
                //
                // Bypassing the claim also bypasses every bound the claim
                // evaluates, and a restart is exactly when a parked wake is
                // most likely to have outlived its deadline. Arbitrate here
                // rather than waking a goal whose wall-clock budget expired
                // while the daemon was down; `confirm_fire` is the second,
                // narrower net for the same condition.
                if goal.waiting_until_ms.is_some() {
                    if crate::goal::pursuit::fires_out_of_bounds(&goal, wake_ms, now) {
                        self.arbitrate_out_of_bounds(&goal, now).await;
                        continue;
                    }
                    self.spawn_wake_run(
                        &goal,
                        crate::goal::pursuit::wait_resume_prompt(&goal, "the wait elapsed"),
                        wake_ms.saturating_sub(now),
                        wake_ms,
                    )
                    .await;
                }
            } else if goal.has_wait_barrier() {
```

- [ ] **Step 2: Add the `arbitrate_out_of_bounds` helper**

Add as a method on `GoalWakeService`, immediately after `spawn_wake_run` (before the closing `}` of the `impl` block):

```rust
    /// A parked wake outlived its wall-clock deadline while the daemon was
    /// down. Block the goal with the deadline note and push the same origin
    /// notice every other structural stop pushes (R5). Uses `confirm_fire`
    /// rather than a bespoke write so the Blocked transition, the barrier
    /// drop and the marker clear all come from the one guard that already
    /// owns them — and so a concurrent claim cannot race this into firing.
    async fn arbitrate_out_of_bounds(&self, goal: &Goal, now: u64) {
        let Some(store) = crate::goal::global() else {
            return;
        };
        let Some(wake_ms) = goal.pending_continuation_ms else {
            return;
        };
        let note = match store.confirm_fire(&goal.session_id, wake_ms, now) {
            Ok(crate::goal::FireDecision::OutOfBounds { note }) => note,
            // Proceed / Superseded: the row moved under us between the sweep's
            // read and this write — leave it to the normal pipeline.
            Ok(_) => return,
            Err(e) => {
                warn!(session = %goal.session_id, error = %e,
                    "goal wake: failed to arbitrate an out-of-bounds parked wake");
                return;
            }
        };
        super::goal_continuation::clear_goal_welded_strategy(&goal.session_id);
        if let Some(key) = SessionKey::parse(&goal.session_id) {
            if let Some(agent) = self.deps.registry.get(key.agent_id()).await {
                let origin = origin_of(&agent, &key).await;
                notify_origin(origin.as_ref(), format!("⏹ {note}")).await;
            }
        }
        info!(session = %goal.session_id, note = %note,
            "goal wake: parked wake outlived its deadline while down; goal blocked");
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo test -p alephcore --lib --no-run 2>&1 | tail -10`
Expected: builds clean

- [ ] **Step 4: Update the module doc**

In the header comment of `src/gateway/execution_engine/goal_wait.rs`, extend bullet 2 (`**Restarts**…`) with one sentence:

```
//!    `CoordTaskStore` (fail-open: a vanished task reads as settled, so a
//!    stale barrier can never wedge a pursuit forever — hermes' rule). A
//!    replayed timer whose deadline expired while the daemon was down is
//!    arbitrated (Blocked + origin notice) instead of woken: bypassing the
//!    claim must not also bypass the claim's bounds.
```

- [ ] **Step 5: Commit**

```bash
rustfmt --check src/gateway/execution_engine/goal_wait.rs
git add src/gateway/execution_engine/goal_wait.rs
git commit -m "goal: boot re-arm arbitrates a parked wake that outlived its deadline instead of firing it"
```

---

### Task 4: Wake-path claims carry the tree-token total (B1d)

**Files:**
- Modify: `src/gateway/execution_engine/goal_wait.rs` (struct, `new`, `claim_and_spawn`)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1376-1383`

**Interfaces:**
- Consumes: `crate::gateway::goal_budget::tree_tokens(store: &Arc<dyn SessionStore>, goal: &Goal, own_key: &SessionKey) -> Option<u64>` (existing).
- Produces: `GoalWakeService::new(deps: ContinuationDeps, coord_store: Option<Arc<dyn CoordTaskStore>>, session_store: Option<Arc<dyn SessionStore>>) -> Self`.

- [ ] **Step 1: Add the field and constructor parameter**

In `src/gateway/execution_engine/goal_wait.rs`, extend the struct:

```rust
pub struct GoalWakeService {
    deps: ContinuationDeps,
    /// For the boot recheck of task barriers. `None` → recheck skipped
    /// (fail-open wake instead: with no store the task can never settle).
    coord_store: Option<Arc<dyn CoordTaskStore>>,
    /// For the token budget on wake claims. A wake has no completing run to
    /// read a live total from, so this used to pass `None` and every wake was
    /// budget-unenforced — and the wake service is the ONLY driver a
    /// task-barrier pursuit ever has, so its whole budget could be spent
    /// through this hole. `None` (not injected / read failure) keeps the old
    /// fail-open behavior.
    session_store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
}

impl GoalWakeService {
    #[must_use]
    pub fn new(
        deps: ContinuationDeps,
        coord_store: Option<Arc<dyn CoordTaskStore>>,
        session_store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    ) -> Self {
        Self {
            deps,
            coord_store,
            session_store,
        }
    }
```

- [ ] **Step 2: Use it in `claim_and_spawn`**

Replace the `// No live token count here …` comment block and the `try_claim_continuation` call inside `claim_and_spawn` with:

```rust
        // The tree total in `over_budget`'s coordinate system (own session +
        // each enrolled delegation member's delta). Without it every wake was
        // a free step past the token budget — and for a task-barrier pursuit
        // this service is the only driver there is. `None` (no store, an
        // unparseable key, or a read failure) keeps the budget unenforced for
        // this claim, exactly as before; the next post_run re-enforces it.
        let tokens = match (&self.session_store, SessionKey::parse(&session)) {
            (Some(store), Some(key)) => {
                crate::gateway::goal_budget::tree_tokens(store, goal, &key).await
            }
            _ => None,
        };
        let decision =
            match store.try_claim_continuation(&session, tokens, now_ms(), gate_configured, None) {
```

- [ ] **Step 3: Pass the store at boot**

In `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`, the `GoalWakeService::new(...)` call (~line 1378) currently takes two arguments. Add the third:

```rust
            let goal_wake = std::sync::Arc::new(
                alephcore::gateway::execution_engine::goal_wait::GoalWakeService::new(
                    goal_deps,
                    coord_store.clone(),
                    Some(session_store.clone()),
                ),
            );
```

`session_store: Arc<dyn SessionStore>` is already a parameter of this function (declared at line 126) and is still owned at this point (it is moved at line ~1796, after this block).

- [ ] **Step 4: Verify it compiles**

Run: `cargo test -p alephcore --lib --no-run 2>&1 | tail -10` then `cargo check -p aleph-server 2>&1 | tail -10`
Expected: both clean

- [ ] **Step 5: Commit**

```bash
rustfmt --check src/gateway/execution_engine/goal_wait.rs
git add src/gateway/execution_engine/goal_wait.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "goal: wake claims read the tree token total instead of running the budget unenforced"
```

---

### Task 5: `park_if_active` — the third CAS sibling (B2, store half)

**Files:**
- Modify: `src/goal/store.rs` (add after `pause_if_active`, ~line 605)
- Test: `src/goal/store.rs` `mod tests`

**Interfaces:**
- Produces: `GoalStore::park_if_active(&self, session_id: &str, until_ms: u64, reason: &str, now_ms: u64) -> Result<bool>`. Task 6 calls it.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn park_if_active_arms_a_timer_barrier_without_leaving_active() {
    use crate::goal::PursuitMode;
    let (store, _d) = temp_store();
    let g = Goal::new("sess-park", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_pending_continuation(Some(1_000));
    store.put(&g).unwrap();

    assert!(store
        .park_if_active("sess-park", 300_000, "RATE_LIMITED: retry later", 2_000)
        .unwrap());
    let live = store.get("sess-park").unwrap().unwrap();
    assert_eq!(live.status, GoalStatus::Active, "a park is not a verdict");
    assert_eq!(live.waiting_until_ms, Some(300_000));
    assert_eq!(
        live.waiting_reason.as_deref(),
        Some("RATE_LIMITED: retry later")
    );
    assert_eq!(
        live.pending_continuation_ms, None,
        "the failed run's marker must not gate the new park's wake"
    );
}

#[test]
fn park_if_active_never_touches_a_terminal_or_paused_goal() {
    let (store, _d) = temp_store();
    for status in [GoalStatus::Blocked, GoalStatus::Complete, GoalStatus::Paused] {
        let g = Goal::new("sess-t", "obj", 0, 1_000).with_status(status, 1_000);
        store.put(&g).unwrap();
        assert!(
            !store.park_if_active("sess-t", 300_000, "why", 2_000).unwrap(),
            "must not park a {status:?} goal"
        );
        let live = store.get("sess-t").unwrap().unwrap();
        assert_eq!(live.status, status);
        assert!(!live.has_wait_barrier());
    }
    // No goal at all is likewise a no-op, never a resurrection.
    assert!(!store.park_if_active("nope", 300_000, "why", 2_000).unwrap());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib goal::store::tests::park_if_active 2>&1 | tail -20`
Expected: FAIL — `no method named 'park_if_active'`

- [ ] **Step 3: Implement it**

Insert after `pause_if_active` in `src/goal/store.rs`:

```rust
    /// Park a goal that is still being actively pursued on a deadline wait
    /// barrier, in one guard — the THIRD sibling of [`Self::block_if_active`]
    /// and [`Self::pause_if_active`], with the same atomicity and the same
    /// "never clobber a terminal goal" contract.
    ///
    /// The difference is intent, and it is the whole point: `Blocked` means
    /// the pursuit hit something it cannot get past, `Paused` means a human is
    /// holding it — and neither is true of a provider rate limit or a network
    /// outage, which the run-failure path used to render as a permanent
    /// verdict. Parking leaves the goal `Active` so the existing wake pipeline
    /// resumes it, and deliberately does NOT touch the welded strategy: the
    /// plan is still the plan.
    ///
    /// Clears the pending-continuation marker (the failed run's marker would
    /// otherwise gate the park's own wake behind the 60s stale grace), exactly
    /// as blocking and pausing do. `false` = nothing active to park.
    pub fn park_if_active(
        &self,
        session_id: &str,
        until_ms: u64,
        reason: &str,
        now_ms: u64,
    ) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(live) if live.is_active() => {
                Self::put_locked(
                    &conn,
                    &live
                        .with_wait_until(until_ms, Some(reason.to_string()), now_ms)
                        .with_pending_continuation(None),
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore --lib goal::store::tests::park_if_active 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
rustfmt --check src/goal/store.rs
git add src/goal/store.rs
git commit -m "goal: add park_if_active, the reversible CAS sibling of block/pause"
```

---

### Task 6: A transient provider failure parks, it does not judge (B2, driver half)

**Files:**
- Modify: `src/goal/pursuit.rs` (add `transient_park_note`)
- Modify: `src/gateway/execution_engine/goal_continuation.rs:412-444` (`block_goal_on_failure`)
- Test: `src/goal/pursuit.rs` `mod tests`

**Interfaces:**
- Consumes: `GoalStore::park_if_active` (Task 5); `ExecutionError::receipt_kind(&self) -> crate::gateway::i18n::ReceiptKind` (existing, pub); `ReceiptKind::code(self) -> &'static str` (existing, pub const); `crate::providers::llm_retry::extract_retry_after_str(raw: &str) -> Option<std::time::Duration>` (existing, pub).
- Produces: `pursuit::transient_park_note(code: &str, delay_ms: u64) -> String`.

- [ ] **Step 1: Write the failing test for the note**

Add to `src/goal/pursuit.rs` `mod tests`:

```rust
#[test]
fn transient_park_note_names_the_cause_and_the_retry_window() {
    let note = transient_park_note("RATE_LIMITED", 300_000);
    assert!(note.contains("RATE_LIMITED"), "got: {note}");
    assert!(note.contains("5m"), "the retry window must be legible: {note}");
    // The note is both the model-facing `waiting_reason` and the user-facing
    // push, so it must read as a pause, never as a halt.
    assert!(!note.to_lowercase().contains("halt"), "got: {note}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib goal::pursuit::tests::transient_park_note 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'transient_park_note'`

- [ ] **Step 3: Implement it in `src/goal/pursuit.rs`**

Add after `budget_reached_note`:

```rust
/// Why an autonomous pursuit parked on a transient provider failure, and how
/// long until it retries. Serves three readers at once — the goal's
/// `waiting_reason` (so `get` / `list` / the per-turn summary all explain the
/// pause), the resume prompt, and the origin-channel notice — so it must read
/// as a pause rather than a verdict.
///
/// `code` is the stable `ReceiptKind::code()` string (`RATE_LIMITED` /
/// `PROVIDERS_UNREACHABLE`), never the raw provider error chain: that chain is
/// already classified by the time it reaches here, and echoing it would leak
/// internal detail into a persisted, prompt-injected field.
#[must_use]
pub fn transient_park_note(code: &str, delay_ms: u64) -> String {
    format!(
        "Autonomous pursuit paused on a transient provider failure ({code}); \
         retrying in ~{}.",
        fmt_duration_ms(delay_ms)
    )
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore --lib goal::pursuit::tests::transient_park_note 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 5: Rewrite `block_goal_on_failure`**

Replace `src/gateway/execution_engine/goal_continuation.rs:406-444` (the doc comment and the whole function) with:

```rust
/// Default retry window when a transient failure carries no `Retry-After`.
const TRANSIENT_PARK_FALLBACK_MS: u64 = 300_000;

/// A continuation run failed (non-cancellation, non-busy). Route it by what
/// KIND of failure it was — the classification is not ours to invent:
/// [`ExecutionError::receipt_kind`] is already the single source three user
/// surfaces read, and its own doc records that a rate-limit or network
/// signature at this layer means every provider and model in the chain was
/// tried, so the failure is genuinely transient.
///
/// - Transient (`RateLimited` / `Unreachable`) → PARK on the existing wait
///   barrier and let the wake pipeline resume it. Blocking here used to be a
///   verdict on a 429: it made the goal invisible in the prompt, deleted the
///   welded plan, and pushed "pursuit halted" for something the codebase's own
///   classifier labels "retry is worthwhile, soon". The retry is bounded
///   without any new state — the failed run already spent an iteration at
///   claim time and the wake spends another, so the iteration cap caps the
///   recovery, and the wall-clock deadline (now enforced at the wake's
///   execution instant) caps it again.
/// - Everything else → `Blocked` with the error and an origin notice, exactly
///   as before. Without it a transient failure leaves the goal a stuck
///   `Active` with no in-flight run — a silent stall — and `Blocked` goals are
///   invisible in the prompt, so the channel notice is the user's only signal.
pub(super) async fn block_goal_on_failure(
    session: &str,
    error: &ExecutionError,
    origin: Option<&OriginRoute>,
) {
    let Some(store) = crate::goal::global() else {
        return;
    };
    let raw = format!("{error}");
    let kind = error.receipt_kind();
    let now = now_ms();

    if matches!(
        kind,
        crate::gateway::i18n::ReceiptKind::RateLimited
            | crate::gateway::i18n::ReceiptKind::Unreachable
    ) {
        let delay_ms = crate::providers::llm_retry::extract_retry_after_str(&raw)
            .map_or(TRANSIENT_PARK_FALLBACK_MS, |d| {
                u64::try_from(d.as_millis()).unwrap_or(TRANSIENT_PARK_FALLBACK_MS)
            })
            .max(1_000);
        let note = crate::goal::pursuit::transient_park_note(kind.code(), delay_ms);
        match store.park_if_active(session, now.saturating_add(delay_ms), &note, now) {
            Ok(true) => {
                // Deliberately NOT clearing the welded strategy: a provider
                // outage did not invalidate the plan.
                info!(session = %session, code = kind.code(), delay_ms,
                    "goal pursuit: transient provider failure; parked for retry");
                notify_origin(origin, format!("⏸ {note}")).await;
            }
            Ok(false) => {} // nothing active to park — stay silent.
            Err(e) => warn!(error = %e, session = %session,
                "goal pursuit: failed to persist transient park"),
        }
        return;
    }

    let reason: String = raw.chars().take(300).collect();
    let note = format!(
        "Autonomous pursuit was halted by an error and blocked for your \
         guidance: {reason}. Review progress, then clear or re-set the \
         goal to continue."
    );
    // Atomic: only blocks a goal still being pursued — never clobbers one the
    // failed run had already marked complete/blocked, nor one the user
    // cleared while the failure landed.
    match store.block_if_active(session, &note, now) {
        Ok(true) => {
            clear_goal_welded_strategy(session);
            // Notify ONLY when a goal was actually blocked: when the failed
            // run had already terminated its own goal (or the user cleared
            // it), a "pursuit halted" push would be a false alarm.
            notify_origin(
                origin,
                format!("⚠️ Autonomous pursuit of your standing goal halted: {reason}"),
            )
            .await;
        }
        Ok(false) => {} // nothing left to block — stay silent.
        Err(e) => warn!(error = %e, session = %session,
            "goal pursuit: failed to persist failure block"),
    }
}
```

- [ ] **Step 6: Verify**

Run: `cargo test -p alephcore --lib goal:: 2>&1 | tail -20` and `cargo test -p alephcore --lib --no-run 2>&1 | tail -10`
Expected: PASS / clean build

- [ ] **Step 7: Commit**

```bash
rustfmt --check src/goal/pursuit.rs src/gateway/execution_engine/goal_continuation.rs
git add src/goal/pursuit.rs src/gateway/execution_engine/goal_continuation.rs
git commit -m "goal: a transient provider failure parks for retry instead of blocking the pursuit"
```

---

### Task 7: The retirement seam retires Paused goals too (B3)

**Files:**
- Modify: `src/goal/store.rs` (add `block_if_not_terminal` after `block_if_abandonable`)
- Modify: `src/gateway/continuation_lifecycle.rs:30-51` (`block_session_goal`)
- Test: `src/goal/store.rs` `mod tests`

**Interfaces:**
- Produces: `GoalStore::block_if_not_terminal(&self, session_id: &str, note: &str, now_ms: u64) -> Result<bool>`. Only `continuation_lifecycle::block_session_goal` calls it.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn block_if_not_terminal_retires_a_paused_goal_but_not_a_finished_one() {
    let (store, _d) = temp_store();

    // Paused: the session is being retired, so a Paused goal here can never
    // be resumed again (arming is refused cross-session and this session is
    // about to become unreachable). Retire it honestly.
    let paused = Goal::new("sess-p", "obj", 0, 1_000).with_status(GoalStatus::Paused, 1_000);
    store.put(&paused).unwrap();
    assert!(store.block_if_not_terminal("sess-p", "closed", 2_000).unwrap());
    let live = store.get("sess-p").unwrap().unwrap();
    assert_eq!(live.status, GoalStatus::Blocked);
    assert_eq!(live.note.as_deref(), Some("closed"));

    // Active: same as `block_if_active`.
    let active = Goal::new("sess-a", "obj", 0, 1_000);
    store.put(&active).unwrap();
    assert!(store.block_if_not_terminal("sess-a", "closed", 2_000).unwrap());
    assert_eq!(
        store.get("sess-a").unwrap().unwrap().status,
        GoalStatus::Blocked
    );

    // Terminal states keep their own verdict and their own note.
    for status in [GoalStatus::Complete, GoalStatus::Blocked] {
        let g = Goal::new("sess-t", "obj", 0, 1_000)
            .with_status(status, 1_000)
            .with_note(Some("original".into()), 1_000);
        store.put(&g).unwrap();
        assert!(!store
            .block_if_not_terminal("sess-t", "closed", 2_000)
            .unwrap());
        let live = store.get("sess-t").unwrap().unwrap();
        assert_eq!(live.status, status);
        assert_eq!(live.note.as_deref(), Some("original"));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib goal::store::tests::block_if_not_terminal 2>&1 | tail -20`
Expected: FAIL — `no method named 'block_if_not_terminal'`

- [ ] **Step 3: Implement it**

Insert after `block_if_abandonable` in `src/goal/store.rs`:

```rust
    /// Retire a goal on a session that is being CLOSED: blocks it from either
    /// non-terminal state (`Active` or `Paused`), never from a terminal one.
    ///
    /// The wider predicate exists for exactly one caller
    /// (`continuation_lifecycle::block_session_goal`) and must not spread.
    /// [`Self::block_if_active`]'s narrower "Active only" is correct where it
    /// is used — a goal a human deliberately paused must not be demoted to
    /// `Blocked` because some unrelated run failed. But when the session
    /// itself is retired (an epoch bump or `sessions.delete`), a `Paused` goal
    /// can never be resumed: arming is refused cross-session
    /// (`GoalTool::reject_remote`) and the owning session no longer exists, so
    /// leaving it Paused only makes `goal(action='list')` advertise a pursuit
    /// nobody can restart — and leaks its welded strategy row forever. The
    /// loop's retirement seam already retires Paused loops for this reason.
    pub fn block_if_not_terminal(&self, session_id: &str, note: &str, now_ms: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(live)
                if matches!(live.status, GoalStatus::Active | GoalStatus::Paused) =>
            {
                Self::put_locked(
                    &conn,
                    &live
                        .with_status(GoalStatus::Blocked, now_ms)
                        .with_note(Some(note.to_string()), now_ms)
                        .without_wait(now_ms)
                        .with_pending_continuation(None),
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
```

- [ ] **Step 4: Point the retirement seam at it**

In `src/gateway/continuation_lifecycle.rs`, in `block_session_goal`, change the call and its doc:

```rust
/// Honestly terminate a session's goal: block it from either non-terminal
/// state and delete its welded strategy so a stale plan can neither steer
/// later turns nor block a fresh start. Best-effort; returns whether a goal
/// was actually retired (terminal goals are never clobbered —
/// [`crate::goal::GoalStore::block_if_not_terminal`]'s contract).
///
/// `Paused` is retired alongside `Active` for the same reason the loop side
/// retires paused loops: the epoch bump (or delete) makes the old session
/// unreachable, and a goal can only be re-armed from its own session, so a
/// `Paused` goal left behind is one `goal(action='list')` keeps advertising
/// and nobody can ever restart. Consumers: the epoch-retirement seam below.
/// The `ResumeCoordinator` abandon paths use the narrower
/// [`block_abandonable_session_goal`] instead.
pub fn block_session_goal(session: &str, note: &str) -> bool {
    let Some(store) = crate::goal::global() else {
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match store.block_if_not_terminal(session, note, now_ms) {
        Ok(true) => {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::goal_key(session));
            }
            info!(session = %session, "goal retired: {note}");
            true
        }
        Ok(false) => false,
        Err(e) => {
            warn!(session = %session, error = %e, "failed to retire session goal");
            false
        }
    }
}
```

Leave `block_abandonable_session_goal` untouched — its narrower predicate is correct.

- [ ] **Step 5: Verify**

Run: `cargo test -p alephcore --lib goal:: 2>&1 | tail -20` and `cargo test -p alephcore --lib --no-run 2>&1 | tail -10`
Expected: PASS / clean

- [ ] **Step 6: Commit**

```bash
rustfmt --check src/goal/store.rs src/gateway/continuation_lifecycle.rs
git add src/goal/store.rs src/gateway/continuation_lifecycle.rs
git commit -m "goal: retire a Paused goal when its session is closed, like the loop seam already does"
```

---

### Task 8: The per-turn prompt says a parked pursuit is parked (B4)

**Files:**
- Modify: `src/orchestrator/harness_bridge/context_blocks.rs:239-261` (`render_goal_summary`) and `live_deadline_status` (~line 60)
- Test: `src/orchestrator/harness_bridge/tests.rs` (the existing `render_goal_summary` tests live at ~line 590)

**Interfaces:**
- Consumes: `Goal::waiting_on_task`, `Goal::waiting_until_ms`, `Goal::has_wait_barrier()` (existing).

- [ ] **Step 1: Write the failing tests**

Add to `src/orchestrator/harness_bridge/tests.rs` next to the existing `render_goal_summary` tests:

```rust
#[test]
fn goal_summary_says_when_the_pursuit_is_parked() {
    use crate::goal::{Goal, PursuitMode};
    let task_parked = Goal::new("s", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_wait_on_task("task-7".into(), Some("waiting on the build".into()), 1_000);
    let out = render_goal_summary(&task_parked);
    assert!(out.contains("parked"), "got: {out}");
    assert!(out.contains("task-7"), "got: {out}");

    let timer_parked = Goal::new("s", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_wait_until(999_000, None, 1_000);
    let out = render_goal_summary(&timer_parked);
    assert!(out.contains("parked"), "got: {out}");
    // The remaining wait is clock-derived and belongs on the far side of the
    // prompt-cache breakpoint (`live_deadline_status`), never here: a byte
    // that changes every run would re-key the whole conversation prefix.
    assert!(!out.contains("999"), "no clock-derived bytes here: {out}");
}

#[test]
fn an_unparked_goal_summary_is_byte_identical_to_before() {
    use crate::goal::{Goal, PursuitMode};
    let g = Goal::new("s", "Migrate auth", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 });
    assert_eq!(
        render_goal_summary(&g),
        "Migrate auth (status=active, autonomous iteration 0/5)"
    );
}
```

- [ ] **Step 2: Run to verify the first fails**

Run: `cargo test -p alephcore --lib harness_bridge::tests::goal_summary_says 2>&1 | tail -20`
Expected: FAIL — the output has no "parked"

- [ ] **Step 3: Add the parked fact to `render_goal_summary`**

In `src/orchestrator/harness_bridge/context_blocks.rs`, insert a `parked` fragment and splice it into the format string:

```rust
    // The parked FACT (stable for the whole park) belongs here; the parked
    // COUNTDOWN is clock-derived and rides `live_deadline_status` on the far
    // side of the cache breakpoint. Without this the model reads a parked
    // pursuit as actively running every single turn — the tool's own `get`
    // and `list` renders both say it, and this is the one injected per turn.
    let parked = match (
        goal.waiting_on_task.as_deref(),
        goal.waiting_until_ms.is_some(),
    ) {
        (Some(task_id), _) => format!(", parked (waiting on task '{task_id}')"),
        (None, true) => ", parked (waiting)".to_string(),
        (None, false) => String::new(),
    };
    format!(
        "{} (status=active{budget}{deadline}{pursuit}{parked})",
        goal.objective
    )
```

- [ ] **Step 4: Write the failing test for the countdown half**

`live_deadline_status` needs a live store, so test the pure formatting it delegates to. Add to `src/orchestrator/harness_bridge/tests.rs`:

```rust
#[test]
fn parked_countdown_is_rendered_relative_to_now() {
    // Same convention as `render_deadline`: no clock (0) degrades instead of
    // lying, and an elapsed park reads as due rather than negative.
    assert_eq!(render_park_wait(600_000, 0), "parked, wake time unknown");
    assert_eq!(render_park_wait(600_000, 300_000), "parked, ~5m left");
    assert_eq!(render_park_wait(600_000, 600_001), "parked, wake due");
}
```

- [ ] **Step 5: Run to verify it fails**

Run: `cargo test -p alephcore --lib harness_bridge::tests::parked_countdown 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'render_park_wait'`

- [ ] **Step 6: Implement `render_park_wait` and wire it into `live_deadline_status`**

Add next to `render_deadline` in `context_blocks.rs`:

```rust
/// Render a wait barrier's remaining park as a compact phrase relative to
/// `now_ms`. Sibling of [`render_deadline`], same conventions: `now_ms == 0`
/// (no clock) degrades rather than lying, and an elapsed wake reads as due.
pub(crate) fn render_park_wait(until_ms: u64, now_ms: u64) -> String {
    if now_ms == 0 {
        return "parked, wake time unknown".to_string();
    }
    if now_ms >= until_ms {
        return "parked, wake due".to_string();
    }
    let remaining_s = (until_ms - now_ms) / 1000;
    if remaining_s < 60 {
        format!("parked, ~{remaining_s}s left")
    } else if remaining_s < 3600 {
        format!("parked, ~{}m left", remaining_s / 60)
    } else {
        format!(
            "parked, ~{}h{}m left",
            remaining_s / 3600,
            (remaining_s % 3600) / 60
        )
    }
}
```

Then in `live_deadline_status`, extend the goal block so a park countdown is emitted alongside the deadline countdown:

```rust
    if let Some(goal) = crate::goal::global()
        .and_then(|store| store.get(session_key).ok().flatten())
        .filter(crate::goal::Goal::is_active)
    {
        let mut parts = Vec::new();
        if let Some(deadline) = goal.deadline_ms {
            parts.push(render_deadline(deadline, now_ms));
        }
        // The parked countdown rides here for the same reason the deadline
        // countdown does: it changes every run, so it must stay on the far
        // side of the prompt-cache breakpoint.
        if let Some(until) = goal.waiting_until_ms {
            parts.push(render_park_wait(until, now_ms));
        }
        if !parts.is_empty() {
            lines.push(format!("standing goal: {}", parts.join(", ")));
        }
    }
```

- [ ] **Step 7: Run the tests**

Run: `cargo test -p alephcore --lib harness_bridge::tests:: 2>&1 | tail -25`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
rustfmt --check src/orchestrator/harness_bridge/context_blocks.rs src/orchestrator/harness_bridge/tests.rs
git add src/orchestrator/harness_bridge/context_blocks.rs src/orchestrator/harness_bridge/tests.rs
git commit -m "goal: surface a parked pursuit in the per-turn prompt instead of showing it as running"
```

---

### Task 9: `update(objective=…)` is refused, not silently dropped (B5)

**Files:**
- Modify: `src/builtin_tools/goal.rs` (the `GoalAction::Update` arm, right after the `reject_zero_caps(&args)?` call at ~line 829)
- Test: `src/builtin_tools/goal.rs` `mod tests`

**Interfaces:**
- Consumes: the existing `tool_with_session(session)` / `args(action)` test helpers (lines 1183, 1194).

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn update_refuses_an_objective_change_instead_of_dropping_it() {
    let (tool, _d) = tool_with_session("s-obj");
    let mut set = args(GoalAction::Set);
    set.objective = Some("original objective".into());
    tool.call(set).await.unwrap();

    let mut update = args(GoalAction::Update);
    update.objective = Some("a completely different objective".into());
    let err = tool
        .call(update)
        .await
        .expect_err("a smuggled objective must not be silently discarded");
    let msg = err.to_string();
    assert!(msg.contains("set"), "the error must point at the fix: {msg}");

    // And the stored objective is untouched.
    let out = tool.call(args(GoalAction::Get)).await.unwrap();
    assert!(out.message.contains("original objective"), "got: {}", out.message);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib builtin_tools::goal::tests::update_refuses_an_objective 2>&1 | tail -20`
Expected: FAIL — `a smuggled objective must not be silently discarded` (the call returns `Ok`)

- [ ] **Step 3: Add the rejection**

In `src/builtin_tools/goal.rs`, in the `GoalAction::Update` arm, immediately after `reject_zero_caps(&args)?;`:

```rust
                // `update` never reads `objective`, so passing one used to be
                // silently discarded — and the reply echoed the OLD objective
                // under a `success: true` "Updated." The cross-session path
                // refuses smuggled fields for exactly this reason; the local
                // path owes the same honesty.
                //
                // Refused rather than implemented: the objective is the
                // reference an `owns_reference` edge governs, and `set` is
                // where that ACL lives (`governing_owner_or_refuse`).
                // Implementing an objective change here would mean a second
                // copy of the same gate — and a gate with two implementations
                // is how a governed loop rewrites its own reference through
                // the door nobody guarded.
                if args.objective.is_some() {
                    return Err(AlephError::tool(
                        "goal 'update' cannot change the objective — it only adjusts \
                         status, caps, budget, deadline, gate, lessons and notes. Use \
                         goal(action='set', objective='…') to replace the objective \
                         (that path carries the governance check a replacement needs); \
                         note it resets the autonomous iteration count."
                            .to_string(),
                    ));
                }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore --lib builtin_tools::goal::tests:: 2>&1 | tail -25`
Expected: PASS (all goal tool tests)

- [ ] **Step 5: Commit**

```bash
rustfmt --check src/builtin_tools/goal.rs
git add src/builtin_tools/goal.rs
git commit -m "goal: refuse update(objective=) instead of silently discarding it"
```

---

### Task 10: Wait args are validated against the post-update goal (B6)

**Files:**
- Modify: `src/builtin_tools/goal.rs:542-571` (`validate_wait_args`) and the `GoalAction::Update` arm ordering (~lines 830 and ~929-942)
- Test: `src/builtin_tools/goal.rs` `mod tests`

**Interfaces:**
- Produces: `fn validate_wait_args(args: &GoalArgs, goal: &Goal) -> Result<()>` — same signature, new contract: `goal` is now the goal **after** this update's non-wait fields have been applied.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn wait_is_refused_on_a_goal_this_update_leaves_non_active() {
    let (tool, _d) = tool_with_session("s-wait-blocked");
    let mut set = args(GoalAction::Set);
    set.objective = Some("obj".into());
    set.pursuit_max_iterations = Some(5);
    tool.call(set).await.unwrap();
    // Self-report blocked, then try to park it. `wait_parked` requires an
    // Active goal, so the barrier would never wake and `get` would advertise
    // a park that can never end.
    let mut block = args(GoalAction::Update);
    block.status = Some(GoalStatus::Blocked);
    tool.call(block).await.unwrap();

    let mut park = args(GoalAction::Update);
    park.wait_minutes = Some(30);
    let err = tool
        .call(park)
        .await
        .expect_err("parking a non-active goal must be refused, not silently inert");
    assert!(err.to_string().contains("active"), "got: {err}");
}

#[tokio::test]
async fn wait_is_allowed_when_the_same_update_makes_the_goal_autonomous() {
    let (tool, _d) = tool_with_session("s-wait-promote");
    let mut set = args(GoalAction::Set);
    set.objective = Some("obj".into());
    tool.call(set).await.unwrap(); // interactive (Passive) goal

    // One call that both arms autonomous pursuit and parks it. Validating
    // against the PRE-update goal rejected this as "not autonomous" even
    // though this very call makes it autonomous.
    let mut promote = args(GoalAction::Update);
    promote.pursuit_max_iterations = Some(5);
    promote.wait_minutes = Some(30);
    let out = tool.call(promote).await.unwrap();
    assert!(out.success, "got: {}", out.message);
    assert!(out.message.contains("parked"), "got: {}", out.message);
}
```

- [ ] **Step 2: Run to verify both fail**

Run: `cargo test -p alephcore --lib builtin_tools::goal::tests::wait_is 2>&1 | tail -25`
Expected: both FAIL — the first returns `Ok`, the second returns the "wait barriers only apply to autonomous goals" error.

- [ ] **Step 3: Add the status check to `validate_wait_args`**

In `src/builtin_tools/goal.rs`, extend the function (and its doc) — insert the status check after the `PursuitMode::Active` check:

```rust
/// Boundary validation for the wait-barrier parameters (P7): they only make
/// sense on a still-active AUTONOMOUS goal — the continuation path is the
/// barrier's sole consumer, so parking a passive/terminal goal would silently
/// do nothing and the model must learn that here, not never.
///
/// `goal` is the goal AS THIS UPDATE LEAVES IT, not the stored snapshot: one
/// call can both arm autonomous pursuit and park it, and validating the
/// pre-update row rejected that legal combination while accepting the illegal
/// one (a park on a goal the same call moves out of `Active`).
fn validate_wait_args(args: &GoalArgs, goal: &Goal) -> Result<()> {
    if args.wait_minutes.is_none() && args.wait_for_task.is_none() {
        return Ok(());
    }
    if args.wait_minutes.is_some() && args.wait_for_task.is_some() {
        return Err(AlephError::tool(
            "pass either wait_minutes or wait_for_task, not both — a pursuit \
             parks on exactly one barrier."
                .to_string(),
        ));
    }
    if !matches!(goal.pursuit, PursuitMode::Active { .. }) {
        return Err(AlephError::tool(
            "wait barriers only apply to autonomous goals (set with \
             pursuit_max_iterations) — an interactive goal has no continuation \
             loop to park."
                .to_string(),
        ));
    }
    if goal.status != GoalStatus::Active {
        return Err(AlephError::tool(format!(
            "a wait barrier only wakes an active pursuit; this goal is \
             '{}'. Pass status='active' in the same update to resume it, or \
             drop the wait.",
            goal.status.as_str()
        )));
    }
    if args
        .wait_for_task
        .as_deref()
        .is_some_and(|t| t.trim().is_empty())
    {
        return Err(AlephError::tool(
            "wait_for_task requires a non-empty coordination task id.".to_string(),
        ));
    }
    Ok(())
}
```

- [ ] **Step 4: Move the call site**

In the `GoalAction::Update` arm:

1. **Delete** the `validate_wait_args(&args, &goal)?;` line that currently sits right after `reject_zero_caps(&args)?;` (~line 830).
2. **Insert** it immediately before the wait-barrier application block — i.e. between the `if let Some(lesson) = args.lesson.clone() { … }` block and the `// Wait barrier (model self-park, R7)` comment (~line 929):

```rust
                // Validated here, not at the top of the arm: `goal` now
                // carries this update's status and pursuit changes, so one
                // call may legitimately arm autonomous pursuit and park it,
                // and a park on a goal this call moves out of `Active` is
                // correctly refused. See `validate_wait_args`.
                validate_wait_args(&args, &goal)?;
                // Wait barrier (model self-park, R7): the continuation hook
```

- [ ] **Step 5: Run to verify both pass**

Run: `cargo test -p alephcore --lib builtin_tools::goal::tests:: 2>&1 | tail -25`
Expected: PASS — including the pre-existing `wait_rejected_on_interactive_goal_and_on_set` and `wait_args_are_mutually_exclusive_and_nonzero`. If `wait_rejected_on_interactive_goal_and_on_set` now fails, read it: it may assert the pre-update behavior for a call that also promotes the goal. Adjust that test only if it encodes the old (wrong) contract, and say so in the commit message.

- [ ] **Step 6: Commit**

```bash
rustfmt --check src/builtin_tools/goal.rs
git add src/builtin_tools/goal.rs
git commit -m "goal: validate wait barriers against the post-update goal, in both directions"
```

---

### Task 11: One escaping source for the objective (entropy reduction)

**Files:**
- Modify: `src/goal/pursuit.rs:103-111` (`objective_block`)
- Test: `src/goal/pursuit.rs` `mod tests`

**Interfaces:**
- Consumes: `crate::thinker::xml_util::escape_xml(&str) -> String` (existing; `StandingGoalLayer` already uses it on the same text).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn objective_block_escapes_through_the_one_shared_source() {
    let g = Goal::new("s", "fix <div> & </objective> handling", 0, 0);
    let block = objective_block(&g);
    assert!(block.starts_with("<objective>") && block.ends_with("</objective>"));
    // The wrapper's own closer cannot appear in the body...
    assert_eq!(
        block.matches("</objective>").count(),
        1,
        "only the wrapper's closer may survive: {block}"
    );
    // ...and neither can any other raw markup, because the body goes through
    // the same `xml_util::escape_xml` the `<standing_goal>` layer applies to
    // this exact text. Two escaping schemes for one string is how they drift.
    assert!(!block.contains("<div>"), "got: {block}");
    assert!(block.contains("&lt;div&gt;"), "got: {block}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib goal::pursuit::tests::objective_block 2>&1 | tail -20`
Expected: FAIL — the body still contains a raw `<div>`

- [ ] **Step 3: Delegate the escaping**

Replace `objective_block` in `src/goal/pursuit.rs`:

```rust
/// The objective is USER DATA quoted inside a prompt, not instructions — wrap
/// it in an explicit container so a crafted objective reads as content.
///
/// Escaping goes through `xml_util::escape_xml`, the same single source
/// `StandingGoalLayer` applies to this exact string. The hand-rolled
/// `replace("</objective", …)` this replaces neutralized only the wrapper's
/// own closer, so the two renders of one objective disagreed on what counted
/// as markup — and a bespoke escape is one measured against nothing.
fn objective_block(goal: &Goal) -> String {
    format!(
        "<objective>{}</objective>",
        crate::thinker::xml_util::escape_xml(&goal.objective)
    )
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore --lib goal::pursuit::tests:: 2>&1 | tail -25`
Expected: PASS. If a pre-existing prompt test asserts the raw (unescaped) objective text, update its expectation — the byte change is intentional and only affects objectives containing `&`, `<` or `>`.

- [ ] **Step 5: Commit**

```bash
rustfmt --check src/goal/pursuit.rs
git add src/goal/pursuit.rs
git commit -m "goal: escape the continuation prompt's objective through the shared xml_util source"
```

---

### Task 12: Full verification + documentation

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` (§4.1, line ~908-913)
- Modify: `CLAUDE.md`

- [ ] **Step 1: Run the full minimum-credible verification set**

```bash
cargo test -p alephcore --lib 2>&1 | tail -20
cargo test -p alephcore --features test-helpers --test '*' --no-run 2>&1 | tail -10
cargo check -p aleph-panel 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: all green. `cargo check -p alephcore` alone proves nothing here — it does not compile `#[cfg(test)]` code, and this round changed a public signature (`confirm_fire`) with six test call sites.

- [ ] **Step 2: Fix anything the verification set surfaces, then re-run it**

Commit any fixes separately with a message naming what broke.

- [ ] **Step 3: Update `docs/reference/FEATURE_LOCATOR.md` §4.1**

Append to the `**状态**` paragraph of §4.1, in the established voice (dense, mechanism-first, naming the failure the fix prevents):

```
**第 10 轮（2026-08-05，对标 codex `ext/goal` 2026 版 + kimi ralph_loop，结论：参考项目无一项架构可移植——claim 原子性/闸门/wait-barrier 三处 Aleph 均更优；本轮全部价值在自身断线）**：① **「在认领时求值的上限不是上限」平移到 goal（§4.2 loop round-9 的教训）**——Timer 障的 `should_continue` 判在 `now`，而 `Fire{delay=wake−now}` 让 run 在数小时后执行，`confirm_fire` 只比 marker 不看时钟 ⇒ `wait_minutes=180` 配 `timeout_minutes=30` 在用户上限之后 **150 分钟**跑完整轮并推送。新增单一源 `pursuit::fires_out_of_bounds`（与 `looping::pursuit` 同名同形），**三个消费者缺一不可**：claim 端按投影 wake 判界、`confirm_fire` 端复检、boot `rearm_parked_goals`（它**刻意绕过 claim**，于是也绕过了 claim 的每一道界——重启正是 parked wake 最可能已过期的时刻）。`confirm_fire` 因此从 `bool` 改为 `FireDecision::{Proceed,Superseded,OutOfBounds{note}}`——**越界不能读作"被取代"**：静默跳过会把"跑晚了"换成"Active 且永无人认领"的永久静默卡死，所以 OutOfBounds 在同一把锁里 Blocked+note，调用方走 Exhausted 臂**同一条** `notify_origin`。同批 `GoalWakeService` 接上 `SessionStore` ⇒ 唤醒 claim 传 `tree_tokens` 而非 `None`（此前**每一次唤醒都免预算**，而任务障 goal 的唯一驱动就是它）。② **限流不是判决**——`block_goal_on_failure` 对任何错误一律 Blocked + 删焊入计划 + 推「⚠️ 已中止」，而 `ExecutionError::receipt_kind()`（三个用户面已在用的单一源，其 doc 自陈此层的限流/网络签名意味着**整条 provider 链都试过**）、`llm_retry::extract_retry_after_str`、wait-barrier **三块零件齐备、从没连线**。现 `RateLimited`/`Unreachable` 走新增的 `GoalStore::park_if_active`（`block_if_active`/`pause_if_active` 的第三个 CAS 孪生）park 重试、**不删计划**（provider 宕机没让计划失效）、推「⏸」；其余一字不变。**上限零新增字段**：失败那次 run 已在 claim 时花掉一次迭代、唤醒再花一次 ⇒ 既有迭代上限封顶，① 修好的墙钟界封第二道。codex 为此立了 `usage_limited` 独立状态，Aleph 不需要——`Goal` 是 serde 持久化类型，新枚举值让旧二进制读不了新行。③ **退休会话上的 Paused goal「看得见、停不掉」**——`terminate_session_continuations` 只 `block_if_active`，而 Paused goal 的恢复（`status='active'`）**必须在它自己会话跑**（`reject_remote`），epoch bump / `sessions.delete` 之后那个会话已不可达 ⇒ `goal(list)` 从每个频道都列得出、谁也重启不了，焊入的 strategy 行永久泄漏。loop 侧**恰好修过这一条并把理由写进了它自己的注释**。新增 `block_if_not_terminal` **只**给退休缝用——`block_goal_on_failure`/agent-miss 保持 `block_if_active`（那里"只 Active"是对的：一个被人刻意 pause 的 goal 不该因无关 run 失败被降级）。④ **每轮注入的 `<standing_goal>` 不提 wait barrier**——parked pursuit 对模型读作"正在跑"；三个 render 里 `GoalTool::render` 与 `render_list_line` 都说了，**唯独每轮都发的那个漏了**。现 parked **事实**（park 期间稳定）进 `render_goal_summary`、parked **倒计时**（时钟派生）进 `live_deadline_status`（transient tail，`render_park_wait`）——沿用本模块既有的缓存断点切分，不新造通道。⑤ `update(objective=…)` **静默丢弃**并回「Updated. Standing goal: 旧目标」（跨会话路径为同一理由早已显式拒绝夹带字段）；现拒绝并指路 `set`——**不在 update 里实现它**，因为 objective 是 `owns_reference` 治理的参照而那道 ACL 长在 `set` 上，实现＝同一个闸的第二份拷贝。⑥ `validate_wait_args` 拿**更新前**的 goal 校验且不看 status，两个方向都错：能给 Blocked goal 挂**永远不会醒**的障（`render` 还照说 "parked for ~60m more"），又误拒 `update(pursuit_max_iterations=5, wait_minutes=10)` 这种同调用内刚变自主的合法组合；现改为对**更新后**的 goal 校验并加 status 闸。⑦ 删 Timer 臂不可达分支（`wait_parked` 已含 Active 状态+Active pursuit ⇒ `!should_continue` 蕴含 `exhausted_while_active`），补单测钉住该不变量。⑧ 熵减：`objective_block` 收敛到 `xml_util::escape_xml`（`StandingGoalLayer` 对**同一段文本**已用这一份，此前两处两套转义）。**刻意不做**：codex 的 `usage_limited` 独立状态（见②）、codex 的 token 口径 `input−cached+output`（要动 `get_total_tokens` 全体消费者）、每轮 live 剩余 token 预算（要在 prompt 组装路径加一次异步 store 读；剩余量已在续跑 prompt 与 `goal(get)` 可得）、codex「blocked 需连续 3 轮」prompt 契约（R9 剪枝，`AUDIT_CONTRACT` 已有弱版本）、codex `time_used_seconds` 活跃耗时（要新持久字段 + 逐 turn 记账）、goal 的 Panel/RPC 面（独立一轮）。
```

Then append to the `**打磨话术**` paragraph:

```
**（2026-08-05 起）**「**界限必须在执行时刻成立**：Timer 障的 wake 走 `pursuit::fires_out_of_bounds` 判**投影后的执行时刻**，三个消费者（claim / `confirm_fire` / boot rearm）缺一不可——boot rearm 刻意绕过 claim，所以它必须自己带界。`confirm_fire` 返回 `FireDecision` 三态，**越界绝不能读作 Superseded**（静默跳过＝Active 且永无人认领的永久卡死）。**瞬时 provider 失败 park 不 Block**：判据只用 `ExecutionError::receipt_kind()`，别再写第二个分类器；park 走 `park_if_active` 且**不删焊入计划**。**退休缝用 `block_if_not_terminal`，失败/agent-miss 用 `block_if_active`**——两个谓词的差别是刻意的，别合并。`update` 不能改 objective（那是 `set` 的 ACL 管辖的参照）；wait 参数一律对**更新后**的 goal 校验。」
```

- [ ] **Step 4: Update `CLAUDE.md`**

Two edits:

1. In the §4.2 note that already carries **「在认领时求值的上限不是上限」**, extend the 推论 sentence so it names goal as the second instance:

```
**推论（适用于任何长跑单元）**：凡「先认领、后执行」的调度，界限要在**执行时刻**成立；只在入队处判等于没判。**2026-08-05 已在 goal 的 wait-barrier 上复发一次**——同一个形状，第二个子系统：那里绕过 claim 的 boot rearm 路径连一道界都没有，所以「谁绕过了认领，谁就得自己带界」是这条纪律的第二半。
```

2. Add a new bullet near the 「能力接上了 ≠ 模型会用它」 family:

```
> **⚠️ 分类器已经存在，只是没人问它**: `block_goal_on_failure` 曾把**任何**失败都判成 goal 的终态（Blocked + 删焊入计划 + 推「已中止」），而 `ExecutionError::receipt_kind()` **早就是三个用户面共用的单一源**，其 doc 自陈：此层出现限流/网络签名意味着整条 provider 链都试过、失败**确属瞬时**。同一仓里 `llm_retry::extract_retry_after_str` 能给退避时长、wait-barrier 能 park 自唤醒——**三块零件齐备，谁都没连**。判据一句话：写下一个 `if error { 终态 }` 之前先问**「这个错误已经被谁分过类了」**；答案通常是"有，而且比你打算写的那版更准"。同族是 §4.9「能被精确回答的数字，别用常量猜」——那条讲魔数，这条讲**分支**，两者都是「已知事实就在同一个 crate 里，零调用」。
```

- [ ] **Step 5: Final verification and commit**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
git add docs/reference/FEATURE_LOCATOR.md CLAUDE.md
git commit -m "docs: record goal standing-goal round 10 in FEATURE_LOCATOR and CLAUDE.md"
git log --oneline main..HEAD
```

Expected: 11-12 commits on `worktree-goal-round10`.

---

## Self-Review Notes

**Spec coverage:** B1 → Tasks 1-4; B2 → Tasks 5-6; B3 → Task 7; B4 → Task 8; B5 → Task 9; B6 → Task 10; B7 → Task 1 Step 7-8; entropy reduction → Task 11; spec §6 deferred ledger → Task 12 Step 3. All spec sections have a task.

**Type consistency:** `fires_out_of_bounds(&Goal, u64, u64) -> bool` (defined Task 1; used Tasks 1, 2, 3). `FireDecision` (defined Task 2; used Tasks 2, 3). `park_if_active(&str, u64, &str, u64) -> Result<bool>` (defined Task 5; used Task 6). `block_if_not_terminal(&str, &str, u64) -> Result<bool>` (defined Task 7; used Task 7). `transient_park_note(&str, u64) -> String` (defined Task 6; used Task 6). `render_park_wait(u64, u64) -> String` (defined Task 8; used Task 8). No name drift.

**Known risk flagged in-plan:** Task 2 Step 7 depends on where `origin` is bound in `execute.rs`'s continuation closure — the step tells the implementer to read the function first and pick the smaller diff rather than guessing.
