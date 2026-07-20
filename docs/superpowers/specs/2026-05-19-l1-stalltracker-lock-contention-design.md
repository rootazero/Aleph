# L1 — `StallTracker::is_stalled` Lock Contention Fix

**Date:** 2026-05-19
**Status:** Approved (design)
**Scope:** Long-task reliability backlog item L1 (final cleanup cycle, M4/L1/L2)

## Problem

`StallTracker::is_stalled` (`src/harness/deps.rs`) is a synchronous method that
reads the last-activity timestamp through `try_lock()`:

```rust
pub fn is_stalled(&self) -> bool {
    if let Ok(guard) = self.last_activity.try_lock() {
        guard.elapsed() > self.config.timeout
    } else {
        false
    }
}
```

When `try_lock()` fails because the lock is held elsewhere, the method returns
`false` — "not stalled" — regardless of how long the agent has actually been
idle. A real stall can be silently missed.

## Investigation Findings

`StallTracker` (`src/harness/deps.rs:142-173`) guards a single `Instant`
behind `Arc<tokio::sync::Mutex<Instant>>` and exposes three methods:

| Method | Sync/async | Lock call |
|--------|-----------|-----------|
| `record_activity` | `async` | `.lock().await` |
| `elapsed` | `async` | `.lock().await` |
| `is_stalled` | **sync** | **`try_lock()`** |

`is_stalled` is the odd one out. It was made synchronous, which left it unable
to `.await` a lock — so it reached for `try_lock` and swallowed the contention
case. The sync/async inconsistency is the root cause of the bug.

**Call sites** (all inside `AgentHarness`, which runs as a single async task):

- `is_stalled()` — one production call: `src/harness/agent.rs:198` (the async
  main loop), plus two test calls in `deps.rs` `mod stall_tests`.
- `record_activity().await` — five sites: `agent.rs:236`, `act.rs:67/114/222`,
  `think.rs:256`.
- `elapsed().await` — one site: `agent.rs:199`.

Because every `StallTracker` user today runs in one async task, the lock is
never actually contended and `try_lock()` never fails — **the missed-stall bug
is latent, not currently live**. The fix is about correctness if a
`StallTracker` is ever shared across tasks, and about removing the code smell
that caused the bug.

## Approach

### Chosen: make `is_stalled` async

Change the signature and body:

```rust
pub async fn is_stalled(&self) -> bool {
    self.last_activity.lock().await.elapsed() > self.config.timeout
}
```

This makes all three `StallTracker` methods consistent — every one is `async`
and acquires the tokio `Mutex` via `.lock().await`. Under contention,
`is_stalled` now waits for the lock and returns an accurate answer instead of
reporting `false`.

### Rejected alternative: lock-free atomic timestamp

`Arc<Mutex<Instant>>` could be replaced with a lock-free `AtomicU64` holding
nanoseconds since a base `Instant`, making all three methods synchronous and
contention structurally impossible. This is arguably the cleaner end state — a
mutex around a single `Copy` value is over-engineered — but it is a larger
diff: `record_activity` and `elapsed` lose `async`, so all six of their
`.await` call sites change, and the struct gains a `base: Instant` field plus
conversion arithmetic. The backlog item is explicitly scoped as a small,
surgical change, so the minimal async fix is preferred. (If lock-free timing
is ever wanted, this remains an option.)

## Design

### Code change

`src/harness/deps.rs` — `StallTracker::is_stalled`:

- Signature: `pub fn is_stalled(&self) -> bool` → `pub async fn is_stalled(&self) -> bool`.
- Body: replace the `try_lock()` `if let … else false` with the single line
  `self.last_activity.lock().await.elapsed() > self.config.timeout`.

### Call-site ripple (3 sites)

- `src/harness/agent.rs:198` — `tracker.is_stalled()` → `tracker.is_stalled().await`.
  Already inside the async `run` loop, so this is a mechanical `.await`.
- `src/harness/deps.rs` `mod stall_tests` — the two existing tests
  (`test_not_stalled_when_recent_activity`, `test_stalled_when_timeout_exceeded`)
  call `tracker.is_stalled()`; each gets `.await`. Both are already
  `#[tokio::test]` async functions.

### Concurrency semantics

- Under contention, `is_stalled` waits for the lock rather than returning a
  wrong `false`.
- No deadlock risk: the critical section is a trivial read of one `Instant`
  and is never held across another `.await`; the lock is never acquired
  recursively (`is_stalled` does not call `record_activity`/`elapsed` or
  vice versa).
- In today's single-task model behavior is unchanged — the lock is always
  immediately available, so `.lock().await` resolves without waiting, exactly
  as `try_lock()` succeeded before.

## Scope Boundaries

- No data-structure change — `Arc<tokio::sync::Mutex<Instant>>` is kept.
- `record_activity` and `elapsed` are unchanged.
- No new `StallTracker` capabilities, no config changes.

## Architectural Compliance (R10)

The change modifies one method in `src/harness/deps.rs`, an existing harness
file — no new file, no growth of the 9-file harness budget. Stall detection is
watchdog scaffolding, not reasoning; making the method `async` does not change
that. It passes the Future-Proof Test — a stronger model has no bearing on a
lock-acquisition fix.

## Testing (TDD)

New test in `src/harness/deps.rs` `mod stall_tests`:
`is_stalled_reports_stall_even_when_lock_is_contended`.

1. **RED:** Write the test against the *current synchronous* `is_stalled`. It
   builds an already-stalled tracker (short timeout, sleep past it), spawns a
   helper task that holds the inner `last_activity` lock for a while, yields so
   the helper acquires the lock first, then asserts `is_stalled()` returns
   `true`. Against today's code `try_lock()` fails while the lock is held, so
   `is_stalled()` returns `false` and the assertion fails — a genuine
   assertion-level RED that proves the bug.
2. **GREEN:** Make `is_stalled` async; update the new test, the two existing
   stall tests, and `agent.rs:198` with `.await`. The async `is_stalled` waits
   for the helper task to release the lock, then correctly observes the stall;
   the test passes.

The two existing tests (`test_not_stalled_when_recent_activity`,
`test_stalled_when_timeout_exceeded`) continue to pass with `.await` added —
they regression-guard the unchanged stalled/not-stalled logic.

Verification: `cargo test -p alephcore --lib harness::` and the targeted
`stall` tests. Known baseline noise (see `project_baseline_test_failures.md`)
is unrelated and not a blocker.
