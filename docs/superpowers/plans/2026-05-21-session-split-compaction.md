# Session-Split Compaction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When in-place compaction can no longer hold context pressure down, end the current session and continue the run in a fresh one (`epoch + 1`) seeded with a clean summary plus the verbatim fresh tail — so per-turn cost resets to bounded instead of growing linearly with task length.

**Architecture:** A new `LoopDirective::SplitSession` is emitted by `ContextBudget` when the `CompactionCircuitBreaker` trips (capped at `max_splits` per run, then `FinalReply`). A new `src/context/compact/session_split.rs` module performs the split: summarize the pre-tail events via the existing `ContextCompactor`, mint the child key with `SessionKey::with_next_epoch()`, register the new epoch through a narrow `SessionEpochRegistrar` trait (gateway `SessionStore` implements it), and seed the child session. The harness `run()` loop holds a mutable `current_session`; `think.rs` handles the new directive and signals the child id back. In-place compaction at the `warning` tier is unchanged.

**Tech Stack:** Rust, async-trait, tokio, serde. Reuses `SessionKey::epoch`/`with_next_epoch`, `SessionStore::get_or_create`, `ContextCompactor`, `CompactionCircuitBreaker`.

**Spec:** [`docs/superpowers/specs/2026-05-21-session-split-compaction-design.md`](../specs/2026-05-21-session-split-compaction-design.md)

**Worktree:** Implementation runs in `worktree-feat-session-split`. Spec + plan live on `main`.

**MERGE POLICY:** Per the user's 2026-05-21 instruction — do NOT merge this branch into `main` after implementation. Stop at "implementation complete, tests green, branch ready" and wait for the user's explicit merge instruction. Task 8 ends at readiness; it does NOT merge.

**Cargo concurrency cap:** This machine OOM-kills past 3 concurrent cargo processes. Before EVERY `cargo` command, prefix the gate:
```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && <cargo command>
```
Use `run_in_background: true` for cargo runs (compiles take 5-20 min); read the output file when notified.

---

## File Structure

| File | New / Modified | Responsibility |
|------|----------------|----------------|
| `src/session/events.rs` | Modified | Add `SessionEvent::SessionForked { parent_session_id: String, at: Timestamp }`. |
| `src/session/mod.rs` *(or a new `src/session/epoch_registrar.rs`)* | Modified / New | Define the `SessionEpochRegistrar` trait. |
| `src/gateway/session_store/mod.rs` | Modified | Implement `SessionEpochRegistrar` for the gateway `SessionStore` (delegates to `get_or_create`). |
| `src/context/budget/mod.rs` | Modified | `LoopDirective::SplitSession` variant; `ContextBudget` split counter; tiered `before_turn`; `ContextBudgetConfig.max_splits`. |
| `src/context/compact/session_split.rs` | **New** | `perform_session_split` + `SplitOutcome` + `SplitError`. |
| `src/context/compact/mod.rs` | Modified | Declare `pub mod session_split;`. |
| `src/harness/deps.rs` | Modified | `HarnessDeps.session_epoch_registrar: Option<Arc<dyn SessionEpochRegistrar>>`. |
| `src/harness/agent.rs` | Modified | `run()` holds a mutable `current_session`; rebinds it on a split signal; harness exposes the final session id. |
| `src/harness/agent/think.rs` | Modified | Handle the `SplitSession` directive; call `perform_session_split`; signal the child id back via the turn return. |
| `src/orchestrator/harness_bridge.rs` | Modified | After the run, read the harness's final session id (R2). |
| `src/harness/tests/task10_wiring.rs` | Modified | Integration tests for split happy-path + fail-soft. |

---

## Task 1: `SessionEvent::SessionForked` variant

**Files:**
- Modify: `src/session/events.rs`

- [ ] **Step 1: Write the failing serde round-trip test**

Append to the `#[cfg(test)] mod tests` in `src/session/events.rs` (read the file to find the existing test module; if none, add `#[cfg(test)] mod tests { use super::*; ... }`):

```rust
#[test]
fn session_forked_event_round_trips_through_json() {
    let event = SessionEvent::SessionForked {
        parent_session_id: "agent:a/main:k:s2".to_string(),
        at: 1_700_000_000_000,
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: SessionEvent = serde_json::from_str(&json).unwrap();
    match parsed {
        SessionEvent::SessionForked { parent_session_id, .. } => {
            assert_eq!(parent_session_id, "agent:a/main:k:s2");
        }
        other => panic!("expected SessionForked, got {other:?}"),
    }
}
```

NOTE: confirm the `at` field type — other variants use `at: Timestamp` (`Timestamp` is a type alias, almost certainly `i64`/`u64` ms). Use the same `Timestamp` type. Confirm `SessionEvent` derives `Serialize`/`Deserialize`/`Debug` (it does — other variants round-trip).

- [ ] **Step 2: Run the failing test (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib session::events 2>&1 | tail -20
```

Expected: compile error — no `SessionForked` variant.

- [ ] **Step 3: Add the variant**

In `src/session/events.rs`, add to the `SessionEvent` enum (the enum is `#[non_exhaustive]` — additive variants are safe), placed near `SessionCreated` / `SessionWoken`:

```rust
    /// Recorded as the first event of a child session created by
    /// compaction-driven session-split. `parent_session_id` is the parent
    /// session key string (`SessionKey::to_key_string()`).
    SessionForked {
        parent_session_id: String,
        at: Timestamp,
    },
```

If `SessionEvent` has a `match` somewhere that is NOT `#[non_exhaustive]`-tolerant (an exhaustive match in non-test code), the compiler will flag it — add a `SessionForked` arm there (treat it like `SessionCreated`: a metadata event, no business effect). Grep `match.*event` / `SessionEvent::` to find exhaustive matches; most code paths only care about message variants.

- [ ] **Step 4: Run the test + whole-crate check (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib session::events 2>&1 | tail -15
```

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo check -p alephcore --lib 2>&1 | tail -15
```

Expected: test passes; crate compiles (fix any exhaustive-match sites flagged).

- [ ] **Step 5: Commit**

```bash
git add src/session/events.rs
git commit -m "session: add SessionForked event for compaction session-split lineage"
```

---

## Task 2: `SessionEpochRegistrar` trait + gateway impl

**Files:**
- Create: `src/session/epoch_registrar.rs`
- Modify: `src/session/mod.rs`
- Modify: `src/gateway/session_store/mod.rs`

- [ ] **Step 1: Create the trait**

Create `src/session/epoch_registrar.rs`:

```rust
//! Narrow trait for registering a new session epoch (generation).
//!
//! Compaction-driven session-split mints a child session key at `epoch + 1`
//! and must make that generation visible to epoch resolution
//! (`SessionStore::get_current_epoch`). The harness depends on this narrow
//! trait rather than the gateway `SessionStore` concrete type, preserving the
//! Core → Interface dependency direction (CLAUDE.md R1 / P4).

use async_trait::async_trait;

use crate::session::service::SessionId;

/// Persists a session key as a live generation so epoch resolution sees it.
#[async_trait]
pub trait SessionEpochRegistrar: Send + Sync {
    /// Register `key` (typically a child session at the next epoch) so that a
    /// subsequent `get_current_epoch` for its base pattern resolves to it.
    async fn register_epoch(&self, key: &SessionId) -> anyhow::Result<()>;
}
```

- [ ] **Step 2: Declare the module**

In `src/session/mod.rs`, add `pub mod epoch_registrar;` alongside the other `pub mod` lines (read the file, match ordering).

- [ ] **Step 3: Write the failing impl test**

Append to the `#[cfg(test)] mod tests` in `src/gateway/session_store/mod.rs` (or wherever `SessionStore` tests live — check `src/gateway/session_store/` for an existing test module; `sqlite_backend` / `file_backend` have tests):

```rust
#[tokio::test]
async fn register_epoch_makes_get_current_epoch_see_new_generation() {
    use crate::session::epoch_registrar::SessionEpochRegistrar;
    use crate::routing::session_key::SessionKey;

    // Build an in-memory / temp-backed SessionStore the same way existing
    // tests in this module do — copy their setup idiom.
    let store = /* existing test-store constructor */;

    let base = SessionKey::Main {
        agent_id: "a".into(),
        main_key: "k".into(),
        epoch: 0,
    };
    store.get_or_create(&base).await.unwrap();

    let child = base.with_next_epoch(); // epoch 1
    store.register_epoch(&child).await.unwrap();

    let resolved = store
        .get_current_epoch(&base.base_key_pattern())
        .await
        .unwrap();
    assert_eq!(resolved, 1, "register_epoch must make epoch 1 the current epoch");
}
```

NOTE: the exact `SessionStore` test-construction idiom and the `base_key_pattern()` argument form must be copied from existing tests in `src/gateway/session_store/` — read them first. `base_key_pattern()` is a real `SessionKey` method (`src/routing/session_key.rs:300`).

- [ ] **Step 4: Run the failing test (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib gateway::session_store 2>&1 | tail -20
```

Expected: compile error — `SessionStore` does not implement `SessionEpochRegistrar`.

- [ ] **Step 5: Implement the trait for `SessionStore`**

In `src/gateway/session_store/mod.rs`, add (the concrete `SessionStore` type — confirm whether `SessionStore` is a trait or a struct; if it is a trait with multiple backends, implement `SessionEpochRegistrar` for the same wrapper type that already exposes `get_or_create`/`get_current_epoch` to callers — likely a struct holding `Arc<dyn SessionStoreBackend>` or similar; read the file to identify the public type):

```rust
#[async_trait::async_trait]
impl crate::session::epoch_registrar::SessionEpochRegistrar for <SessionStorePublicType> {
    async fn register_epoch(&self, key: &crate::session::service::SessionId) -> anyhow::Result<()> {
        self.get_or_create(key)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("register_epoch: get_or_create failed: {e}"))
    }
}
```

If `SessionStore` is itself a trait, add `register_epoch` as a provided method on that trait OR implement `SessionEpochRegistrar` for the concrete manager type the harness will actually be handed. Pick whichever the existing wiring makes natural; the goal is: the gateway's session manager satisfies `SessionEpochRegistrar`.

- [ ] **Step 6: Run the test (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib gateway::session_store 2>&1 | tail -20
```

Expected: the new test passes; pre-existing session_store tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src/session/epoch_registrar.rs src/session/mod.rs src/gateway/session_store/mod.rs
git commit -m "session: add SessionEpochRegistrar trait + gateway SessionStore impl"
```

---

## Task 3: `LoopDirective::SplitSession` + `ContextBudget` tiered trigger

**Files:**
- Modify: `src/context/budget/mod.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` in `src/context/budget/mod.rs`. Read the existing `test_before_turn_circuit_breaker_escalates_to_final_reply` test first and model the setup on it (it constructs a `ContextBudget`, drives `before_turn` until the breaker trips). The breaker trips after `circuit_breaker_max` consecutive compactions.

```rust
#[test]
fn circuit_breaker_trip_emits_split_session_when_under_cap() {
    let mut cfg = default_config();
    cfg.circuit_breaker_max = 2;
    cfg.max_splits = 3;
    let mut budget = ContextBudget::new(&cfg);
    // Drive warning-tier pressure until the breaker trips.
    // (Use the same message/prompt setup that the existing
    //  circuit_breaker_escalates test uses to reach the warning band.)
    let directive = drive_until_breaker_trips(&mut budget); // helper — see note
    assert_eq!(
        directive,
        LoopDirective::SplitSession,
        "first breaker trip under the split cap must request a session split",
    );
}

#[test]
fn split_session_falls_back_to_final_reply_at_cap() {
    let mut cfg = default_config();
    cfg.circuit_breaker_max = 2;
    cfg.max_splits = 1;
    let mut budget = ContextBudget::new(&cfg);
    let first = drive_until_breaker_trips(&mut budget);
    assert_eq!(first, LoopDirective::SplitSession);
    budget.record_split(); // split_count -> 1 == max_splits
    let second = drive_until_breaker_trips(&mut budget);
    assert_eq!(
        second,
        LoopDirective::FinalReply,
        "once max_splits is reached, the breaker trip must fall back to FinalReply",
    );
}
```

NOTE: `drive_until_breaker_trips` is illustrative — implement the test by replicating the exact `before_turn(...)` call loop from the existing `test_before_turn_circuit_breaker_escalates_to_final_reply` test (same message vec, same thresholds to land in the warning band, called `circuit_breaker_max` times). Do NOT invent a helper that does not exist; inline the loop. Read that existing test and copy its mechanics.

- [ ] **Step 2: Run the failing tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib context::budget 2>&1 | tail -25
```

Expected: compile errors — `LoopDirective::SplitSession`, `cfg.max_splits`, `budget.record_split()` undefined.

- [ ] **Step 3: Add the `SplitSession` directive variant**

In `src/context/budget/mod.rs`, in the `LoopDirective` enum (currently `Continue`, `CompactAndContinue`, `FinalReply`, `StopDiminishing`), add:

```rust
    /// In-place compaction is not keeping pressure down — split the session:
    /// continue the run in a fresh child session (epoch + 1) seeded with a
    /// summary + fresh tail. See `context::compact::session_split`.
    SplitSession,
```

- [ ] **Step 4: Add `max_splits` to config + split counter to `ContextBudget`**

In `ContextBudgetConfig`, add:

```rust
    /// Max session-splits allowed in one run before the circuit-breaker trip
    /// falls back to `FinalReply`. Default 3.
    pub max_splits: usize,
```

Update `default_config()` (the test helper) and any production `ContextBudgetConfig` construction site to set `max_splits: 3`. Grep `ContextBudgetConfig {` to find all construction sites — there is at least `default_config()` in the test module and the production config builder. The production builder reads from `[context_budget]` TOML (`src/config/types/phase6_wiring.rs` area) — add a `max_splits` field there with `#[serde(default = "default_max_splits")]` returning 3, OR default it in the builder if the toml struct should stay minimal. Match how `circuit_breaker_max` / `diminishing_window` are threaded — mirror that field exactly.

In `ContextBudget`, add fields:

```rust
    split_count: usize,
    max_splits: usize,
```

Initialize in `ContextBudget::new`: `split_count: 0, max_splits: config.max_splits`.

Add the public method:

```rust
/// Record that a session-split completed. Increments the per-run split
/// counter; once it reaches `max_splits`, further breaker trips fall back
/// to `FinalReply`.
pub fn record_split(&mut self) {
    self.split_count = self.split_count.saturating_add(1);
}
```

- [ ] **Step 5: Change the breaker-trip branch in `before_turn`**

In `before_turn`, the warning-tier branch currently is:

```rust
        if pressure.ratio >= self.warning_threshold {
            if self.circuit_breaker.record_compaction() {
                tracing::warn!(
                    target: "context_budget",
                    "Compaction circuit breaker tripped — escalating to FinalReply"
                );
                return LoopDirective::FinalReply;
            }
            tracing::info!( /* … */ );
            return LoopDirective::CompactAndContinue;
        }
```

Change the breaker-trip arm:

```rust
        if pressure.ratio >= self.warning_threshold {
            if self.circuit_breaker.record_compaction() {
                if self.split_count < self.max_splits {
                    tracing::warn!(
                        target: "context_budget",
                        split_count = self.split_count,
                        "Compaction circuit breaker tripped — requesting session split"
                    );
                    return LoopDirective::SplitSession;
                }
                tracing::warn!(
                    target: "context_budget",
                    split_count = self.split_count,
                    "Compaction circuit breaker tripped and split cap reached — escalating to FinalReply"
                );
                return LoopDirective::FinalReply;
            }
            tracing::info!( /* … keep the existing info log unchanged … */ );
            return LoopDirective::CompactAndContinue;
        }
```

Leave the `critical` branch (`pressure.ratio >= self.critical_threshold` → `FinalReply`) and the `after_turn` / `StopDiminishing` path unchanged.

- [ ] **Step 6: Run the tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib context::budget 2>&1 | tail -25
```

Expected: the 2 new tests pass. The existing `test_before_turn_circuit_breaker_escalates_to_final_reply` test will now FAIL (the breaker trip now returns `SplitSession`, not `FinalReply`) — UPDATE that test: rename it to `..._escalates_to_split_session` and assert `LoopDirective::SplitSession`. Keep one assertion path for the cap → `FinalReply` (covered by the new `split_session_falls_back_to_final_reply_at_cap`).

- [ ] **Step 7: Commit**

```bash
git add src/context/budget/mod.rs src/config/types/phase6_wiring.rs
git commit -m "context/budget: emit SplitSession on circuit-breaker trip, capped by max_splits"
```

(Drop `phase6_wiring.rs` if you defaulted `max_splits` in the builder instead of the toml struct.)

---

## Task 4: `perform_session_split` module

**Files:**
- Create: `src/context/compact/session_split.rs`
- Modify: `src/context/compact/mod.rs`

- [ ] **Step 1: Create the module skeleton + failing tests**

Create `src/context/compact/session_split.rs`:

```rust
//! Compaction-driven session-split.
//!
//! When in-place compaction can no longer hold context pressure down, the
//! harness ends the current session and continues in a fresh child session
//! (`epoch + 1`) seeded with a summary of the pre-tail history plus the
//! verbatim fresh tail. The parent session's log is frozen — never re-read
//! by the loop — so per-turn cost resets to bounded.

use std::sync::Arc;

use crate::context::compact::compactor::ContextCompactor;
use crate::session::epoch_registrar::SessionEpochRegistrar;
use crate::session::events::SessionEvent;
use crate::session::service::{SessionId, SessionService};
use crate::session::events::SessionEventRecord;

/// Outcome of a successful split.
#[derive(Debug, Clone)]
pub struct SplitOutcome {
    pub child_session_id: SessionId,
}

/// Why a split could not be performed.
#[derive(Debug)]
pub enum SplitError {
    /// The session key kind has no epoch (Group/Task/Subagent/Ephemeral) —
    /// `with_next_epoch()` returned the key unchanged.
    NotSplittable,
    /// Summarization, epoch registration, or event emission failed.
    Failed(anyhow::Error),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSplittable => write!(f, "session key kind is not splittable"),
            Self::Failed(e) => write!(f, "session split failed: {e}"),
        }
    }
}
impl std::error::Error for SplitError {}

/// Perform a compaction-driven session split.
///
/// `tail_start` is the index into `events` where the fresh tail begins —
/// the caller (`think.rs`) computes it via the harness-private
/// `tail_start_index`. `events[..tail_start]` is summarized;
/// `events[tail_start..]` is copied verbatim into the child.
pub async fn perform_session_split(
    session: &dyn SessionService,
    epoch_registrar: &dyn SessionEpochRegistrar,
    compactor: &ContextCompactor,
    parent_session_id: &SessionId,
    events: &[SessionEventRecord],
    tail_start: usize,
) -> Result<SplitOutcome, SplitError> {
    // 1. Mint the child key. If the kind has no epoch, with_next_epoch returns
    //    an equal key — not splittable.
    let child = parent_session_id.with_next_epoch();
    if &child == parent_session_id {
        return Err(SplitError::NotSplittable);
    }

    // 2. Summarize events[..tail_start] via the compactor. Build a message
    //    list from the pre-tail events and call the compactor to produce the
    //    `[Context Summary]` text. Reuse the compactor's existing summary
    //    machinery (see `ContextCompactor::compact`); a thin helper that
    //    returns just the summary string is acceptable.
    let summary_text = summarize_pretail(compactor, events, tail_start)
        .await
        .map_err(SplitError::Failed)?;

    // 3. Register the new epoch so gateway routing sees it.
    epoch_registrar
        .register_epoch(&child)
        .await
        .map_err(SplitError::Failed)?;

    // 4. Seed the child: SessionForked -> SystemMessage(summary) -> fresh tail.
    let now = crate::session::events::now_ms();
    session
        .emit_event(
            &child,
            SessionEvent::SessionForked {
                parent_session_id: parent_session_id.to_key_string(),
                at: now,
            },
        )
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("emit SessionForked: {e}")))?;

    session
        .emit_event(
            &child,
            SessionEvent::SystemMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: summary_text,
                at: crate::session::events::now_ms(),
            },
        )
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("emit summary: {e}")))?;

    for record in &events[tail_start..] {
        session
            .emit_event(&child, record.event.clone())
            .await
            .map_err(|e| SplitError::Failed(anyhow::anyhow!("copy fresh-tail event: {e}")))?;
    }

    Ok(SplitOutcome { child_session_id: child })
}

/// Summarize `events[..tail_start]` into a single `[Context Summary]` string.
async fn summarize_pretail(
    compactor: &ContextCompactor,
    events: &[SessionEventRecord],
    tail_start: usize,
) -> anyhow::Result<String> {
    // Implementation: assemble the pre-tail events into a Vec<UnifiedMessage>
    // and run the compactor's summary path. The compactor already produces a
    // `[Context Summary]` placeholder in `compact()`; expose/reuse that core
    // so this returns just the string. See the implementer note below.
    todo!("see implementer note — reuse ContextCompactor summary core")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_epoch_key_kind_is_not_splittable() {
        // An Ephemeral key has no epoch — with_next_epoch returns it unchanged.
        let parent = crate::routing::session_key::SessionKey::Ephemeral {
            ephemeral_id: "x".into(),
        };
        // Build minimal fakes for session / registrar / compactor, OR assert
        // the early NotSplittable branch before any of them is touched. The
        // child==parent check is the first statement, so a test can pass
        // throwaway fakes and still exercise only that branch.
        // (See implementer note for fake construction.)
    }

    #[tokio::test]
    async fn split_seeds_child_with_forked_summary_and_fresh_tail() {
        // With a Main key at epoch 0, a fake SessionService recording emitted
        // events, a fake registrar, and a compactor stub:
        //   - child id is the parent at epoch 1
        //   - child's events are: SessionForked, SystemMessage(summary),
        //     then the verbatim events[tail_start..]
        // (See implementer note for fake construction.)
    }
}
```

**Implementer note for Task 4:** The two hard parts are (a) `summarize_pretail` and (b) the test fakes.

(a) `ContextCompactor::compact` already produces a `[Context Summary]` message in-place. Read `src/context/compact/compactor.rs` and either: extract the summary-producing core into a `pub(crate)` method that returns the summary `String`, then call it from `summarize_pretail`; or, if `compact(&mut messages, ...)` is the only entry, call it on a `Vec<UnifiedMessage>` built from the pre-tail events and read back the resulting first `[Context Summary]` message. The first approach is cleaner — prefer it. Whichever you choose, `summarize_pretail` must end up returning the summary text and the `todo!()` must be gone.

(b) Test fakes: `SessionService` is a trait — implement a minimal `RecordingSessionService` that stores emitted `(SessionId, SessionEvent)` pairs in a `Mutex<Vec<...>>` and returns them. `SessionEpochRegistrar` is one async method — a fake that records the key. `ContextCompactor::new(provider, config)` needs an `AiProvider` — use the same mock provider the existing compactor tests use (`src/context/compact/compactor.rs` has tests — copy their `ContextCompactor` construction). If wiring a real `ContextCompactor` into the unit test is heavy, gate the summary with a stubbed summarizer: structure `summarize_pretail` so the test can inject a canned summary, OR test `perform_session_split`'s seeding logic with a compactor whose provider returns a fixed string. The seeding assertions (child id, event order) are the priority.

- [ ] **Step 2: Declare the module**

In `src/context/compact/mod.rs`, add `pub mod session_split;` alongside the existing `pub mod compactor;` etc.

- [ ] **Step 3: Run the failing tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib context::compact::session_split 2>&1 | tail -25
```

Expected: fails — `todo!()` panic and/or unimplemented test bodies.

- [ ] **Step 4: Implement `summarize_pretail` + fill in the two test bodies**

Replace the `todo!()` per implementer note (a). Fill in both test bodies per implementer note (b). The tests must assert: `NotSplittable` for an `Ephemeral` parent; child id `== parent.with_next_epoch()`; the child's emitted event sequence is exactly `[SessionForked, SystemMessage, <verbatim events[tail_start..]>]`.

- [ ] **Step 5: Run the tests (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib context::compact::session_split 2>&1 | tail -25
```

Expected: both tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/context/compact/session_split.rs src/context/compact/mod.rs
git commit -m "context/compact: add perform_session_split (epoch bump + summary + fresh-tail seed)"
```

---

## Task 5: Harness deps + loop integration

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent.rs`
- Modify: `src/harness/agent/think.rs`

- [ ] **Step 1: Add the deps field**

In `src/harness/deps.rs`, add to `HarnessDeps`:

```rust
    /// Registrar that makes a split-created child epoch visible to gateway
    /// epoch resolution. `None` disables session-split — the loop falls back
    /// to `FinalReply` when the budget asks for a split. See
    /// `context::compact::session_split`.
    pub session_epoch_registrar: Option<Arc<dyn crate::session::epoch_registrar::SessionEpochRegistrar>>,
```

Every `HarnessDeps { ... }` construction site must add `session_epoch_registrar: None` (production wiring sets it in Task 6; tests use `None`). Grep `HarnessDeps {` — there are sites in `src/harness/agent.rs` (constructor test helpers ~lines 930/993/1053), `src/harness/tests/*.rs`, `src/orchestrator/harness_bridge.rs`, `src/orchestrator/deps_builder.rs`. Add `session_epoch_registrar: None` to each (Task 6 changes the orchestrator one).

- [ ] **Step 2: Write the failing integration test**

Append to `src/harness/tests/task10_wiring.rs` a test that drives a `SplitSession` directive. Read the existing `diminishing_returns_fires_grace_and_hits_limit` test (Cycle 3) for the `HarnessDeps` fixture idiom. The test needs: a `ContextBudget` configured so the breaker trips quickly (`circuit_breaker_max` small, `warning_threshold` low so warning-tier pressure hits), `max_splits >= 1`, a `context_compactor` wired (a compactor over a mock provider), and a fake `session_epoch_registrar`.

```rust
#[tokio::test]
async fn split_session_directive_continues_run_in_child_session() {
    // Budget: tiny window so warning pressure hits; circuit_breaker_max = 1
    //   so the FIRST warning-tier turn trips the breaker -> SplitSession.
    // Provider: returns a short final text (no tool calls) so the run ends
    //   cleanly after the split.
    // Assert: run completes Ok; the harness's reported final session id has
    //   epoch == parent.epoch + 1; the child session has a SessionForked
    //   first event.
    // (Full fixture mirrors diminishing_returns_fires_grace_and_hits_limit;
    //  add context_compactor + session_epoch_registrar: Some(fake).)
}

#[tokio::test]
async fn split_session_failsoft_falls_back_to_final_reply() {
    // Same setup but session_epoch_registrar is a fake whose register_epoch
    // returns Err. Assert: run still completes Ok (no panic), hit_limit is
    // set (FinalReply fallback path taken), final session id == parent
    // (no split took effect).
}
```

The test bodies are spelled out as comments because the exact `HarnessDeps` field list and `ContextBudget` config must be copied from the current `task10_wiring.rs` — the implementer fills them by mirroring the existing tests. The ASSERTIONS above are the contract.

- [ ] **Step 3: Run the failing test (gated)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib harness::tests::task10_wiring::split_session 2>&1 | tail -25
```

Expected: fails — the `SplitSession` directive is not handled; the harness has no "final session id" accessor.

- [ ] **Step 4: `run()` — mutable current session**

In `src/harness/agent.rs`, `run(&self, session_id: &SessionId, ...)`: introduce `let mut current_session: SessionId = session_id.clone();` at the top of `run`. Replace the `&session_id` passed into `run_turn_internal` with `&current_session`. After each `run_turn_internal` call, if the turn signals a split (see Step 5's return-tuple extension), rebind `current_session = child_id` and continue the loop.

Add a way for the harness to expose the final session id. The simplest: a field `final_session_id: Mutex<Option<SessionId>>` (or reuse an existing run-result struct) set at the end of `run()` to `current_session`. Add a public accessor `pub fn final_session_id(&self) -> Option<SessionId>`. Mirror how `hit_limit` / `total_tokens` are exposed.

- [ ] **Step 5: `run_turn_internal` — return the split signal**

`run_turn_internal` currently returns `Result<(TurnState, usize, bool), HarnessError>`. Extend to `Result<(TurnState, usize, bool, Option<SessionId>), HarnessError>` — the 4th element is `Some(child)` when a split occurred this turn, else `None`. Update every `return Ok((...))` / `result = Ok((...))` site in `think.rs` to add the 4th element (`None` everywhere except the split path). Update `run()`'s match arms to destructure the 4-tuple and rebind `current_session` when the 4th is `Some`.

- [ ] **Step 6: `think.rs` — handle `SplitSession`**

In `src/harness/agent/think.rs`, where `budget_directive` is matched (alongside the `CompactAndContinue` and `FinalReply` arms), add a `SplitSession` arm. It runs AFTER the `events` + `tail_start` are computed (both already exist near the top of the turn). Pseudocode shape:

```rust
if matches!(budget_directive, Some(LoopDirective::SplitSession)) {
    let did_split = match (
        self.deps.context_compactor.as_ref(),
        self.deps.session_epoch_registrar.as_ref(),
    ) {
        (Some(compactor), Some(registrar)) => {
            match crate::context::compact::session_split::perform_session_split(
                self.deps.session.as_ref(),
                registrar.as_ref(),
                compactor.as_ref(),
                session_id,
                &events,
                tail_start,
            )
            .await
            {
                Ok(outcome) => {
                    // tell ContextBudget a split happened
                    if let Some(budget) = self.deps.context_budget.as_ref() {
                        budget.lock().await.record_split();
                    }
                    Some(outcome.child_session_id)
                }
                Err(e) => {
                    tracing::warn!(?session_id, %e, "session split failed; falling back to FinalReply");
                    None
                }
            }
        }
        _ => None, // registrar/compactor not wired — fall back
    };

    if let Some(child) = did_split {
        // Signal the new session up to run(); the loop continues in the child.
        return Ok((TurnState::Continue, 0, false, Some(child)));
    }
    // Fall-soft: behave exactly like the FinalReply branch.
    self.hit_limit.store(true, Ordering::Relaxed);
    self.fire_grace_turn(session_id, &events, &messages, callback, iterations, GraceReason::Budget).await;
    callback.on_complete_via_harness();
    return Ok((TurnState::Done, 0, false, None));
}
```

Place this arm BEFORE the `CompactAndContinue` handling (a `SplitSession` directive must not also trigger in-place compaction). Confirm `tail_start` and `events` and `messages` are all in scope at the insertion point — `events`/`tail_start` are computed at the very top of the turn; `messages` is built right after. If the `SplitSession` arm must run before `messages` exists, either move it after `messages` is built, or pass only `events`+`tail_start` (the split does not need `messages` — `perform_session_split` takes `events`). The grace-turn fallback DOES need `messages`; place the whole arm after `messages` is assembled to keep both paths valid.

- [ ] **Step 7: Run the tests (gated, background — harness compile is slow)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib harness::tests::task10_wiring 2>&1 | tail -30
```

Expected: the 2 new split tests pass; all pre-existing task10_wiring tests still pass.

- [ ] **Step 8: Commit**

```bash
git add src/harness/deps.rs src/harness/agent.rs src/harness/agent/think.rs src/harness/tests/task10_wiring.rs
git commit -m "harness: handle SplitSession directive — switch loop to child session"
```

---

## Task 6: Orchestrator wiring

**Files:**
- Modify: `src/orchestrator/harness_bridge.rs`
- Modify: `src/orchestrator/deps_builder.rs` *(if it constructs `HarnessDeps`)*

- [ ] **Step 1: Wire the registrar into `HarnessDeps`**

In the orchestrator code that builds `HarnessDeps` (`harness_bridge.rs` and/or `deps_builder.rs`), set `session_epoch_registrar` to the gateway session manager that implements `SessionEpochRegistrar` (Task 2). The orchestrator already holds a `session_service`; identify whether it also has a handle to the gateway `SessionStore`/session-manager. If it does, pass `Some(that.clone())`. If it does not, this is the wiring gap — thread the registrar `Arc` from the boot/orchestrator-construction site (`src/bin/aleph-server/commands/start/orchestrator_init.rs`) down to where `HarnessDeps` is built. Keep it `Option` — if no registrar is available in a given deployment, `None` is valid (session-split disabled).

- [ ] **Step 2: Read the harness's final session id after the run**

In `harness_bridge.rs`, after the inner `AgentHarness::run(...)` completes, call the new `final_session_id()` accessor. If it returns `Some(child)` and `child != session_id`, update the bridge's local `session_id` to `child` before persisting run results / trace, so downstream consumers key off the final epoch. Add a `tracing::info!` recording the parent→child transition.

- [ ] **Step 3: Build + run orchestrator tests (gated, background)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib orchestrator 2>&1 | tail -25
```

Expected: clean compile; orchestrator tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/orchestrator/
git commit -m "orchestrator: wire SessionEpochRegistrar + adopt harness final session id"
```

---

## Task 7: Audit + integration hardening

**Files:**
- Modify: `src/harness/tests/task10_wiring.rs` (if audit reveals gaps)

- [ ] **Step 1: Audit trace + stall tracker across a split (R3)**

Read `src/harness/trace.rs` / `src/harness/trace_sink.rs` and the `StallTracker` usage in `run()`. Confirm: (a) trace events do not assume a single fixed session id for a run — if a trace event carries a session id, it should reflect `current_session` at emit time, not the original; (b) the `StallTracker` is time-based and session-agnostic (it is — `record_activity` / `is_stalled` use `Instant`, no session). If the trace sink keys persisted trace rows on the original session id and that is wrong post-split, note it; a minimal fix is to pass `&current_session` where the trace currently uses the run's original `session_id`. If no real issue exists, write a one-line comment in `run()` documenting that trace/stall are split-safe and move on.

- [ ] **Step 2: Whole-crate check + full provider/harness test sweep (gated, background)**

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo check -p alephcore --lib --tests 2>&1 | tail -20
```

```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib context::budget context::compact session:: harness::tests::task10_wiring 2>&1 | tail -40
```

(Run the four areas as separate gated invocations if cargo rejects multiple filter args — it does; one filter per invocation.)

Expected: clean check; all cycle tests green.

- [ ] **Step 3: Commit any audit fixes**

```bash
git add -A src/harness/
git commit -m "harness: confirm trace + stall tracker are session-split safe"
```

(Skip this commit if the audit found nothing to change.)

---

## Task 8: Final review — STOP, do not merge

**Files:** (none — review + memory)

- [ ] **Step 1: Dispatch a final holistic review**

Per `subagent-driven-development`, dispatch one final reviewer over the whole implementation diff (`99a6c1974..HEAD`, `src/` only): spec coverage, R1/R3/R4/R10 compliance, cross-task type consistency, and an independent test run.

- [ ] **Step 2: Merge latest main into the worktree branch**

```bash
git fetch origin
git merge main --no-edit
```

Resolve conflicts if any; re-run Task 7 Step 2 after.

- [ ] **Step 3: Update memory**

Create `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_session_split_compaction_cycle5.md` (type=project): the tiered design, commit SHAs, R1/R2 resolution, deferred items, tests-green snapshot, and the fact that the branch is **committed but NOT merged — awaiting user instruction**. Add a one-line entry to the top of `MEMORY.md`.

- [ ] **Step 4: STOP — report readiness, do NOT merge**

Per the user's 2026-05-21 instruction: do NOT merge this branch into `main`. Report: implementation complete, all tests green, branch `worktree-feat-session-split` ready, here are the commit SHAs — and wait for the user's explicit merge instruction. Do NOT run `git merge` into main, do NOT fast-forward, do NOT remove the worktree.

---

## Self-Review Notes

**Spec coverage:**
- §1 tiered trigger → Task 3.
- §2 `LoopDirective::SplitSession` → Task 3.
- §3 `perform_session_split` → Task 4.
- §4 `SessionEpochRegistrar` → Task 2; `HarnessDeps` field + `run()` mutable session + `think.rs` arm → Task 5; harness reports final id → Task 5.
- §5 R1 (epoch registration) → Tasks 2 + 4 (`register_epoch` call). R2 (orchestrator view) → Task 6.
- §2 `SessionForked` event → Task 1.
- §Testing → unit tests in Tasks 1-4, integration in Task 5, audit in Task 7.
- §Risks R3 (trace/stall) → Task 7 Step 1.

**Type consistency:**
- `SessionEpochRegistrar::register_epoch(&self, key: &SessionId) -> anyhow::Result<()>` — defined Task 2, called Task 4, wired Tasks 5-6.
- `LoopDirective::SplitSession` — Task 3, matched Task 5.
- `perform_session_split(session, epoch_registrar, compactor, parent_session_id, events, tail_start)` — defined Task 4, called Task 5 with the same arg order.
- `SplitOutcome.child_session_id: SessionId` — Task 4, read Task 5.
- `ContextBudget::record_split()` — Task 3, called Task 5.
- `run_turn_internal` return `(TurnState, usize, bool, Option<SessionId>)` — Task 5 Step 5, consumed Task 5 Step 4.
- `final_session_id()` accessor — Task 5, read Task 6.
- `SessionEvent::SessionForked { parent_session_id: String, at: Timestamp }` — Task 1, emitted Task 4.

**Placeholder scan:** No TBD/TODO in shipped code. Task 4's skeleton contains one `todo!("…")` that Step 4 explicitly removes — the plan calls this out as a required step, not a placeholder left behind. The Task 4/5 test bodies given as comment-contracts name exact assertions and point at the existing test to mirror — they are "copy the fixture idiom from test X" instructions, not vague gaps.

**Scope check:** 8 tasks across budget / session / context / harness / orchestrator / gateway. Large but sequential and each task is independently testable. It is one coherent feature (session-split) and was approved as one cycle. The heaviest of the long-task-hardening cycles.
