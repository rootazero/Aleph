# `/loop` Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a lightweight, in-session, timer-driven `/loop` command that re-runs a fixed prompt on a cadence (fixed interval or model-paced), mirroring `goal`'s continuation machinery but gated by a clock instead of a completion condition.

**Architecture:** A new in-memory `looping` subsystem (no `tasks.db` persistence — dies on daemon restart = "随会话消亡") holds per-session `LoopState`. After each run completes, a new branch in `execute.rs` (beside the existing goal-continuation hook) checks the session's loop, and if it should fire, re-injects the loop's prompt via the existing `spawn_continuation_run` — extended with a `delay_ms` param so the next tick waits for the cadence. Model-paced cadence is set by the model calling `loop(action='update', next_wake='8m')` (R8 tool call, no free-text parsing).

**Tech Stack:** Rust, Tokio, `once_cell::sync::OnceCell`, `serde`/`schemars`, `rusqlite` (NOT used here — in-memory only), existing `AlephTool` trait + `ContinuationDeps`.

**Branch / worktree:** `loop-command` at `/Volumes/TBU4/Workspace/Aleph-wt-loop` (already created from `main` HEAD `3b1b19d4e`). All work happens here. Do NOT touch `main`.

**Build constraint (project rule):** Per the task protocol AND the global cargo-lock rule, do NOT run `cargo check`/`test`/`clippy`/`build` during this task — only ONE cargo process may run machine-wide and the user asked to skip verification and commit directly. Each task's "verify" step is satisfied by careful inspection, not compilation. Commit after each task.

**Reference patterns (read before starting):**
- `src/goal/types.rs` — immutable `with_*` builder pattern + serde-compat tests (mirror for `LoopState`).
- `src/goal/mod.rs` — `OnceCell<Arc<...>>` global singleton (mirror, but in-memory backend).
- `src/tasks/goal_pursuit.rs` — `should_continue`/`exhausted_while_active`/`cap_reached_note`/`deadline_reached_note`/`continuation_prompt` (mirror for loop pursuit).
- `src/builtin_tools/goal.rs` — `GoalTool`/`GoalArgs`/`GoalAction` + `with_session_key_handle` (mirror for `LoopTool`).
- `src/gateway/execution_engine/execute.rs:619-766` — goal continuation hook (add loop branch beside it); `:868` `spawn_continuation_run` (add `delay_ms`).
- `src/executor/builtin_registry/builder/constructor.rs:240-254` (goal store boot) + `:1721` (session-key handle wiring).
- `src/executor/builtin_registry/definitions.rs:1044` (goal tool registration by name).

---

## File Structure

**Create:**
- `src/looping/mod.rs` — in-memory `LoopRegistry` + process-global `OnceCell` (`init_global`/`global`).
- `src/looping/types.rs` — `LoopState`, `Cadence`, `LoopStatus` + immutable builders.
- `src/looping/pursuit.rs` — pure gate functions: `should_fire`, `exhausted`, `tick_delay_ms`, `tick_prompt`, `cap_reached_note`, `deadline_reached_note`.
- `src/builtin_tools/loop_manage.rs` — `LoopTool` (registered name `loop`), `LoopArgs`, `LoopAction`, interval parser.

**Modify:**
- `src/lib.rs` — `pub mod looping;`
- `src/builtin_tools/mod.rs` — `pub mod loop_manage;` + re-export.
- `src/executor/builtin_registry/definitions.rs` — register `"loop"` by name.
- `src/executor/builtin_registry/builder/constructor.rs` — `looping::init_global` at boot + wire `LoopTool` with session-key handle.
- `src/gateway/execution_engine/execute.rs` — add `delay_ms` to `spawn_continuation_run` (+ update 2 goal callsites) + add loop continuation branch.

---

## Task 1: `looping/types.rs` — LoopState data model

**Files:**
- Create: `src/looping/types.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/looping/types.rs` with the test module first (types referenced will not exist yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LoopState {
        LoopState::new(
            "sess-1",
            "check deploy status",
            Cadence::Fixed { interval_ms: 300_000 },
            1_000,
        )
    }

    #[test]
    fn new_loop_is_active_with_zero_iterations() {
        let l = sample();
        assert_eq!(l.status, LoopStatus::Active);
        assert_eq!(l.iterations_used, 0);
        assert_eq!(l.session_id, "sess-1");
        assert_eq!(l.created_at_ms, 1_000);
        assert!(l.max_iterations.is_none());
        assert!(l.deadline_ms.is_none());
        assert!(l.next_wake_ms.is_none());
    }

    #[test]
    fn spent_iteration_increments_and_returns_new_copy() {
        let l = sample();
        let after = l.clone().spent_iteration();
        assert_eq!(after.iterations_used, 1);
        assert_eq!(l.iterations_used, 0, "original unchanged");
    }

    #[test]
    fn with_status_returns_new_copy() {
        let l = sample();
        let stopped = l.clone().with_status(LoopStatus::Stopped);
        assert_eq!(stopped.status, LoopStatus::Stopped);
        assert_eq!(l.status, LoopStatus::Active, "original unchanged");
    }

    #[test]
    fn with_caps_sets_optional_bounds() {
        let l = sample().with_max_iterations(Some(20)).with_deadline_ms(Some(99_999));
        assert_eq!(l.max_iterations, Some(20));
        assert_eq!(l.deadline_ms, Some(99_999));
    }

    #[test]
    fn with_next_wake_sets_model_paced_delay() {
        let l = LoopState::new("s", "p", Cadence::ModelPaced { fallback_ms: 600_000 }, 0)
            .with_next_wake_ms(Some(480_000));
        assert_eq!(l.next_wake_ms, Some(480_000));
    }

    #[test]
    fn serde_roundtrip_preserves_state() {
        let l = sample().with_max_iterations(Some(5));
        let json = serde_json::to_string(&l).unwrap();
        let back: LoopState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.iterations_used, l.iterations_used);
        assert_eq!(back.max_iterations, Some(5));
    }
}
```

- [ ] **Step 2: Verify the test would fail**

Inspect: the `super::*` types (`LoopState`, `Cadence`, `LoopStatus`) are not yet defined above the test module — this is the RED state. (No `cargo test` per build constraint.)

- [ ] **Step 3: Write the implementation**

Prepend to `src/looping/types.rs` (above the test module):

```rust
//! In-memory loop state: a per-session timer-driven repeat. Mirrors
//! `goal::types` but is NEVER persisted — the registry lives in process
//! memory only, so a daemon restart clears all loops ("随会话消亡").
//!
//! Distinct from `goal` (condition-stop) and `cron` (persistent, own session):
//! a loop is "current session, multi-turn, clock-gated, never self-stops".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How the loop picks the delay before its next tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cadence {
    /// Fixed interval: wait this many ms between ticks.
    Fixed { interval_ms: u64 },
    /// Model-paced: the model sets `next_wake_ms` each tick via
    /// `loop(action='update', next_wake=...)`. If unset, use `fallback_ms`.
    ModelPaced { fallback_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopStatus {
    Active,
    Stopped,
}

/// One session's loop. Immutable-update style: `with_*` / `spent_*` return
/// new copies, never mutate in place (project immutability rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoopState {
    pub session_id: String,
    /// The fixed prompt re-injected verbatim each tick.
    pub prompt: String,
    pub cadence: Cadence,
    /// Model-paced override (absolute epoch ms of the next wake). `None` →
    /// fall back to the cadence default. For `Fixed` cadence this is ignored.
    #[serde(default)]
    pub next_wake_ms: Option<u64>,
    pub iterations_used: u32,
    /// Optional safety cap: stop after this many ticks. `None` → unbounded.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Optional safety cap: absolute epoch-ms wall-clock deadline.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    /// Optional soft token budget (carried for parity with goal; the
    /// continuation hook passes 0 tokens today, same as goal, so this is
    /// advisory until token accounting is wired through the hook).
    #[serde(default)]
    pub token_budget: Option<u64>,
    pub status: LoopStatus,
    pub created_at_ms: u64,
}

impl LoopState {
    #[must_use]
    pub fn new(session_id: &str, prompt: &str, cadence: Cadence, now_ms: u64) -> Self {
        Self {
            session_id: session_id.to_string(),
            prompt: prompt.to_string(),
            cadence,
            next_wake_ms: None,
            iterations_used: 0,
            max_iterations: None,
            deadline_ms: None,
            token_budget: None,
            status: LoopStatus::Active,
            created_at_ms: now_ms,
        }
    }

    #[must_use]
    pub fn spent_iteration(mut self) -> Self {
        self.iterations_used = self.iterations_used.saturating_add(1);
        self
    }

    #[must_use]
    pub fn with_status(mut self, status: LoopStatus) -> Self {
        self.status = status;
        self
    }

    #[must_use]
    pub const fn with_max_iterations(mut self, max: Option<u32>) -> Self {
        self.max_iterations = max;
        self
    }

    #[must_use]
    pub const fn with_deadline_ms(mut self, deadline_ms: Option<u64>) -> Self {
        self.deadline_ms = deadline_ms;
        self
    }

    #[must_use]
    pub const fn with_token_budget(mut self, budget: Option<u64>) -> Self {
        self.token_budget = budget;
        self
    }

    #[must_use]
    pub const fn with_next_wake_ms(mut self, next_wake_ms: Option<u64>) -> Self {
        self.next_wake_ms = next_wake_ms;
        self
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == LoopStatus::Active
    }
}
```

> Note: `with_status` and `new` allocate `String`, so they are not `const fn`. The cap setters and `with_next_wake_ms` only assign `Copy`/`Option` fields → keep them `const fn` (matches goal's style and avoids clippy `missing_const_for_fn`).

- [ ] **Step 4: Verify by inspection**

Confirm every type/method referenced in the Step-1 tests now exists with matching names and signatures. Confirm no `let mut` outside the `with_*`/`spent_*` self-rebind pattern.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/looping/types.rs
git commit -m "feat(looping): add LoopState in-memory data model"
```

---

## Task 2: `looping/mod.rs` — in-memory registry + global singleton

**Files:**
- Create: `src/looping/mod.rs`

- [ ] **Step 1: Write the failing tests**

Put this test module at the bottom of `src/looping/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::looping::types::{Cadence, LoopState, LoopStatus};

    fn st(sess: &str) -> LoopState {
        LoopState::new(sess, "p", Cadence::Fixed { interval_ms: 1000 }, 0)
    }

    #[test]
    fn put_then_get_returns_state() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        assert_eq!(reg.get("a").unwrap().session_id, "a");
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn put_overwrites_same_session() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        reg.put(st("a").spent_iteration());
        assert_eq!(reg.get("a").unwrap().iterations_used, 1);
    }

    #[test]
    fn remove_deletes_state() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        reg.remove("a");
        assert!(reg.get("a").is_none());
    }

    #[test]
    fn get_active_filters_stopped() {
        let reg = LoopRegistry::default();
        reg.put(st("a").with_status(LoopStatus::Stopped));
        assert!(reg.get("a").is_some(), "get returns regardless of status");
        assert!(reg.get_active("a").is_none(), "get_active skips Stopped");
    }
}
```

- [ ] **Step 2: Verify the test would fail**

Inspect: `LoopRegistry` and the `looping` module are not yet declared. RED state confirmed by absence.

- [ ] **Step 3: Write the implementation**

Prepend to `src/looping/mod.rs`:

```rust
//! Loop subsystem: a per-session, timer-driven repeat managed by the LLM via
//! the `loop` tool (R8) and re-fired each turn by the execution engine's
//! continuation hook. The clock-gated sibling of `goal` (condition-gated).
//!
//! Storage is PROCESS MEMORY ONLY — never `tasks.db`. A daemon restart clears
//! every loop, which is the physical guarantee of the "随会话消亡" semantics.

pub mod pursuit;
pub mod types;

pub use types::{Cadence, LoopState, LoopStatus};

use crate::sync_primitives::Arc;
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory map of `session_key` → `LoopState`.
#[derive(Default)]
pub struct LoopRegistry {
    inner: Mutex<HashMap<String, LoopState>>,
}

impl LoopRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, LoopState>> {
        // Poison-safe (project rule P7): recover the guard rather than panic.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert or overwrite the loop for a session.
    pub fn put(&self, state: LoopState) {
        self.lock().insert(state.session_id.clone(), state);
    }

    /// Read the loop for a session regardless of status.
    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<LoopState> {
        self.lock().get(session_id).cloned()
    }

    /// Read the loop only if it is `Active`.
    #[must_use]
    pub fn get_active(&self, session_id: &str) -> Option<LoopState> {
        self.lock()
            .get(session_id)
            .filter(|l| l.is_active())
            .cloned()
    }

    /// Drop the loop for a session.
    pub fn remove(&self, session_id: &str) {
        self.lock().remove(session_id);
    }
}

/// Process-global registry. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as "no
/// loop subsystem" and the continuation hook stays dormant.
static GLOBAL: OnceCell<Arc<LoopRegistry>> = OnceCell::new();

/// Install the global registry at boot. Idempotent.
pub fn init_global(registry: Arc<LoopRegistry>) {
    let _ = GLOBAL.set(registry);
}

/// Read the global registry, if initialized.
#[must_use]
pub fn global() -> Option<Arc<LoopRegistry>> {
    GLOBAL.get().cloned()
}

/// Test-only override.
#[cfg(test)]
pub fn set_global_for_test(registry: Arc<LoopRegistry>) {
    let _ = GLOBAL.set(registry);
}
```

> Check `crate::sync_primitives::Arc` exists (it does — `goal/mod.rs` uses it). If `sync_primitives` does not re-export `Arc`, use `std::sync::Arc` instead — confirm by reading `src/goal/mod.rs:10` which imports `use crate::sync_primitives::Arc;`.

- [ ] **Step 4: Verify by inspection**

Confirm `LoopRegistry`, `put`/`get`/`get_active`/`remove`, `init_global`/`global` exist and match the test calls. Confirm `pursuit` and `types` submodules are declared.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/looping/mod.rs
git commit -m "feat(looping): add in-memory LoopRegistry + global singleton"
```

---

## Task 3: `looping/pursuit.rs` — pure gate functions

**Files:**
- Create: `src/looping/pursuit.rs`

- [ ] **Step 1: Write the failing tests**

Put at the bottom of `src/looping/pursuit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::looping::types::{Cadence, LoopState, LoopStatus};

    fn fixed() -> LoopState {
        LoopState::new("s", "check CI", Cadence::Fixed { interval_ms: 300_000 }, 1_000)
    }

    #[test]
    fn active_uncapped_loop_fires() {
        assert!(should_fire(&fixed(), 0, 2_000));
    }

    #[test]
    fn stopped_loop_never_fires() {
        let l = fixed().with_status(LoopStatus::Stopped);
        assert!(!should_fire(&l, 0, 2_000));
    }

    #[test]
    fn loop_at_max_iterations_does_not_fire() {
        let mut l = fixed().with_max_iterations(Some(2));
        l = l.spent_iteration().spent_iteration(); // used == 2
        assert!(!should_fire(&l, 0, 2_000));
        assert!(exhausted(&l, 0, 2_000));
    }

    #[test]
    fn loop_below_max_iterations_fires() {
        let l = fixed().with_max_iterations(Some(2)).spent_iteration(); // used == 1
        assert!(should_fire(&l, 0, 2_000));
        assert!(!exhausted(&l, 0, 2_000));
    }

    #[test]
    fn past_deadline_does_not_fire() {
        let l = fixed().with_deadline_ms(Some(5_000));
        assert!(!should_fire(&l, 0, 6_000));
        assert!(exhausted(&l, 0, 6_000));
    }

    #[test]
    fn before_deadline_fires() {
        let l = fixed().with_deadline_ms(Some(10_000));
        assert!(should_fire(&l, 0, 6_000));
    }

    #[test]
    fn now_ms_zero_never_trips_deadline() {
        // now_ms == 0 means "clock unavailable" — must not falsely exhaust.
        let l = fixed().with_deadline_ms(Some(5_000));
        assert!(should_fire(&l, 0, 0));
    }

    #[test]
    fn fixed_cadence_delay_is_interval() {
        assert_eq!(tick_delay_ms(&fixed(), 2_000), 300_000);
    }

    #[test]
    fn model_paced_uses_next_wake_relative_to_now() {
        let l = LoopState::new("s", "p", Cadence::ModelPaced { fallback_ms: 600_000 }, 0)
            .with_next_wake_ms(Some(10_000));
        // next wake at epoch 10_000, now 4_000 → 6_000 ms from now.
        assert_eq!(tick_delay_ms(&l, 4_000), 6_000);
    }

    #[test]
    fn model_paced_past_next_wake_clamps_to_zero() {
        let l = LoopState::new("s", "p", Cadence::ModelPaced { fallback_ms: 600_000 }, 0)
            .with_next_wake_ms(Some(3_000));
        assert_eq!(tick_delay_ms(&l, 9_000), 0);
    }

    #[test]
    fn model_paced_without_next_wake_uses_fallback() {
        let l = LoopState::new("s", "p", Cadence::ModelPaced { fallback_ms: 600_000 }, 0);
        assert_eq!(tick_delay_ms(&l, 9_000), 600_000);
    }

    #[test]
    fn tick_prompt_restates_prompt_and_controls() {
        let p = tick_prompt(&fixed());
        assert!(p.contains("check CI"));
        assert!(p.contains("action='stop'"));
    }

    #[test]
    fn notes_mention_reason() {
        assert!(cap_reached_note(&fixed().with_max_iterations(Some(2))).contains("iteration"));
        assert!(deadline_reached_note(&fixed()).contains("time"));
    }
}
```

- [ ] **Step 2: Verify the test would fail**

Inspect: none of `should_fire`/`exhausted`/`tick_delay_ms`/`tick_prompt`/`cap_reached_note`/`deadline_reached_note` exist yet. RED confirmed.

- [ ] **Step 3: Write the implementation**

Prepend to `src/looping/pursuit.rs`:

```rust
//! Pure gate functions for the loop continuation hook. No I/O, no `LoopState`
//! mutation — the execution engine calls these to decide whether to re-fire a
//! loop and how long to wait. Mirrors `tasks::goal_pursuit` but clock-gated.

use crate::looping::types::{Cadence, LoopState};

/// Has any safety cap been reached? `tokens_now` is accepted for parity with
/// goal but the hook passes 0 today (token accounting not wired through).
#[must_use]
pub fn exhausted(state: &LoopState, _tokens_now: u64, now_ms: u64) -> bool {
    if let Some(max) = state.max_iterations {
        if state.iterations_used >= max {
            return true;
        }
    }
    if let Some(deadline) = state.deadline_ms {
        // now_ms == 0 means clock unavailable — never trip on it.
        if now_ms != 0 && now_ms > deadline {
            return true;
        }
    }
    false
}

/// Should this loop fire another tick now? Active and no cap reached.
#[must_use]
pub fn should_fire(state: &LoopState, tokens_now: u64, now_ms: u64) -> bool {
    state.is_active() && !exhausted(state, tokens_now, now_ms)
}

/// How long to wait before the next tick, in ms.
#[must_use]
pub fn tick_delay_ms(state: &LoopState, now_ms: u64) -> u64 {
    match &state.cadence {
        Cadence::Fixed { interval_ms } => *interval_ms,
        Cadence::ModelPaced { fallback_ms } => match state.next_wake_ms {
            Some(wake) => wake.saturating_sub(now_ms),
            None => *fallback_ms,
        },
    }
}

/// The prompt re-injected each tick: the user's fixed prompt plus a one-line
/// reminder of the loop controls (so the model can re-pace or stop). Kept tiny
/// — the intelligence lives in the model, not this scaffold (R9).
#[must_use]
pub fn tick_prompt(state: &LoopState) -> String {
    let n = state.iterations_used.saturating_add(1);
    let pace = match state.cadence {
        Cadence::ModelPaced { .. } => {
            " To change the cadence, call loop(action='update', next_wake='8m')."
        }
        Cadence::Fixed { .. } => "",
    };
    format!(
        "{prompt}\n\n[Timer loop — tick {n}. This runs on a schedule and will \
         not self-stop. To end it, call loop(action='stop').{pace}]",
        prompt = state.prompt,
    )
}

/// User-facing reason when the iteration cap stops the loop.
#[must_use]
pub fn cap_reached_note(state: &LoopState) -> String {
    format!(
        "Loop stopped: reached the iteration cap ({} ticks).",
        state.max_iterations.unwrap_or(0)
    )
}

/// User-facing reason when the wall-clock deadline stops the loop.
#[must_use]
pub fn deadline_reached_note(_state: &LoopState) -> String {
    "Loop stopped: reached its time limit.".to_string()
}
```

- [ ] **Step 4: Verify by inspection**

Confirm each tested function name + signature matches. Confirm `now_ms == 0` guard present in `exhausted`. Confirm `saturating_sub` in `tick_delay_ms` (no underflow panic).

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/looping/pursuit.rs
git commit -m "feat(looping): add pure loop-pursuit gate functions"
```

---

## Task 4: Register `looping` module in lib.rs

**Files:**
- Modify: `src/lib.rs`

- [ ] **Step 1: Locate the goal module declaration**

Inspect `src/lib.rs` and find the line `pub mod goal;` (the module list is alphabetical-ish; place `looping` near it).

- [ ] **Step 2: Add the module declaration**

Add after the `pub mod goal;` line (or in the appropriate alphabetical slot):

```rust
pub mod looping;
```

- [ ] **Step 3: Verify by inspection**

Confirm `src/lib.rs` now declares `pub mod looping;` exactly once and that `src/looping/mod.rs` exists.

- [ ] **Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/lib.rs
git commit -m "feat(looping): register looping module in lib.rs"
```

---

## Task 5: `builtin_tools/loop_manage.rs` — the `loop` tool

**Files:**
- Create: `src/builtin_tools/loop_manage.rs`

This task has two parts: a no-dep interval parser, then the tool itself.

- [ ] **Step 1: Write the failing tests (parser + tool)**

Put at the bottom of `src/builtin_tools/loop_manage.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interval_handles_units() {
        assert_eq!(parse_interval_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_interval_ms("5m").unwrap(), 300_000);
        assert_eq!(parse_interval_ms("2h").unwrap(), 7_200_000);
        assert_eq!(parse_interval_ms("500ms").unwrap(), 500);
    }

    #[test]
    fn parse_interval_rejects_garbage() {
        assert!(parse_interval_ms("soon").is_err());
        assert!(parse_interval_ms("").is_err());
        assert!(parse_interval_ms("5x").is_err());
    }

    #[test]
    fn parse_interval_rejects_sub_second_fixed() {
        // sub-second intervals would hammer the loop — reject below 1000ms
        // (mirrors cron_manage every_ms < 1000 guard).
        assert!(parse_interval_ms("100ms").is_err());
    }

    #[tokio::test]
    async fn start_with_interval_registers_active_fixed_loop() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("sess-x");
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Start,
                interval: Some("5m".to_string()),
                prompt: Some("check deploy".to_string()),
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        let st = reg.get("sess-x").unwrap();
        assert!(st.is_active());
        assert_eq!(st.prompt, "check deploy");
        assert!(matches!(st.cadence, crate::looping::Cadence::Fixed { interval_ms: 300_000 }));
    }

    #[tokio::test]
    async fn start_without_interval_is_model_paced() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Start,
            interval: None,
            prompt: Some("watch CI".to_string()),
            max_iterations: Some(20),
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        let st = reg.get("s").unwrap();
        assert!(matches!(st.cadence, crate::looping::Cadence::ModelPaced { .. }));
        assert_eq!(st.max_iterations, Some(20));
    }

    #[tokio::test]
    async fn stop_marks_loop_stopped() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s", "p", crate::looping::Cadence::Fixed { interval_ms: 1000 }, 0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Stop,
            interval: None, prompt: None, max_iterations: None,
            timeout_minutes: None, token_budget: None, next_wake: None,
        })
        .await
        .unwrap();
        assert!(!reg.get("s").unwrap().is_active());
    }

    #[tokio::test]
    async fn update_sets_next_wake_for_model_paced() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s", "p", crate::looping::Cadence::ModelPaced { fallback_ms: 600_000 }, 0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Update,
            interval: None, prompt: None, max_iterations: None,
            timeout_minutes: None, token_budget: None,
            next_wake: Some("8m".to_string()),
        })
        .await
        .unwrap();
        // next_wake stored as an absolute epoch-ms; just assert it is now set.
        assert!(reg.get("s").unwrap().next_wake_ms.is_some());
    }
}
```

- [ ] **Step 2: Verify the test would fail**

Inspect: `parse_interval_ms`, `LoopTool`, `LoopArgs`, `LoopAction`, `with_session_for_test` do not exist. RED confirmed.

- [ ] **Step 3: Write the implementation**

Prepend to `src/builtin_tools/loop_manage.rs`. Mirror `src/builtin_tools/goal.rs` for the `AlephTool` impl shape (read it first for the exact trait method names — `name`, `description`, `examples`, `run`, and the args/output associated types). Adjust the trait impl below to match `goal.rs` exactly if any method name differs.

```rust
//! `loop` builtin tool (R8): the LLM starts/stops/paces an in-session timer
//! loop in natural language. The clock-gated sibling of `goal`.
//!
//! Registered under the name `loop`, so `/loop ...` resolves to it via the
//! command parser. The actual re-firing happens in the execution engine's
//! continuation hook (see `gateway::execution_engine::execute`).

use crate::looping::{Cadence, LoopRegistry, LoopState, LoopStatus};
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopAction {
    /// Begin a timer loop in this session.
    Start,
    /// Stop the session's loop (the only way it ends, absent a safety cap).
    Stop,
    /// Read the current loop: cadence, ticks used, caps, next wake.
    Status,
    /// Re-pace a model-paced loop (`next_wake`) or adjust caps.
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LoopArgs {
    pub action: LoopAction,
    /// Fixed cadence, human form: "30s" / "5m" / "2h". Omit on `start` to use
    /// model-paced cadence (you set the next wake each tick via `update`).
    pub interval: Option<String>,
    /// The prompt re-run each tick — required for `start`.
    pub prompt: Option<String>,
    /// Optional safety cap: stop after this many ticks.
    pub max_iterations: Option<u32>,
    /// Optional safety cap: wall-clock minutes from now.
    pub timeout_minutes: Option<u32>,
    /// Optional soft token budget.
    pub token_budget: Option<u64>,
    /// For `update` on a model-paced loop: when to wake next, human form
    /// ("8m"). Stored as an absolute deadline (now + delta).
    pub next_wake: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopOutput {
    pub success: bool,
    pub message: String,
}

/// Default model-paced fallback when the model never sets `next_wake`.
const MODEL_PACED_FALLBACK_MS: u64 = 600_000; // 10 min

/// Parse a human duration ("30s","5m","2h","500ms") into ms. Rejects garbage
/// and sub-second values (a sub-second loop would hammer the engine). No new
/// dependency — small hand parser (R3 core minimalism).
pub fn parse_interval_ms(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty interval".to_string());
    }
    let (num, unit_ms): (&str, u64) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        return Err(format!("unrecognized interval '{s}' (use 30s/5m/2h)"));
    };
    let value: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid interval number in '{s}'"))?;
    let ms = value.saturating_mul(unit_ms);
    if ms < 1_000 {
        return Err(format!("interval too short: '{s}' is below the 1s minimum"));
    }
    Ok(ms)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub struct LoopTool {
    registry: Arc<LoopRegistry>,
    session_key: Option<Arc<RwLock<String>>>,
    #[cfg(test)]
    test_session: Option<String>,
}

impl LoopTool {
    #[must_use]
    pub fn new(registry: Arc<LoopRegistry>) -> Self {
        Self {
            registry,
            session_key: None,
            #[cfg(test)]
            test_session: None,
        }
    }

    #[must_use]
    pub fn with_session_key_handle(mut self, handle: Option<Arc<RwLock<String>>>) -> Self {
        self.session_key = handle;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_session_for_test(mut self, sess: &str) -> Self {
        self.test_session = Some(sess.to_string());
        self
    }

    async fn session(&self) -> String {
        #[cfg(test)]
        if let Some(s) = &self.test_session {
            return s.clone();
        }
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }

    /// Core dispatch — public so tests call it directly without the trait.
    pub async fn run(&self, args: LoopArgs) -> Result<LoopOutput, String> {
        let session = self.session().await;
        info!(session = %session, action = ?args.action, "loop operation");
        match args.action {
            LoopAction::Start => self.start(&session, args),
            LoopAction::Stop => self.stop(&session),
            LoopAction::Status => self.status(&session),
            LoopAction::Update => self.update(&session, args),
        }
    }

    fn start(&self, session: &str, args: LoopArgs) -> Result<LoopOutput, String> {
        let prompt = args
            .prompt
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| "start requires a non-empty prompt".to_string())?;
        let cadence = match &args.interval {
            Some(i) => Cadence::Fixed {
                interval_ms: parse_interval_ms(i)?,
            },
            None => Cadence::ModelPaced {
                fallback_ms: MODEL_PACED_FALLBACK_MS,
            },
        };
        let now = now_ms();
        let deadline = args
            .timeout_minutes
            .map(|m| now.saturating_add(u64::from(m).saturating_mul(60_000)));
        let state = LoopState::new(session, &prompt, cadence, now)
            .with_max_iterations(args.max_iterations)
            .with_deadline_ms(deadline)
            .with_token_budget(args.token_budget);
        self.registry.put(state);
        Ok(LoopOutput {
            success: true,
            message: format!(
                "Loop started in this session. It will re-run every tick and \
                 will not self-stop — call loop(action='stop') to end it."
            ),
        })
    }

    fn stop(&self, session: &str) -> Result<LoopOutput, String> {
        match self.registry.get(session) {
            Some(state) => {
                self.registry.put(state.with_status(LoopStatus::Stopped));
                Ok(LoopOutput { success: true, message: "Loop stopped.".to_string() })
            }
            None => Ok(LoopOutput {
                success: false,
                message: "No loop in this session.".to_string(),
            }),
        }
    }

    fn status(&self, session: &str) -> Result<LoopOutput, String> {
        match self.registry.get(session) {
            Some(s) => Ok(LoopOutput {
                success: true,
                message: format!(
                    "Loop: status={:?}, ticks_used={}, cadence={:?}, \
                     max_iterations={:?}, deadline_ms={:?}",
                    s.status, s.iterations_used, s.cadence, s.max_iterations, s.deadline_ms
                ),
            }),
            None => Ok(LoopOutput {
                success: false,
                message: "No loop in this session.".to_string(),
            }),
        }
    }

    fn update(&self, session: &str, args: LoopArgs) -> Result<LoopOutput, String> {
        let Some(mut state) = self.registry.get(session) else {
            return Ok(LoopOutput {
                success: false,
                message: "No loop in this session.".to_string(),
            });
        };
        if let Some(nw) = &args.next_wake {
            let delta = parse_interval_ms(nw)?;
            state = state.with_next_wake_ms(Some(now_ms().saturating_add(delta)));
        }
        if args.max_iterations.is_some() {
            state = state.with_max_iterations(args.max_iterations);
        }
        self.registry.put(state);
        Ok(LoopOutput { success: true, message: "Loop updated.".to_string() })
    }
}
```

- [ ] **Step 4: Add the `AlephTool` trait impl**

Read `src/builtin_tools/goal.rs:148` (`impl AlephTool for GoalTool`) for the EXACT trait signature in this codebase (associated `Args`/`Output` types, `name()`, `description()`, `examples()`, and the `execute`/`run` method name). Then append a matching impl for `LoopTool` that delegates to `self.run(args)`. Use these strings:

- `name()` → `"loop"`
- `description()` → a paragraph contrasting with goal: *"Start a timer loop that re-runs a prompt on a schedule in THIS session. Unlike `goal` (which stops when a condition is met), a loop runs to a clock and never self-stops — end it with action='stop'. Use action='start' with `interval` (e.g. '5m') for a fixed cadence, or omit `interval` for model-paced (call action='update' with `next_wake` each tick to set the next delay). Optional safety caps: max_iterations, timeout_minutes. Use for watch/poll duties (e.g. 'every 5 minutes check the deploy and tell me if it changed')."*
- `examples()` → mirror goal's `Vec<String>` form:
  - `loop(action='start', interval='5m', prompt='Check the deploy status; tell me if it changed')`
  - `loop(action='start', prompt='Triage the PR queue', max_iterations=20)` (model-paced)
  - `loop(action='update', next_wake='8m')`
  - `loop(action='status')`
  - `loop(action='stop')`

If `goal.rs`'s trait uses a JSON-args entry point (e.g. `execute(&self, args_json) -> ...` that deserializes `GoalArgs`), follow that exact pattern: deserialize `LoopArgs`, call `self.run(args).await`, serialize `LoopOutput`. Match goal's error mapping.

- [ ] **Step 5: Verify by inspection**

Confirm `parse_interval_ms`, `LoopTool::{new,with_session_key_handle,with_session_for_test,run,start,stop,status,update}`, `LoopArgs`, `LoopAction`, `LoopOutput` all exist with signatures the Step-1 tests call. Confirm the `AlephTool` impl method names match `goal.rs` exactly (this is the #1 compile risk — verify against the real trait).

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/builtin_tools/loop_manage.rs
git commit -m "feat(looping): add loop builtin tool (start/stop/status/update)"
```

---

## Task 6: Register & wire the `loop` tool at boot

**Files:**
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/builder/constructor.rs`

- [ ] **Step 1: Declare + re-export the module**

In `src/builtin_tools/mod.rs`, beside `pub mod goal;` (line ~55) add:

```rust
pub mod loop_manage;
```

And beside the goal re-export (line ~164) add:

```rust
pub use loop_manage::{LoopAction, LoopArgs, LoopOutput, LoopTool};
```

- [ ] **Step 2: Register the tool by name in definitions.rs**

In `src/executor/builtin_registry/definitions.rs`, right after the `"goal" => ...` arm (line ~1044), add:

```rust
        // Loop tool — backed by the process-global in-memory LoopRegistry
        // (init_global at boot). None before boot, same as goal.
        "loop" => crate::looping::global().map(|reg| {
            Box::new(crate::builtin_tools::LoopTool::new(reg)) as Box<dyn AlephToolDyn>
        }),
```

> Also find the static list of builtin tool NAMES this file uses to advertise tools (search the file for `"goal"` — there is usually a names array / match in addition to the constructor arm). Add `"loop"` everywhere `"goal"` appears so the tool is surfaced and `/loop` resolves. If `goal` appears only in this one arm, no further edit is needed.

- [ ] **Step 3: Init the registry + wire the tool at boot**

In `src/executor/builtin_registry/builder/constructor.rs`, right after the goal block (lines 240-254, ending `let goal_tool = ...GoalTool::new(goal_store);`), add:

```rust
        // Loop subsystem — in-memory only (never tasks.db); cleared on restart.
        let loop_registry = Arc::new(crate::looping::LoopRegistry::default());
        crate::looping::init_global(loop_registry.clone());
        let loop_tool = crate::builtin_tools::LoopTool::new(loop_registry);
```

Then find the struct-literal where `goal_tool: goal_tool.with_session_key_handle(...)` is set (line ~1721) and add a sibling field if the registry builder holds tool instances. If the builder does NOT store tool instances by field (i.e. tools are built on demand in `definitions.rs`), then the boot-time `loop_tool` local is only needed to mirror goal — in that case, the `definitions.rs` arm (Step 2) already constructs the tool with the session-key handle applied downstream the same way goal's is. 

> Decision point for the implementer: read constructor.rs:1700-1725 to see whether `goal_tool` is stored as a struct field (`goal_tool: ...`). 
> - If YES → add `loop_tool: loop_tool.with_session_key_handle(memory_session_key_handle.clone()),` in the same literal, AND add a `loop_tool` field to that builder struct (find its definition, add `loop_tool: LoopTool,`), AND ensure it gets registered into the tool set the same place `goal_tool` is.
> - If the goal tool reaches the registry purely via the `definitions.rs` name-arm (Step 2) and the session-key handle is applied by a generic post-construction pass, then the name-arm is sufficient and you only keep `init_global` from this step (drop the unused `loop_tool` local to avoid a dead-binding warning).

The session-key handle is `memory_session_key_handle` (same `Arc<RwLock<String>>` goal uses at constructor.rs:1721).

- [ ] **Step 4: Verify by inspection**

Confirm: `loop_manage` declared + re-exported; `"loop"` name-arm added in definitions.rs; `looping::init_global` called once at boot; session-key handle wired the same way as goal (whichever of the two paths goal actually uses). Confirm no unused local `loop_tool` binding is left dangling.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/builtin_tools/mod.rs src/executor/builtin_registry/definitions.rs src/executor/builtin_registry/builder/constructor.rs
git commit -m "feat(looping): register and boot-wire the loop tool"
```

---

## Task 7: Extend `spawn_continuation_run` with `delay_ms` (entropy-reduction)

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs`

- [ ] **Step 1: Add the parameter to the function signature**

At `src/gateway/execution_engine/execute.rs:868`, the function:

```rust
fn spawn_continuation_run(
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    adapter: Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    session_key: crate::routing::session_key::SessionKey,
    session_key_str: String,
    prompt: String,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
) {
```

Add a trailing parameter:

```rust
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    delay_ms: Option<u64>,
) {
```

- [ ] **Step 2: Sleep before execute inside the spawned task**

Inside the `tokio::spawn(async move { ... })` block (starts at line ~897), as the FIRST statement after the `async move {` and before `let Some(cont_agent) = registry.get(...)`:

```rust
    tokio::spawn(async move {
        // Loop cadence: wait the requested delay before this tick fires. Goal
        // continuations pass None (immediate); loop ticks pass Some(interval).
        if let Some(d) = delay_ms {
            tokio::time::sleep(std::time::Duration::from_millis(d)).await;
        }
        let Some(cont_agent) = registry.get(&cont_agent_id).await else {
```

> Verify `cont_agent_id` is computed BEFORE the spawn (it is — line ~896 `let cont_agent_id = session_key.agent_id().to_string();` sits before `tokio::spawn`). The sleep must be inside the spawned task so the caller (post-run hook) returns immediately.

- [ ] **Step 3: Update the two existing goal callsites to pass `None`**

At execute.rs:702 (gate-failure continuation) and execute.rs:727 (should_continue continuation), both currently end with `cont_deps.event_bus.clone(),` as the last arg. Add `None,` after it in each:

```rust
                                                    spawn_continuation_run(
                                                        cont_deps.registry.clone(),
                                                        cont_deps.adapter.clone(),
                                                        request.session_key.clone(),
                                                        session_key_str.clone(),
                                                        prompt,
                                                        cont_deps.event_bus.clone(),
                                                        None,
                                                    );
```

Apply the same `None,` addition to BOTH callsites. Goal behavior is unchanged (immediate continuation).

- [ ] **Step 4: Verify by inspection**

Confirm the function has the new `delay_ms: Option<u64>` param, the sleep is the first statement inside `tokio::spawn`, and BOTH goal callsites (702, 727) now pass `None` as the final argument. Grep for `spawn_continuation_run(` in the file — there must be exactly the definition + 2 goal callsites so far (the loop callsite comes in Task 8).

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/gateway/execution_engine/execute.rs
git commit -m "refactor(execution): add delay_ms to spawn_continuation_run

Converges immediate (goal) and delayed (loop) continuation paths into one
function. Goal callsites pass None — behavior unchanged."
```

---

## Task 8: Add the loop continuation branch in `execute.rs`

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs`

- [ ] **Step 1: Locate the insertion point**

The goal continuation hook lives inside `if let Some(cont_deps) = self.continuation_deps.get() { ... }` (line 624) and ends at the closing brace around line 776-777 (after the `Ok(None) => {}` / `Err(e) => {}` match arms for the goal store lookup). The loop branch goes immediately AFTER the goal block's closing brace, still INSIDE the `if let Some(cont_deps)` block, so it reuses `cont_deps`.

Read lines 766-778 to confirm the exact brace nesting before inserting.

- [ ] **Step 2: Write the loop branch**

Insert after the goal `match store.get(...) { ... }` block closes (and before the `if let Some(cont_deps)` block's own closing brace):

```rust
                    // Loop continuation hook: the clock-gated sibling of the
                    // goal hook above. Fires only when this session has an
                    // Active loop that has not hit a safety cap. Increments the
                    // tick counter BEFORE spawning so caps are enforced even if
                    // a tick crashes. Re-fires via the SAME spawn machinery as
                    // goal, but with a cadence delay.
                    if let Some(loop_reg) = crate::looping::global() {
                        let session_key_str = request.session_key.to_key_string();
                        if let Some(state) = loop_reg.get_active(&session_key_str) {
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_millis() as u64);
                            if crate::looping::pursuit::should_fire(&state, 0, now_ms) {
                                let bumped = state.clone().spent_iteration();
                                let delay = crate::looping::pursuit::tick_delay_ms(&state, now_ms);
                                let prompt = crate::looping::pursuit::tick_prompt(&state);
                                loop_reg.put(bumped);
                                spawn_continuation_run(
                                    cont_deps.registry.clone(),
                                    cont_deps.adapter.clone(),
                                    request.session_key.clone(),
                                    session_key_str.clone(),
                                    prompt,
                                    cont_deps.event_bus.clone(),
                                    Some(delay),
                                );
                                info!(session = %session_key_str, delay_ms = delay,
                                    ticks = state.iterations_used + 1,
                                    "loop: enqueued next tick");
                            } else if crate::looping::pursuit::exhausted(&state, 0, now_ms) {
                                let note = if state
                                    .deadline_ms
                                    .is_some_and(|d| now_ms != 0 && now_ms > d)
                                {
                                    crate::looping::pursuit::deadline_reached_note(&state)
                                } else {
                                    crate::looping::pursuit::cap_reached_note(&state)
                                };
                                loop_reg.put(
                                    state.with_status(crate::looping::LoopStatus::Stopped),
                                );
                                info!(session = %session_key_str, note = %note,
                                    "loop: safety cap reached, loop stopped");
                            }
                        }
                    }
```

> The `session_key_str` shadows the one in the goal block — that's fine, the goal block's binding is out of scope here. If the compiler warns about an unused earlier binding, leave the goal block as-is (it uses its own).

- [ ] **Step 3: Verify by inspection**

Confirm: the loop branch is inside the `if let Some(cont_deps)` block (so `cont_deps` is in scope); it calls `spawn_continuation_run(..., Some(delay))`; it increments before spawning; `exhausted` path flips status to `Stopped`. Grep `spawn_continuation_run(` — now exactly definition + 2 goal callsites + 1 loop callsite.

- [ ] **Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/gateway/execution_engine/execute.rs
git commit -m "feat(looping): re-fire loops via the continuation hook"
```

---

## Task 9: Unattended soft cap (safety net)

**Files:**
- Modify: `src/builtin_tools/loop_manage.rs`

The risk: an unattended autonomous run (`metadata["unattended"]="true"`) starts an uncapped loop → 24/7 daemon burns tokens forever. Mirror goal's posture: when a loop is started with NO cap at all, give it a default soft `max_iterations`. The cleanest, lowest-coupling place is at `start` time in the tool (the tool already owns cap assembly), because the tool cannot see the run's unattended flag — so apply the soft cap unconditionally when the user gave no bound, which is also the safest default and matches the article's "the model/user can always raise it."

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/builtin_tools/loop_manage.rs`:

```rust
    #[tokio::test]
    async fn start_without_any_cap_gets_default_soft_max() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Start,
            interval: Some("5m".to_string()),
            prompt: Some("p".to_string()),
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        // No user cap → a default soft cap is applied so unattended loops
        // cannot run unbounded forever.
        assert_eq!(reg.get("s").unwrap().max_iterations, Some(DEFAULT_SOFT_MAX_ITERATIONS));
    }

    #[tokio::test]
    async fn explicit_cap_is_respected_over_default() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Start,
            interval: Some("5m".to_string()),
            prompt: Some("p".to_string()),
            max_iterations: Some(1000),
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        assert_eq!(reg.get("s").unwrap().max_iterations, Some(1000));
    }
```

- [ ] **Step 2: Verify the test would fail**

Inspect: `DEFAULT_SOFT_MAX_ITERATIONS` does not exist and `start` does not apply a default. RED confirmed.

- [ ] **Step 3: Implement the soft cap**

In `src/builtin_tools/loop_manage.rs`, add the constant near `MODEL_PACED_FALLBACK_MS`:

```rust
/// Default safety cap applied when a loop is started with no explicit
/// max_iterations AND no timeout — prevents an unattended uncapped loop from
/// running forever on the 24/7 daemon. Generous (the model/user can raise it),
/// but never truly unbounded by default.
pub const DEFAULT_SOFT_MAX_ITERATIONS: u32 = 500;
```

In `start`, replace the `max_iterations` assembly so that when the user gave neither a max nor a timeout, the soft default is used. Change:

```rust
        let state = LoopState::new(session, &prompt, cadence, now)
            .with_max_iterations(args.max_iterations)
            .with_deadline_ms(deadline)
            .with_token_budget(args.token_budget);
```

to:

```rust
        // Safety net: a loop with no user-supplied bound at all gets a soft
        // iteration cap so unattended pursuit cannot run unbounded forever.
        let effective_max = match (args.max_iterations, deadline) {
            (Some(m), _) => Some(m),
            (None, Some(_)) => None, // a deadline is itself a bound
            (None, None) => Some(DEFAULT_SOFT_MAX_ITERATIONS),
        };
        let state = LoopState::new(session, &prompt, cadence, now)
            .with_max_iterations(effective_max)
            .with_deadline_ms(deadline)
            .with_token_budget(args.token_budget);
```

- [ ] **Step 4: Verify by inspection**

Confirm `DEFAULT_SOFT_MAX_ITERATIONS` exists; `start` applies it only when both `max_iterations` and `timeout_minutes` are absent; explicit caps win. Re-read the two new tests and confirm the logic satisfies them.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-loop
git add src/builtin_tools/loop_manage.rs
git commit -m "feat(looping): apply default soft iteration cap to uncapped loops"
```

---

## Self-Review (completed by plan author)

**Spec coverage:**
- §4.1 LoopState/Cadence/LoopStatus → Task 1 ✅
- §4.2 in-memory registry + global → Task 2 ✅
- §4.3 pursuit gate functions → Task 3 ✅
- §4.4 loop tool (start/stop/status/update + interval parse) → Task 5 ✅
- §4.5 execute.rs loop branch → Task 8 ✅
- §4.6 spawn_continuation_run delay_ms → Task 7 ✅
- §4.7 unattended soft cap → Task 9 ✅
- §4.8 boot wiring → Task 6 ✅
- §5 entropy reduction (converge spawn paths) → Task 7 ✅
- §6 test strategy → tests in Tasks 1,2,3,5,9 ✅
- §7 redline check → in-memory (R3), tool+model-paced (R7/R8), not in src/harness (R10) ✅
- §8 file list → matches Tasks 1-8 ✅
- §9 acceptance criteria → covered by Task 5/8/9 tests + manual deploy check

**Type consistency:** `LoopState`, `Cadence::{Fixed,ModelPaced}`, `LoopStatus::{Active,Stopped}`, `should_fire`/`exhausted`/`tick_delay_ms`/`tick_prompt`, `LoopArgs`/`LoopAction::{Start,Stop,Status,Update}`/`LoopOutput`, `LoopRegistry::{put,get,get_active,remove}`, `init_global`/`global`, `parse_interval_ms`, `DEFAULT_SOFT_MAX_ITERATIONS`, `MODEL_PACED_FALLBACK_MS`, `spawn_continuation_run(..., delay_ms)` — all consistent across tasks.

**Known open verifications for the implementer (flagged inline, not placeholders):**
1. Task 5/6: the exact `AlephTool` trait method names — verify against `src/builtin_tools/goal.rs:148`. This is the top compile risk.
2. Task 6: whether the builder stores tool instances as struct fields or builds them on-demand in `definitions.rs` — the plan handles both branches explicitly.
3. `crate::sync_primitives::Arc` availability (Task 2 note).

**Build note:** Per project rule + user instruction, no `cargo` verification is run; correctness rests on the TDD test design + inspection steps. The implementer commits each task directly in the `loop-command` worktree.
