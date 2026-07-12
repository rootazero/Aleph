//! `GoalStore` — `SQLite` persistence for standing goals, keyed by session.
//!
//! One row per session (PK = `session_id`), goal serialized as a JSON blob.
//! Opens via the process-safe helper (`open_sqlite_safe`, Spec C) so it
//! never races the daemon's other `SQLite` writers. Survives `/resume`.

use std::path::Path;

use crate::error::{AlephError, Result};
use crate::goal::pursuit;
use crate::goal::types::{Goal, GoalStatus};

/// A claimed continuation presumed dead once this long past its due wake — the
/// pursuit then re-claims instead of staying blocked forever on a marker whose
/// task was killed (daemon crash, panicked task). Same grace the loop's tick
/// pipeline uses.
const PENDING_STALE_GRACE_MS: u64 = 60_000;

/// Delay before a continuation that lost the session run-slot (`AgentBusy`) is
/// retried. Well under `PENDING_STALE_GRACE_MS` so a re-armed continuation is
/// never treated as stale before it wakes. Mirrors the loop's busy retry.
const BUSY_RETRY_DELAY_MS: u64 = 30_000;

/// Outcome of [`GoalStore::try_claim_continuation`] — the single atomic decision
/// the post-run continuation hook acts on. Mirrors `looping::TickDecision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationDecision {
    /// The model self-reported `complete` on an autonomous goal and an objective
    /// gate is configured: the caller must run the gate and then call
    /// [`GoalStore::confirm_complete`] or [`GoalStore::reopen_after_gate_veto`].
    /// Nothing was written — gate arbitration owns the transition.
    AwaitingGate(Box<Goal>),
    /// A continuation was claimed: the iteration is spent, the pending marker is
    /// stamped. Spawn it with `delay_ms`; at fire time it must
    /// [`GoalStore::confirm_fire`] with `wake_ms` before executing.
    Fire {
        delay_ms: u64,
        wake_ms: u64,
        prompt: String,
    },
    /// A structural cap tripped: the goal was persisted as `Blocked` with `note`
    /// (returned so the caller can log / notify the origin channel).
    Exhausted { note: String },
    /// Nothing to do: no goal, passive goal, terminal goal, or a continuation is
    /// already in flight.
    Idle,
}

/// Outcome of [`GoalStore::rearm_after_busy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RearmDecision {
    /// Re-spawn the same continuation after `delay_ms`, confirming `wake_ms`.
    Retry { delay_ms: u64, wake_ms: u64 },
    /// A cap tripped while the collision played out: the goal was blocked with
    /// `note` (returned so the caller notifies the origin channel — R5).
    Exhausted { note: String },
    /// Goal gone, terminal, or already re-claimed — drop this continuation.
    Drop,
}

pub struct GoalStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl GoalStore {
    /// Open (creating if needed) the goal DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AlephError::other(e.to_string()))?;
        }
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::other(format!("goal store open: {e}")))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS goals (
                 session_id TEXT PRIMARY KEY,
                 json       TEXT NOT NULL
             )",
            [],
        )
        .map_err(|e| AlephError::other(format!("goal store init: {e}")))?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        // P7 lock-safety: never propagate poison.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Upsert the goal for its session (replaces any existing one).
    pub fn put(&self, goal: &Goal) -> Result<()> {
        let json = serde_json::to_string(goal)
            .map_err(|e| AlephError::other(format!("goal serialize: {e}")))?;
        self.lock()
            .execute(
                "INSERT INTO goals (session_id, json) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET json = excluded.json",
                rusqlite::params![goal.session_id, json],
            )
            .map_err(|e| AlephError::other(format!("goal put: {e}")))?;
        Ok(())
    }

    /// Fetch the goal for `session_id`, if any. A missing row is `Ok(None)`;
    /// corrupt JSON is also `Ok(None)` (fail-safe: a bad row must never wedge
    /// prompt assembly). Real DB errors propagate via `?` rather than being
    /// silently swallowed as "not found".
    pub fn get(&self, session_id: &str) -> Result<Option<Goal>> {
        use rusqlite::OptionalExtension;
        let conn = self.lock();
        let row: Option<String> = conn
            .query_row(
                "SELECT json FROM goals WHERE session_id = ?1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::other(format!("goal get: {e}")))?;
        Ok(row.and_then(|j| serde_json::from_str::<Goal>(&j).ok()))
    }

    /// Remove the standing goal for `session_id` (no-op if absent).
    pub fn delete(&self, session_id: &str) -> Result<()> {
        self.lock()
            .execute(
                "DELETE FROM goals WHERE session_id = ?1",
                rusqlite::params![session_id],
            )
            .map_err(|e| AlephError::other(format!("goal delete: {e}")))?;
        Ok(())
    }

    /// Read a goal under an already-held connection lock (the atomic-claim
    /// helpers below read → decide → write inside ONE guard).
    fn get_locked(conn: &rusqlite::Connection, session_id: &str) -> Result<Option<Goal>> {
        use rusqlite::OptionalExtension;
        let row: Option<String> = conn
            .query_row(
                "SELECT json FROM goals WHERE session_id = ?1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::other(format!("goal get: {e}")))?;
        Ok(row.and_then(|j| serde_json::from_str::<Goal>(&j).ok()))
    }

    /// Write a goal under an already-held connection lock (see [`Self::get_locked`]).
    fn put_locked(conn: &rusqlite::Connection, goal: &Goal) -> Result<()> {
        let json = serde_json::to_string(goal)
            .map_err(|e| AlephError::other(format!("goal serialize: {e}")))?;
        conn.execute(
            "INSERT INTO goals (session_id, json) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET json = excluded.json",
            rusqlite::params![goal.session_id, json],
        )
        .map_err(|e| AlephError::other(format!("goal put: {e}")))?;
        Ok(())
    }

    /// The continuation hook's whole post-run decision, under ONE lock guard:
    /// seed the token baseline, gate on an in-flight continuation, then either
    /// claim the next continuation (spend the iteration, stamp the pending
    /// marker) or block an exhausted goal with its reason. The read→await→write
    /// shape it replaces leaked iterations: a continuation that lost the session
    /// run-slot (`AgentBusy`) had already been counted, so every user message
    /// that raced a pursuit silently burned one autonomous step for zero work —
    /// and if that colliding run then failed (its own hook never runs), the goal
    /// stalled `Active` with nothing in flight. Mirrors
    /// `looping::LoopRegistry::try_claim_tick`.
    ///
    /// `tokens_total` is the session's live cumulative token count (`None` →
    /// unavailable, budget unenforced this round). `gate_configured` reflects the
    /// global `[[stop_hooks]]` gate OR a per-goal `gate_command`; when the model
    /// has self-reported `complete` under one, this returns
    /// [`ContinuationDecision::AwaitingGate`] and writes nothing — gate
    /// arbitration is the caller's (it is async, and must not run under the lock).
    pub fn try_claim_continuation(
        &self,
        session_id: &str,
        tokens_total: Option<u64>,
        now_ms: u64,
        gate_configured: bool,
    ) -> Result<ContinuationDecision> {
        let conn = self.lock();
        let Some(current) = Self::get_locked(&conn, session_id)? else {
            return Ok(ContinuationDecision::Idle);
        };

        // The model claimed completion and a gate must arbitrate it. Nothing to
        // write, and no baseline to seed (the goal is no longer Active).
        if pursuit::awaiting_gate(&current, gate_configured) {
            return Ok(ContinuationDecision::AwaitingGate(Box::new(current)));
        }

        // Lazy token-baseline capture on the first claim that sees a budget
        // (codex `tokenStartFresh`): just captured → 0 spent, never a false
        // over-budget. Only meaningful for an Active pursuit — the budget is
        // consumed by the continuation path alone.
        let goal = match (
            current.token_budget,
            current.baseline_captured,
            current.is_active(),
            tokens_total,
        ) {
            (Some(_), false, true, Some(total)) => current.clone().with_baseline(total, now_ms),
            _ => current.clone(),
        };
        let baseline_seeded = goal.baseline_captured != current.baseline_captured;
        // Budget enforcement needs the live total; without one (or without a
        // budget at all) pass 0 so only the iteration/deadline caps apply.
        let tokens_now = if goal.token_budget.is_some() {
            tokens_total.unwrap_or(0)
        } else {
            0
        };

        // A continuation already in flight blocks another claim — the fan-out
        // gate. Past the stale grace its task is presumed dead and the claim
        // proceeds (that task's own `confirm_fire` will then mismatch and skip).
        if let Some(wake) = goal.pending_continuation_ms {
            if now_ms < wake.saturating_add(PENDING_STALE_GRACE_MS) {
                if baseline_seeded {
                    Self::put_locked(&conn, &goal)?; // persist the seeded baseline
                }
                return Ok(ContinuationDecision::Idle);
            }
        }

        if pursuit::should_continue(&goal, tokens_now, now_ms) {
            let prompt = pursuit::continuation_prompt(&goal, tokens_now, now_ms);
            // Immediate: a goal continuation has no cadence to wait for (unlike a
            // loop tick), so the claim's wake IS now.
            let wake_ms = now_ms;
            // Spend the iteration BEFORE the run so the cap holds even if the
            // continuation crashes before re-entering the hook.
            Self::put_locked(
                &conn,
                &goal
                    .spent_continuation(now_ms)
                    .with_pending_continuation(Some(wake_ms)),
            )?;
            Ok(ContinuationDecision::Fire {
                delay_ms: 0,
                wake_ms,
                prompt,
            })
        } else if pursuit::exhausted_while_active(&goal, tokens_now, now_ms) {
            let note = pursuit::stop_reason_note(&goal, tokens_now, now_ms);
            Self::put_locked(
                &conn,
                &goal
                    .with_status(GoalStatus::Blocked, now_ms)
                    .with_note(Some(note.clone()), now_ms)
                    .with_pending_continuation(None),
            )?;
            Ok(ContinuationDecision::Exhausted { note })
        } else {
            if baseline_seeded {
                Self::put_locked(&conn, &goal)?;
            }
            Ok(ContinuationDecision::Idle)
        }
    }

    /// True when the live row is still the goal we gated, still awaiting that
    /// gate's verdict. The compare-and-swap predicate behind both post-gate
    /// writes (codex's `expected_goal_id` CAS): the gate is a shell command that
    /// runs for tens of seconds with the session's run slot FREE, so the user can
    /// clear the goal (row deleted), set a new objective (new id), or the model
    /// can move it on — and a blind write-back of the pre-gate snapshot would
    /// resurrect the dead goal and spawn an unattended run for it.
    fn still_awaiting_gate(live: &Goal, expected: &Goal) -> bool {
        live.id == expected.id
            && live.status == GoalStatus::Complete
            && live.gate_outcome == crate::goal::GateOutcome::Unchecked
    }

    /// Commit an objective-gate verdict that ends the pursuit (confirmed complete,
    /// or vetoed with no runway left), guarded by [`Self::still_awaiting_gate`].
    /// `false` = the goal changed under the gate and the verdict was discarded.
    /// Clears any pending marker so no in-flight continuation can resurrect it.
    pub fn commit_gate_pass(&self, expected: &Goal, next: &Goal) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, &expected.session_id)? {
            Some(live) if Self::still_awaiting_gate(&live, expected) => {
                Self::put_locked(&conn, &next.clone().with_pending_continuation(None))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Claim a continuation for a goal an objective gate just reopened (the caller
    /// supplies the gate-failure prompt). Same CAS guard as
    /// [`Self::commit_gate_pass`], same accounting as
    /// [`Self::try_claim_continuation`]'s `Fire` — spend the iteration, stamp the
    /// pending marker — in one guard. Returns the `wake_ms` the spawned run must
    /// [`Self::confirm_fire`] with, or `None` when the goal changed under the gate.
    pub fn claim_after_gate_veto(
        &self,
        expected: &Goal,
        reopened: &Goal,
        now_ms: u64,
    ) -> Result<Option<u64>> {
        let conn = self.lock();
        match Self::get_locked(&conn, &expected.session_id)? {
            Some(live) if Self::still_awaiting_gate(&live, expected) => {
                Self::put_locked(
                    &conn,
                    &reopened
                        .clone()
                        .spent_continuation(now_ms)
                        .with_pending_continuation(Some(now_ms)),
                )?;
                Ok(Some(now_ms))
            }
            _ => Ok(None),
        }
    }

    /// Block a goal that is still being actively pursued (a continuation run
    /// failed), in one guard: never clobber a goal the failed run had already
    /// marked complete/blocked, or one the user cleared meanwhile. Clears the
    /// pending marker so no in-flight continuation resurrects it. `false` = there
    /// was nothing left to block.
    pub fn block_if_active(&self, session_id: &str, note: &str, now_ms: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(live) if live.is_active() => {
                Self::put_locked(
                    &conn,
                    &live
                        .with_status(GoalStatus::Blocked, now_ms)
                        .with_note(Some(note.to_string()), now_ms)
                        .with_pending_continuation(None),
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Fire-time gate for a claimed continuation: proceed only if the goal is
    /// still `Active` and THIS continuation is still the one on the books
    /// (`wake_ms` matches the pending marker), clearing the marker in the same
    /// guard. `false` = superseded (the user cleared/completed the goal, or a
    /// stale-grace re-claim replaced this one) — the run must NOT execute. This
    /// is what keeps a re-armed continuation from burning a stale LLM turn on a
    /// goal that ended during its retry delay.
    pub fn confirm_fire(&self, session_id: &str, wake_ms: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(g) if g.is_active() && g.pending_continuation_ms == Some(wake_ms) => {
                Self::put_locked(&conn, &g.with_pending_continuation(None))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Re-arm a continuation that lost the session run-slot (`AgentBusy`): the
    /// iteration was already spent at claim time, so this re-stamps the pending
    /// marker with a short retry delay WITHOUT spending another one, and the
    /// caller re-spawns the same prompt. Without it a busy collision either
    /// burned the iteration for nothing (the old skip) or permanently blocked the
    /// user's goal (the older still). Mirrors `LoopRegistry::rearm_after_busy`.
    pub fn rearm_after_busy(&self, session_id: &str, now_ms: u64) -> Result<RearmDecision> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(g) if g.is_active() && g.pending_continuation_ms.is_none() => {
                // The wall clock may have run out while the collision played out;
                // the token budget is claim-side only (no live counter here).
                if pursuit::exhausted_while_active(&g, 0, now_ms) {
                    let note = pursuit::stop_reason_note(&g, 0, now_ms);
                    Self::put_locked(
                        &conn,
                        &g.with_status(GoalStatus::Blocked, now_ms)
                            .with_note(Some(note.clone()), now_ms),
                    )?;
                    return Ok(RearmDecision::Exhausted { note });
                }
                let wake_ms = now_ms.saturating_add(BUSY_RETRY_DELAY_MS);
                Self::put_locked(&conn, &g.with_pending_continuation(Some(wake_ms)))?;
                Ok(RearmDecision::Retry {
                    delay_ms: BUSY_RETRY_DELAY_MS,
                    wake_ms,
                })
            }
            // Goal gone / terminal, or another continuation was claimed meanwhile.
            _ => Ok(RearmDecision::Drop),
        }
    }

    /// Commit a tool-side field update (status / caps / budget / deadline / gate /
    /// lessons / note) under ONE guard, re-reading the fields the CLAIM pipeline
    /// owns and keeping the live ones.
    ///
    /// Ownership split — the tool owns what the user can ask for; the claim
    /// pipeline owns the pursuit's accounting (`continuations_used`, the token
    /// baseline) and its scheduling marker (`pending_continuation_ms`). The tool
    /// computes `next` from a `get` snapshot, so any of those three that moved in
    /// the read→write gap (a claimed continuation firing, a busy re-arm) would be
    /// silently rolled back by a plain `put`: a resurrected stale marker stalls
    /// the next claim for the full 60s stale grace, and a rolled-back counter
    /// hands the pursuit iterations it already spent. Merging by owner makes that
    /// unrepresentable (loop's `commit_field_update` parity, one field wider).
    ///
    /// Returns `false` — a no-op — when the goal vanished since the read, so the
    /// caller reports that honestly instead of silently re-creating it.
    pub fn commit_field_update(&self, next: &Goal) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, &next.session_id)? {
            Some(live) => {
                let mut merged = next.clone();
                merged.pending_continuation_ms = live.pending_continuation_ms;
                merged.continuations_used = live.continuations_used;
                merged.tokens_at_start = live.tokens_at_start;
                merged.baseline_captured = live.baseline_captured;
                Self::put_locked(&conn, &merged)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Enumerate all stored goals (one row per session). Corrupt rows are
    /// skipped (fail-safe, mirroring `get`). Used by the dream lessons-promotion
    /// stage to sweep lessons into long-term memory.
    pub fn list_all(&self) -> Result<Vec<Goal>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT json FROM goals")
            .map_err(|e| AlephError::other(format!("goal list_all prepare: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::other(format!("goal list_all query: {e}")))?;
        let mut goals = Vec::new();
        for row in rows {
            let json = row.map_err(|e| AlephError::other(format!("goal list_all row: {e}")))?;
            if let Ok(goal) = serde_json::from_str::<Goal>(&json) {
                goals.push(goal); // corrupt rows skipped, like `get`.
            }
        }
        Ok(goals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::types::{Goal, GoalStatus};

    fn temp_store() -> (GoalStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::open(&dir.path().join("goals.db")).unwrap();
        (store, dir)
    }

    #[test]
    fn put_get_roundtrip() {
        let (store, _d) = temp_store();
        let g = Goal::new("sess-1", "Do the thing", 0, 0);
        store.put(&g).unwrap();
        let got = store.get("sess-1").unwrap().unwrap();
        assert_eq!(got.objective, "Do the thing");
        assert_eq!(got.status, GoalStatus::Active);
    }

    #[test]
    fn put_replaces_existing_for_same_session() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "first", 0, 0)).unwrap();
        store.put(&Goal::new("sess-1", "second", 0, 0)).unwrap();
        let got = store.get("sess-1").unwrap().unwrap();
        assert_eq!(got.objective, "second", "one active goal per session");
    }

    #[test]
    fn get_missing_is_none() {
        let (store, _d) = temp_store();
        assert!(store.get("nope").unwrap().is_none());
    }

    #[test]
    fn delete_removes_row() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "x", 0, 0)).unwrap();
        store.delete("sess-1").unwrap();
        assert!(store.get("sess-1").unwrap().is_none());
    }

    #[test]
    fn list_all_returns_every_session_goal() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "a", 0, 0)).unwrap();
        store.put(&Goal::new("sess-2", "b", 0, 0)).unwrap();
        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 2);
        let mut objs: Vec<&str> = all.iter().map(|g| g.objective.as_str()).collect();
        objs.sort_unstable();
        assert_eq!(objs, vec!["a", "b"]);
    }

    #[test]
    fn list_all_empty_when_no_goals() {
        let (store, _d) = temp_store();
        assert!(store.list_all().unwrap().is_empty());
    }

    // ---- the atomic continuation claim -------------------------------------

    use crate::goal::types::{GateOutcome, PursuitMode};

    fn pursuing(session: &str, max: u32) -> Goal {
        Goal::new(session, "obj", 0, 0).with_pursuit(PursuitMode::Active {
            max_iterations: max,
        })
    }

    #[test]
    fn claim_fires_once_then_gates_until_the_continuation_lands() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();

        let first = store
            .try_claim_continuation("s", None, 1_000, false)
            .unwrap();
        let ContinuationDecision::Fire { wake_ms, .. } = first else {
            panic!("expected Fire, got {first:?}");
        };
        assert_eq!(store.get("s").unwrap().unwrap().continuations_used, 1);

        // A second run completing while that continuation is in flight must NOT
        // claim another one (and must not spend another iteration).
        assert_eq!(
            store
                .try_claim_continuation("s", None, 1_100, false)
                .unwrap(),
            ContinuationDecision::Idle
        );
        assert_eq!(store.get("s").unwrap().unwrap().continuations_used, 1);

        // Once the claimed continuation fires, the chain is free to continue.
        assert!(store.confirm_fire("s", wake_ms).unwrap());
        assert!(matches!(
            store
                .try_claim_continuation("s", None, 1_200, false)
                .unwrap(),
            ContinuationDecision::Fire { .. }
        ));
        assert_eq!(store.get("s").unwrap().unwrap().continuations_used, 2);
    }

    #[test]
    fn a_dead_claim_self_heals_past_the_stale_grace() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();
        let ContinuationDecision::Fire { wake_ms, .. } = store
            .try_claim_continuation("s", None, 1_000, false)
            .unwrap()
        else {
            panic!("expected Fire");
        };
        // The spawned task died (daemon crash / panic) — its marker would block
        // the pursuit forever without the grace.
        let past_grace = wake_ms + PENDING_STALE_GRACE_MS + 1;
        assert!(matches!(
            store
                .try_claim_continuation("s", None, past_grace, false)
                .unwrap(),
            ContinuationDecision::Fire { .. }
        ));
        // …and the dead task's own fire is then refused (its wake is superseded).
        assert!(!store.confirm_fire("s", wake_ms).unwrap());
    }

    #[test]
    fn confirm_fire_refuses_a_goal_that_ended_during_the_delay() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();
        let ContinuationDecision::Fire { wake_ms, .. } = store
            .try_claim_continuation("s", None, 1_000, false)
            .unwrap()
        else {
            panic!("expected Fire");
        };
        // User cleared the goal while the continuation waited out a busy retry.
        store.delete("s").unwrap();
        assert!(
            !store.confirm_fire("s", wake_ms).unwrap(),
            "a ghost run must not execute against a goal that no longer exists"
        );
    }

    #[test]
    fn rearm_after_busy_retries_the_same_step_without_spending_another() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();
        let ContinuationDecision::Fire { wake_ms, .. } = store
            .try_claim_continuation("s", None, 1_000, false)
            .unwrap()
        else {
            panic!("expected Fire");
        };
        assert!(store.confirm_fire("s", wake_ms).unwrap()); // fired, then AgentBusy
        let decision = store.rearm_after_busy("s", 2_000).unwrap();
        let RearmDecision::Retry { delay_ms, wake_ms } = decision else {
            panic!("expected Retry, got {decision:?}");
        };
        assert_eq!(delay_ms, BUSY_RETRY_DELAY_MS);
        let g = store.get("s").unwrap().unwrap();
        assert_eq!(g.continuations_used, 1, "the retry must not cost a step");
        assert_eq!(g.pending_continuation_ms, Some(wake_ms));
        // The re-armed step is gated against fan-out exactly like a fresh claim.
        assert_eq!(
            store
                .try_claim_continuation("s", None, 2_100, false)
                .unwrap(),
            ContinuationDecision::Idle
        );
    }

    #[test]
    fn exhausted_claim_blocks_the_goal_with_its_reason() {
        let (store, _d) = temp_store();
        let mut g = pursuing("s", 2);
        g.continuations_used = 2;
        store.put(&g).unwrap();
        let ContinuationDecision::Exhausted { note } = store
            .try_claim_continuation("s", None, 1_000, false)
            .unwrap()
        else {
            panic!("expected Exhausted");
        };
        assert!(note.contains("iteration cap"));
        let stored = store.get("s").unwrap().unwrap();
        assert_eq!(stored.status, GoalStatus::Blocked);
        assert_eq!(stored.note.as_deref(), Some(note.as_str()));
    }

    #[test]
    fn claim_returns_awaiting_gate_without_writing_anything() {
        let (store, _d) = temp_store();
        let g = pursuing("s", 5).with_status(GoalStatus::Complete, 1);
        store.put(&g).unwrap();
        let d = store
            .try_claim_continuation("s", None, 1_000, true)
            .unwrap();
        assert!(matches!(d, ContinuationDecision::AwaitingGate(_)));
        // Gate arbitration owns the transition — the claim must not have touched
        // the row (a spurious bump on a terminal goal, or a stolen iteration).
        assert_eq!(store.get("s").unwrap().unwrap(), g);
    }

    #[test]
    fn gate_verdicts_are_discarded_when_the_goal_changed_under_the_gate() {
        let (store, _d) = temp_store();
        let gated = pursuing("s", 5).with_status(GoalStatus::Complete, 1);
        store.put(&gated).unwrap();

        // The gate is a shell command: while it runs (tens of seconds, run slot
        // free) the user replaces the objective.
        let replacement = Goal::new("s", "something else entirely", 0, 5);
        store.put(&replacement).unwrap();

        let confirmed = gated.clone().with_gate_outcome(GateOutcome::Passed, 9);
        assert!(
            !store.commit_gate_pass(&gated, &confirmed).unwrap(),
            "a stale gate pass must not overwrite the user's new goal"
        );
        assert!(
            store
                .claim_after_gate_veto(&gated, &gated, 9)
                .unwrap()
                .is_none(),
            "a stale gate veto must not resurrect the old goal or spawn a run"
        );
        assert_eq!(store.get("s").unwrap().unwrap(), replacement);
    }

    #[test]
    fn gate_pass_commits_when_the_goal_is_untouched() {
        let (store, _d) = temp_store();
        let gated = pursuing("s", 5).with_status(GoalStatus::Complete, 1);
        store.put(&gated).unwrap();
        let confirmed = gated.clone().with_gate_outcome(GateOutcome::Passed, 9);
        assert!(store.commit_gate_pass(&gated, &confirmed).unwrap());
        assert_eq!(
            store.get("s").unwrap().unwrap().gate_outcome,
            GateOutcome::Passed
        );
    }

    #[test]
    fn commit_field_update_keeps_the_live_pending_marker() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();
        // The tool reads a snapshot…
        let snapshot = store.get("s").unwrap().unwrap();
        assert_eq!(snapshot.pending_continuation_ms, None);
        // …a continuation is claimed in the gap…
        let ContinuationDecision::Fire { wake_ms, .. } = store
            .try_claim_continuation("s", None, 1_000, false)
            .unwrap()
        else {
            panic!("expected Fire");
        };
        // …and the tool writes its (stale) snapshot back.
        let edited = snapshot.with_note(Some("user note".into()), 2_000);
        assert!(store.commit_field_update(&edited).unwrap());
        let live = store.get("s").unwrap().unwrap();
        assert_eq!(live.note.as_deref(), Some("user note"), "the edit landed");
        assert_eq!(
            live.pending_continuation_ms,
            Some(wake_ms),
            "the live claim must survive a tool write"
        );
        assert_eq!(live.continuations_used, 1, "…and so must its accounting");
    }

    #[test]
    fn commit_field_update_reports_a_goal_cleared_under_it() {
        let (store, _d) = temp_store();
        let g = pursuing("s", 5);
        store.put(&g).unwrap();
        store.delete("s").unwrap();
        assert!(!store.commit_field_update(&g).unwrap());
        assert!(
            store.get("s").unwrap().is_none(),
            "a cleared goal must not be re-created by a losing update"
        );
    }

    #[test]
    fn block_if_active_never_clobbers_a_terminal_goal() {
        let (store, _d) = temp_store();
        let done = pursuing("s", 5).with_status(GoalStatus::Complete, 1);
        store.put(&done).unwrap();
        assert!(!store.block_if_active("s", "boom", 9).unwrap());
        assert_eq!(
            store.get("s").unwrap().unwrap().status,
            GoalStatus::Complete
        );

        store.put(&pursuing("s", 5)).unwrap();
        assert!(store.block_if_active("s", "boom", 9).unwrap());
        let g = store.get("s").unwrap().unwrap();
        assert_eq!(g.status, GoalStatus::Blocked);
        assert_eq!(g.note.as_deref(), Some("boom"));
    }
}
