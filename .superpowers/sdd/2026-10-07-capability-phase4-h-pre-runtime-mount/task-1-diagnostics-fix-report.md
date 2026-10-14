# Task 1 — Diagnostic Control Fix Report (round-2 review resolution)

**Status:** DONE
**Base commit:** `8d9f7804b98065caeb8fab88a5884936b11e2367` on
`capability-phase4-follow-up` — `capability: add Task 1 diagnostic control surface`
**Parent report:** `.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/task-1-diagnostics-report.md`
**Files in scope (only):**
- `src/capability/diagnostic_control.rs`
- `src/capability/projection_host.rs`

`src/capability/mod.rs` is NOT touched: it already exposes `pub mod diagnostic_control;`
in the base commit, no Task-1-round-2 diff is needed.

**Tests:** guarded
`python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py test --package alephcore --lib capability::diagnostic_control --no-fail-fast`
→ `10 passed; 0 failed`
plus scoped re-runs:
- `capability::projection_host` → `33 passed; 0 failed`
- `capability::` (whole module) → `169 passed; 0 failed` (was 160 in the base
  report; +9 = 2 new in `diagnostic_control` + 7 new in `projection_host`)

---

## 1. Review findings resolved

The previous diagnostic-control surface (`task-1-diagnostics-report.md`,
GREEN at base `8d9f7804b`) had two structural gaps flagged for a follow-up
fix wave. Both are addressed in this round; no production semantics beyond
those two gaps changed.

| Finding | Fix |
|---|---|
| `DiagnosticControl::new(host, tree)` did not verify that the supplied `tree` was the EXACT `Arc<OwnershipTree>` the host was mounted on. A second, structurally-equivalent tree would silently pair the control with a parallel authority. | `DiagnosticControl::new` now returns `Result<Self, DiagnosticError>`. Under the host's authority, `host.owner_tree()` is exposed (crate-visible) and the constructor checks `Arc::ptr_eq(host.owner_tree(), &tree)`. Mismatch → `DiagnosticError::AuthorityMismatch` (fail-closed, no fallback). Same-tree construction succeeds. |
| `diagnostic_hold` relied on the CALLER's future to clear the slot. Drop / abort of the holder future would leave the slot occupied forever (the per-host `Notify` would never fire). The hold did not survive its owner's lifetime either, but it would leak and block subsequent diagnostics. | Hold state is `DiagnosticHoldState { next_id: u64, active: Option<DiagnosticHold> }`. The host OWNS the expiry: a `tokio::spawn`-ed alarm (holding only a `Weak<HostInner>`) sleeps until `release_at` and calls `clear_if(id)` — a slot clear conditional on the id still being live. Explicit `release` aborts the alarm (its `clear_if` is a no-op for the now-emptied slot). Closing also `release`s first. |

The fix preserves every existing diagnostic behavior. No source-worker
shape, no applier-worker shape, no registry cursor, no facade snapshot, no
`notify`/hold protocol beyond the two specific gaps above.

## 2. Cleanup note (full-repo rustfmt was reverted)

The previous Opus session left the working tree in a 155-file modified
state: the two `src/capability/*.rs` Task-1 files plus 153 unrelated
`rustfmt` reformat diffs. Per scope, the rustfmt noise was restored via
`git restore --worktree -- <path>` for the 153 paths
(`git diff --name-only` minus the two capability files). The four
human-owned untracked plan/prompt files at
`docs/superpowers/{plans,prompts}/2026-10-0[67]-capability-phase4*.md`
were NOT touched. Final `git diff --name-only` is exactly:

```
src/capability/diagnostic_control.rs
src/capability/projection_host.rs
```

`rustfmt --edition 2021 --check` on those two files is clean; `git diff
--check` is clean. No `cargo fmt` was run on the full tree.

## 3. Source evidence (final GREEN code)

### 3.1 Authority identity (`diagnostic_control.rs`)

```rust
/// Build a diagnostic control over the given host and ownership tree.
/// Both must be the SAME authority objects the rest of the system uses:
/// `tree` must be the exact `Arc` the host was mounted on (checked by
/// pointer identity, never by value). A mismatch is refused fail-closed
/// with [`DiagnosticError::AuthorityMismatch`].
pub fn new(
    host: Arc<ProjectionHost>,
    tree: Arc<OwnershipTree>,
) -> Result<Self, DiagnosticError> {
    if !Arc::ptr_eq(host.owner_tree(), &tree) {
        return Err(DiagnosticError::AuthorityMismatch);
    }
    Ok(Self { host, tree })
}
```

`ProjectionHost::owner_tree` is `pub(crate)` and returns `&Arc<OwnershipTree>`
so identity is checked with `Arc::ptr_eq`, never by structural equality of
`OwnershipTree`. Mismatch is a unit variant; the control never falls back
to "find the closest tree" or "shadow the host's tree".

### 3.2 Host-owned hold slot (`projection_host.rs`)

```rust
struct DiagnosticHold {
    /// Per-host monotonic id; every clear is conditional on it so a stale
    /// timer or holder can never clear a newer hold.
    id: u64,
    plane: DiagnosticPlane,
    release_at: tokio::time::Instant,
    /// Host-owned expiry alarm. It (not the caller's future) is responsible
    /// for clearing the slot at `release_at`; aborted on explicit release.
    timer: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct DiagnosticHoldState {
    next_id: u64,
    active: Option<DiagnosticHold>,
}

impl DiagnosticHoldState {
    /// Clear the slot if (and only if) it still holds hold `id`.
    fn clear_if(&mut self, id: u64) -> bool {
        if self.active.as_ref().is_some_and(|h| h.id == id) {
            self.active = None;
            true
        } else {
            false
        }
    }
}
```

`diagnostic_hold` install path (excerpt):

```rust
guard.next_id = guard.next_id.wrapping_add(1);
let id = guard.next_id;
// The host owns expiry: the alarm clears the slot even if the
// caller's future is dropped. It holds only a `Weak` so it never
// extends the host's lifetime, and clears only its own id. It is
// spawned under the slot lock, so it cannot observe the slot
// before this hold is installed.
let weak = Arc::downgrade(&self.inner);
let timer = tokio::spawn(async move {
    tokio::time::Instant::sleep_until(release_at).await;
    if let Some(inner) = weak.upgrade() {
        let cleared = inner
            .diagnostic_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear_if(id);
        if cleared {
            inner.diagnostic_hold_notify.notify_waiters();
        }
    }
});
guard.active = Some(DiagnosticHold { id, plane, release_at, timer });
```

`diagnostic_release`:

```rust
let taken = self.inner.diagnostic_hold.lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner)
    .active.take();
if let Some(hold) = taken {
    // The slot is already cleared under the lock; aborting the alarm
    // only avoids a redundant wake (its id-guarded clear is a no-op).
    hold.timer.abort();
    self.inner.diagnostic_hold_notify.notify_waiters();
}
```

`diagnostic_close` calls `release()` first, then runs the existing
`start_shutdown(&self.inner)` and waits on `completion_notify` with the
5000 ms deadline (no change to the close protocol — `close` still bypasses
the hold by releasing it).

### 3.3 Race analysis

| Race | Outcome |
|---|---|
| Holder future dropped / aborted before expiry | Slot is still occupied; the host-owned alarm sleeps until `release_at`, then `clear_if(id)` empties the slot. Proved by `dropped_hold_future_is_released_by_host_timer` (timer fires; second hold accepted). |
| `release()` and timer fire concurrently | Either order is safe: `release()` first empties the slot under the lock, then the timer fires and `clear_if` finds `None` (or a newer id) and is a no-op. Timer first empties the slot and notifies; `release()`'s `take()` then returns `None` (or a different id) and aborts the no-longer-relevant `JoinHandle`. |
| Stale timer vs. newer hold | New hold gets a fresh id. Old timer's `clear_if(old_id)` finds the new id active and is a no-op. Proved by `stale_timer_does_not_clear_newer_hold`. |
| `close()` during active hold | `close()` first calls `release()` (which aborts the alarm) then `start_shutdown` then awaits the shared completion boundary. The alarm cannot run after `release()` because `release()`'s `take()` is sequenced before the alarm's `Weak::upgrade` finds a populated slot. |
| Host dropped before expiry | `Weak::upgrade` returns `None` inside the alarm; alarm exits without touching the slot. The `Drop` for `HostInner` runs the `cancel` token and joins the workers. |
| Concurrent install attempts | Second `hold` finds `active.is_some()` and returns `DiagnosticError::HoldAlreadyActive`. The active hold is not replaced. Proved by `hold_rejects_out_of_range_and_overlapping_requests`. |

## 4. Tests

### 4.1 `capability::diagnostic_control` (10 tests, all pass)

```
running 10 tests
test capability::diagnostic_control::tests::dispose_runtime_irreversible ... ok
test capability::diagnostic_control::tests::same_owner_tree_is_accepted_and_shared ... ok
test capability::diagnostic_control::tests::mismatched_owner_tree_is_rejected ... ok
test capability::diagnostic_control::tests::bump_returns_generation ... ok
test capability::diagnostic_control::tests::status_distinguishes_queued_from_applied ... ok
test capability::diagnostic_control::tests::bump_then_status_observes_strictly_greater_owner_generations ... ok
test capability::diagnostic_control::tests::revoke_tool_uses_canonical_id_and_does_not_advance_cursor ... ok
test capability::diagnostic_control::tests::close_bypasses_hold_and_awaits_shared_completion ... ok
test capability::diagnostic_control::tests::source_hold_lag_recovers_with_snapshot ... ok
test capability::diagnostic_control::tests::delivery_hold_overflow_replaces_stale_pending_state ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 20816 filtered out; finished in 2.02s
CARGO_EXIT: 0
```

Two new tests are marked with the new requirement name (matching the
review ask):

- `mismatched_owner_tree_is_rejected` — builds a host on tree `A`,
  constructs the control with tree `B`; `Arc::ptr_eq` fails,
  `DiagnosticError::AuthorityMismatch` returned. Then re-constructs the
  control with `A` and asserts `Ok`.
- `same_owner_tree_is_accepted_and_shared` — constructs the control with
  the mounted `Arc` clone, then proves the bump landed on the SAME tree
  by reading the same monotonic nonce from a direct caller's `bump`.

The 8 pre-existing tests are preserved verbatim except for the
`.unwrap()` added to the 5 `DiagnosticControl::new` callsites inside
`tests`. No assertion weakening.

### 4.2 `capability::projection_host` (33 tests, all pass)

```
running 33 tests
... (26 pre-existing) ...
test capability::projection_host::tests::hold_on_closed_host_is_rejected ... ok
test capability::projection_host::tests::diagnostic_close_timeout_stays_fail_closed ... ok
test capability::projection_host::tests::cancellation_bypasses_active_hold ... ok
test capability::projection_host::tests::dropped_hold_future_is_released_by_host_timer ... ok
test capability::projection_host::tests::hold_rejects_out_of_range_and_overlapping_requests ... ok
test capability::projection_host::tests::delivery_hold_overflow_queues_invalidated_before_replacement ... ok
test capability::projection_host::tests::stale_timer_does_not_clear_newer_hold ... ok
... (more pre-existing) ...

test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 20793 filtered out; finished in 0.31s
CARGO_EXIT: 0
```

The seven new tests map directly to the review asks:

| Test | What it pins |
|---|---|
| `dropped_hold_future_is_released_by_host_timer` | `task.abort()` does not clear the slot. The host-owned timer clears it at expiry (bounded wait). A second hold is accepted after the first releases. |
| `stale_timer_does_not_clear_newer_hold` | First hold (100 ms) released manually; second hold (1000 ms) installed. At t=150 ms (past first expiry) the slot is still the second hold. The stale timer is a no-op via `clear_if`. |
| `hold_rejects_out_of_range_and_overlapping_requests` | `Duration::ZERO` and `Duration::from_millis(5001)` both return `InvalidHoldDuration`; the slot is `None` afterwards. Overlap (second hold while first is active) returns `HoldAlreadyActive`; the active hold's plane is unchanged. |
| `delivery_hold_overflow_queues_invalidated_before_replacement` | While the `Delivery` hold is held, the source keeps running, overflow replaces the stale backlog, the queue head is `Invalidated` followed by the final-membership replacement, and `replacement_count` is non-zero. After `release()`, the REAL applier drains and the final applied snapshot has all 10 tools. Capacity is not resized. |
| `cancellation_bypasses_active_hold` | A `SourceIntake` hold is installed; ordinary `close_and_await()` finishes well under 5000 ms — `close` is not blocked by a diagnostic hold. |
| `diagnostic_close_timeout_stays_fail_closed` | The applier test gate stalls the real applier mid-delivery. `diagnostic_close` returns `CloseTimeout { elapsed_ms: 5000 }` in ≤ 5001 ms, host stays `is_closing() == true`, `lifecycle == Closing`, the completion outcome is `None` (not fabricated). Releasing the gate lets the retained joins complete and `lifecycle` advances to `Closed`. |
| `hold_on_closed_host_is_rejected` | After `close_and_await`, `diagnostic_hold` returns `DiagnosticError::Closed`; the slot is `None` (no half-installed hold). |

### 4.3 `capability::` whole module (169 tests, all pass)

```
test result: ok. 169 passed; 0 failed; 0 ignored; 0 measured; 20657 filtered out; finished in 20.42s
CARGO_EXIT: 0
```

The +9 vs. the base report (160 → 169) is exactly the 2 new
`diagnostic_control` tests plus the 7 new `projection_host` tests. No
regression in any other `capability::` module.

## 5. Guard / preflight

Every cargo invocation went through
`.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py`
(page-size header check, global flock, active cargo/rustc process check).

```
{"page_bytes": 16384, "pages": {"free": 234846, "inactive": 271898, "speculative": 19412}, "available_KiB": 8418496, "threshold_KiB": 4194304, "gate": "PASS"}
{"page_bytes": 16384, "pages": {"free": 339626, "inactive": 180449, "speculative": 49233}, "available_KiB": 9108928, "threshold_KiB": 4194304, "gate": "PASS"}
{"page_bytes": 16384, "pages": {"free": 365487, "inactive": 113649, "speculative": 40275}, "available_KiB": 8310912, "threshold_KiB": 4194304, "gate": "PASS"}
```

All three guards passed the 4 194 304 KiB threshold. No raw `cargo`
invocation, no `target` deletion, no subagents.

## 6. Diff scope (final)

```
 src/capability/diagnostic_control.rs |  66 +++++++++++++++++++++++++++++++-----
 src/capability/projection_host.rs    | 961 +++++++++++++++++++++++++++++++-------
 2 files changed, 839 insertions(+), 188 deletions(-)
```

(More negative lines than the prior 188 include a small number of
`pointee.lock()…into_inner` rewrites where the lock chain gained an extra
`.active` access path; the diff still nets positive because the new
tests, the `DiagnosticHoldState` machinery, the alarm spawn, and the
`owner_tree` accessor are net additions.)

`rustfmt --edition 2021 --check` is clean on both files; `git diff
--check` is clean.

## 7. Files NOT Touched (per scope)

* `src/capability/mod.rs` — already exposes `pub mod diagnostic_control;`
  in the base commit; no round-2 diff needed, NOT modified.
* `src/builtin/**`, `src/gateway/**`, `qa/**`, `docs/**` — untouched
  (the four untracked plan/prompt files at
  `docs/superpowers/{plans,prompts}/2026-10-0[67]-capability-phase4*.md`
  are human-owned inputs and remain untracked).
* The 153 unrelated `rustfmt` modifications that were present in the
  pre-fix working tree have been `git restore --worktree --`'d back to
  the base commit. `git diff --name-only` returns only the two files
  above.
* `Cargo.toml`, `Cargo.lock`, all `tests/**` — untouched.

## 8. What This Report Does NOT Prove (cannot-verify)

* **Real binary QA** (Task 5) is out of scope. This report does NOT
  claim the `aleph-server` binary boots, drains, or behaves correctly
  end-to-end on top of the diagnostic surface.
* **Operator / handler-identity / runtime-registration checks** (Task 2
  / 3) are still NOT implemented in this commit. `DiagnosticControl` has
  no caller-identity check, no `runtime_register` helper, and no
  operator-side checks.
* **Long-tail contention** for `diagnostic_hold` (e.g. two threads
  racing install + release + drop simultaneously) is not exercised by a
  dedicated test; the implementation is sound under sequential
  reasoning (single mutex, `id`-guarded clear, `Weak` upgrade before
  mutation), but a stochastic stress test is not in this report.
* **Hold on a host whose applier is already mid-`Invalidated`**
  specifically is not pinned; the design treats the slot as
  mutex-guarded and lets the existing worker path handle ordering
  naturally.
* **Diagnostics plan amendment actions** other than the two findings
  closed above are not claimed by this report. The supplement
  (`docs/superpowers/plans/2026-10-07-capability-phase4-h-pre-diagnostics-supplement.md`)
  remains the governing design; the
  `diagnostics-spec-review.md` amendment list is unchanged.

## 9. Reproduction

```bash
# Cleanup (idempotent: no-op if the 153 files were never modified)
cd /Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up
git diff --name-only \
  | grep -vE '^src/capability/(diagnostic_control|projection_host)\.rs$' \
  | xargs git restore --worktree --

# Format check (in-scope only)
rustfmt --edition 2021 --check \
  src/capability/diagnostic_control.rs \
  src/capability/projection_host.rs

# Tests (via the project's guard)
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py \
  test --package alephcore --lib capability::diagnostic_control --no-fail-fast

python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py \
  test --package alephcore --lib capability::projection_host --no-fail-fast

python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py \
  test --package alephcore --lib capability:: --no-fail-fast
```
