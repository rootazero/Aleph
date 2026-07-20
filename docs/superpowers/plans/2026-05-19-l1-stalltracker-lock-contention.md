# L1 StallTracker Lock Contention Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `StallTracker::is_stalled` acquire its lock correctly (`.lock().await`) instead of `try_lock()`, so a real stall is never silently missed under lock contention.

**Architecture:** `StallTracker` guards a single `Instant` behind `Arc<tokio::sync::Mutex<Instant>>`. `record_activity` and `elapsed` are already `async` and use `.lock().await`; `is_stalled` is the lone synchronous method and uses `try_lock()`, returning `false` ("not stalled") whenever the lock is unavailable. The fix makes `is_stalled` `async` and consistent with its siblings, then threads `.await` through its three call sites.

**Tech Stack:** Rust, `tokio::sync::Mutex`, `#[tokio::test]`, `cargo test -p alephcore`.

**Spec:** `docs/superpowers/specs/2026-05-19-l1-stalltracker-lock-contention-design.md`

**Baseline noise:** Known pre-existing unrelated test failures exist (see `project_baseline_test_failures.md`). Verify with the targeted commands below, not the full suite.

---

### Task 1: Make `StallTracker::is_stalled` async

**Files:**
- Modify: `src/harness/deps.rs` (`is_stalled` method; the two existing tests; one new test in `mod stall_tests`)
- Modify: `src/harness/agent.rs` (the one production call site)

This is a sync→async signature change. The async change and all its call-site updates must land together — code cannot compile in between — so Step 3 changes the method and every call site in one move. The TDD RED in Step 2 is a genuine assertion failure: the new test is written against the *current synchronous* signature and fails because `try_lock()` returns `false` under contention.

- [ ] **Step 1: Write the failing contention test**

In `src/harness/deps.rs`, inside the `#[cfg(test)] mod stall_tests { ... }` block (it currently ends at the file's last `}` around line 201), add this test after the existing `test_stalled_when_timeout_exceeded` test:

```rust
    #[tokio::test]
    async fn is_stalled_reports_stall_even_when_lock_is_contended() {
        let config = StallConfig {
            timeout: Duration::from_millis(10),
            check_interval: Duration::from_millis(1),
        };
        let tracker = StallTracker::new(config);

        // Become stalled: idle well past the 10ms timeout.
        tokio::time::sleep(Duration::from_millis(25)).await;

        // A second task holds the inner `last_activity` lock for a while.
        let holder = {
            let last_activity = tracker.last_activity.clone();
            tokio::spawn(async move {
                let _guard = last_activity.lock().await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            })
        };
        // Give the holder task time to acquire the lock first.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // The old `try_lock` impl returns `false` here (lock unavailable) and
        // misses the stall. The async impl waits for the lock, then correctly
        // observes the elapsed time.
        assert!(
            tracker.is_stalled(),
            "is_stalled must report a real stall even while the lock is held elsewhere",
        );

        holder.await.unwrap();
    }
```

Note: `mod stall_tests` is a child module of the module that defines `StallTracker`, so it can read `StallTracker`'s private `last_activity` field. `last_activity` is an `Arc<Mutex<Instant>>`; `.clone()` clones the `Arc`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib is_stalled_reports_stall_even_when_lock_is_contended 2>&1 | tail -20`
Expected: FAIL — the assertion `is_stalled must report a real stall even while the lock is held elsewhere` fails (the synchronous `try_lock()` returns `false` because the helper task holds the lock).

- [ ] **Step 3: Make `is_stalled` async and update every call site**

This step changes the method and all four call sites together (they must compile as one change).

**3a.** In `src/harness/deps.rs`, replace the `is_stalled` method:

```rust
    pub fn is_stalled(&self) -> bool {
        if let Ok(guard) = self.last_activity.try_lock() {
            guard.elapsed() > self.config.timeout
        } else {
            false
        }
    }
```

with:

```rust
    /// Returns `true` when no activity has been recorded within the
    /// configured timeout. Async because it acquires the same
    /// `tokio::sync::Mutex` as `record_activity` / `elapsed`: under lock
    /// contention it waits for the lock rather than reporting a false
    /// "not stalled".
    pub async fn is_stalled(&self) -> bool {
        self.last_activity.lock().await.elapsed() > self.config.timeout
    }
```

**3b.** In `src/harness/deps.rs` `mod stall_tests`, the test `test_not_stalled_when_recent_activity` has the line:

```rust
        assert!(!tracker.is_stalled());
```

Change it to:

```rust
        assert!(!tracker.is_stalled().await);
```

**3c.** In `src/harness/deps.rs` `mod stall_tests`, the test `test_stalled_when_timeout_exceeded` has the line:

```rust
        assert!(tracker.is_stalled());
```

Change it to:

```rust
        assert!(tracker.is_stalled().await);
```

**3d.** In `src/harness/deps.rs` `mod stall_tests`, the new test from Step 1 has the line:

```rust
        assert!(
            tracker.is_stalled(),
            "is_stalled must report a real stall even while the lock is held elsewhere",
        );
```

Change it to:

```rust
        assert!(
            tracker.is_stalled().await,
            "is_stalled must report a real stall even while the lock is held elsewhere",
        );
```

**3e.** In `src/harness/agent.rs`, the production call site (around line 198, inside `if let Some(ref tracker) = self.stall_tracker {`) has:

```rust
                if tracker.is_stalled() {
```

Change it to:

```rust
                if tracker.is_stalled().await {
```

This is already inside the async `run` loop, so `.await` is valid here.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib is_stalled_reports_stall_even_when_lock_is_contended test_not_stalled_when_recent_activity test_stalled_when_timeout_exceeded 2>&1 | tail -20`
Expected: PASS — all 3 stall tests pass. The contention test now passes because async `is_stalled` waits for the helper task to release the lock and then observes the real elapsed time.

- [ ] **Step 5: Run the harness suite and lint for regressions**

Run: `cargo test -p alephcore --lib harness:: 2>&1 | tail -20`
Expected: PASS — no regressions (the `is_stalled` call site in `agent.rs` compiles and the stall watchdog still works).

Run: `cargo clippy -p alephcore --lib 2>&1 | tail -15`
Expected: no new warnings introduced by this change (pre-existing unrelated warnings may remain).

- [ ] **Step 6: Commit**

```bash
git add src/harness/deps.rs src/harness/agent.rs
git commit -m "harness: make StallTracker::is_stalled async to fix lock-contention miss"
```

NOTE on commit: this repo uses `<scope>: <description>` commit messages with NO attribution trailer (no "Co-Authored-By", no "Generated with"). Use exactly the message specified.

---

## Final Verification

- [ ] **Confirm `try_lock` is gone from `StallTracker`:**

```bash
grep -n "try_lock" src/harness/deps.rs
```

Expected: no output — `is_stalled` no longer uses `try_lock`.

- [ ] **Confirm all `is_stalled` calls are awaited:**

```bash
grep -rn "is_stalled" src/harness/
```

Expected: every call site (`agent.rs` and the three tests in `deps.rs`) reads `is_stalled().await`; the definition reads `pub async fn is_stalled`.
