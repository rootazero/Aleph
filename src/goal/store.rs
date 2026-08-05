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

/// How long an in-flight objective-gate arbitration suppresses a second one.
/// The gate is a shell command the caller runs with the session run-slot free,
/// so a user turn completing inside its window re-enters the hook; the pending
/// marker dedups it. Unlike a Fire continuation (an LLM turn, bounded by
/// `PENDING_STALE_GRACE_MS`), a gate can be a `cargo test`/`just verify`-class
/// command lasting minutes, so it gets its own longer grace before a presumed-
/// dead gate task is re-arbitrated — 15 minutes, since a full-repo verify on a
/// large workspace legitimately outlasts the old 5-minute value and a
/// re-entering hook would otherwise spawn a SECOND concurrent gate (the CAS
/// write-back makes that wasteful, not corrupting, but the dedup promise is
/// the point of the marker). A marker stranded by a mid-gate status
/// change still self-heals at the shorter Fire-path grace once the goal leaves
/// `Complete`.
const GATE_STALE_GRACE_MS: u64 = 900_000;

/// Outcome of [`GoalStore::try_claim_continuation`] — the single atomic decision
/// the post-run continuation hook acts on. Mirrors `looping::TickDecision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationDecision {
    /// The model self-reported `complete` on an autonomous goal and an objective
    /// gate is configured: the caller must run the gate and then commit the
    /// verdict via [`GoalStore::commit_gate_pass`] (passed, or vetoed with no
    /// runway left) or [`GoalStore::claim_after_gate_veto`] (vetoed, runway left).
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

/// Outcome of [`GoalStore::commit_field_update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldUpdate {
    /// The update committed as computed (live status still matched the
    /// snapshot the caller derived the update from).
    Committed,
    /// The goal vanished between the caller's read and the write — nothing
    /// was persisted.
    Gone,
    /// The non-lifecycle fields committed, but a concurrent lifecycle
    /// transition won the read→write gap; the caller's status write (and its
    /// lifecycle companions) was superseded by the live status carried here.
    StatusSuperseded(GoalStatus),
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
        // One-shot "goal settled" watcher-notification markers (see
        // `try_claim_settle_notify`). One row per session; a replaced or
        // re-completed goal overwrites its session's row, so the table stays
        // bounded by the number of sessions with goals.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS settle_notified (
                 session_id TEXT PRIMARY KEY,
                 stamp      TEXT NOT NULL
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
    ///
    /// `workspace` is the claiming run's project root, recorded on the goal so
    /// later hook-less wakes can rebuild the continuation in the same project.
    /// `None` means "no workspace information" (the wake service re-claiming a
    /// goal it only knows from the store) — the stored value is KEPT, so the
    /// wake pipeline never wipes what the post-run hook recorded.
    pub fn try_claim_continuation(
        &self,
        session_id: &str,
        tokens_total: Option<u64>,
        now_ms: u64,
        gate_configured: bool,
        workspace: Option<&str>,
    ) -> Result<ContinuationDecision> {
        let conn = self.lock();
        let Some(current) = Self::get_locked(&conn, session_id)? else {
            return Ok(ContinuationDecision::Idle);
        };

        // The model claimed completion and a gate must arbitrate it. Gate
        // arbitration is an async shell command the caller runs with the session
        // run-slot FREE, so a user turn completing inside that window re-enters
        // this hook — and without a guard each re-entry would launch ANOTHER
        // concurrent gate command (the `still_awaiting_gate` CAS only dedups the
        // write-BACK, not the command execution). Reuse the pending marker as an
        // in-flight gate guard (no new mechanism): stamp it on the first claim,
        // and while it is fresh return `Idle` so no second gate runs; past
        // `GATE_STALE_GRACE_MS` the gate task is presumed dead and we re-arbitrate.
        // No baseline to seed — the goal is Complete, not Active.
        if pursuit::awaiting_gate(&current, gate_configured) {
            if let Some(started) = current.pending_continuation_ms {
                if now_ms < started.saturating_add(GATE_STALE_GRACE_MS) {
                    return Ok(ContinuationDecision::Idle);
                }
            }
            let claimed = current.with_pending_continuation(Some(now_ms));
            Self::put_locked(&conn, &claimed)?;
            return Ok(ContinuationDecision::AwaitingGate(Box::new(claimed)));
        }

        // Lazy self-clear of an ELAPSED deadline barrier (hermes parity): the
        // barrier is satisfied, so drop the stale wait fields on this claim's
        // write path instead of carrying them into renders forever. An
        // unsatisfied barrier is handled below, after the in-flight gate.
        let current = if current
            .waiting_until_ms
            .is_some_and(|until| now_ms != 0 && until <= now_ms)
        {
            current.without_wait(now_ms)
        } else {
            current
        };

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
        // Record the claiming run's project root so a hook-less wake (task
        // settle / boot re-arm / periodic recheck) can rebuild the
        // continuation in the same workspace. `None` = the claim carries no
        // workspace info (wake-service claims) → keep the stored value.
        let goal = match workspace {
            Some(ws) if goal.workspace.as_deref() != Some(ws) => {
                goal.with_workspace(Some(ws.to_string()))
            }
            _ => goal,
        };
        let dirty = goal.baseline_captured != current.baseline_captured
            || goal.workspace != current.workspace;
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
        // A claimed wait-barrier TIMER also parks here: its wake is in the
        // future, so `now < wake + grace` holds for the whole park and no
        // second claim can race it.
        if let Some(wake) = goal.pending_continuation_ms {
            if now_ms < wake.saturating_add(PENDING_STALE_GRACE_MS) {
                if dirty {
                    Self::put_locked(&conn, &goal)?; // persist the seeded baseline
                }
                return Ok(ContinuationDecision::Idle);
            }
        }

        // Wait barrier: a parked pursuit claims its WAKE instead of its next
        // step (R7 — the model chose to park; this is pure clock/event
        // mechanics). A deadline barrier arms an exact timer through the very
        // same Fire machinery a normal continuation uses (confirm_fire CAS,
        // busy re-arm, stale-grace self-heal all apply unchanged); the wake
        // run spends one iteration like every autonomous run, so parking can
        // never smuggle extra runway past the R5 cap. A task barrier stays
        // Idle — the `GoalWakeService` clears it on the task's settle event
        // (boot recheck covers restarts). Wakes with no runway left stay Idle
        // too: exhaustion is arbitrated when the barrier clears, so a parked
        // goal is never Blocked while legitimately waiting.
        if let Some(park) = pursuit::wait_parked(&goal, now_ms) {
            if let pursuit::WaitPark::Timer { wake_ms } = park {
                if pursuit::should_continue(&goal, tokens_now, now_ms) {
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
                // Timer parked with no runway left (iteration/deadline/token
                // cap already spent): arming the timer would only wake into an
                // immediate Block, so arbitrate exhaustion NOW instead of
                // parking silently forever. Same transition as the unparked
                // exhausted path below; clears the barrier + pending marker.
                if pursuit::exhausted_while_active(&goal, tokens_now, now_ms) {
                    let note = pursuit::stop_reason_note(&goal, tokens_now, now_ms);
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
            }
            // Task barrier (woken externally by GoalWakeService), or a timer
            // barrier that is neither runnable nor exhausted (e.g. status not
            // Active): stay parked. Exhaustion for a task barrier is arbitrated
            // when the barrier clears.
            if dirty {
                Self::put_locked(&conn, &goal)?;
            }
            return Ok(ContinuationDecision::Idle);
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
            if dirty {
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

    /// One-shot claim of the "goal settled" watcher notification for this
    /// goal's completion write — the caller pokes `loop_graph` watchers only on
    /// `true`. Mirrors the [`Self::commit_gate_pass`] CAS discipline: the
    /// gate-less Idle arm of the continuation hook re-observes the same
    /// terminal `Complete` row on EVERY later turn of the session, and without
    /// this transition guard each of those turns re-fires a full watcher cron
    /// run (the 60s debounce is the only damper). Keyed on the goal's
    /// `(id, completed_at_ms)` — the instant of the transition INTO
    /// `Complete`, NOT the any-write `updated_at_ms`, so post-completion
    /// field edits (lesson/note appends on a still-Complete goal) cannot
    /// mint a fresh claim. A resumed-then-re-completed goal gets a new
    /// completion instant and fires again. The goal `id` is deterministic
    /// per `(session, objective)` (a same-objective re-set keeps the id), so
    /// the timestamp half of the stamp carries the uniqueness. Legacy rows
    /// persisted before `completed_at_ms` fall back to `updated_at_ms`.
    /// Stale rows for cleared goals are harmless: the next completion's
    /// stamp always differs.
    pub fn try_claim_settle_notify(&self, goal: &Goal) -> Result<bool> {
        let changed = self
            .lock()
            .execute(
                "INSERT INTO settle_notified (session_id, stamp) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET stamp = excluded.stamp
                 WHERE stamp != excluded.stamp",
                rusqlite::params![goal.session_id, settle_stamp(goal)],
            )
            .map_err(|e| AlephError::other(format!("goal settle-notify claim: {e}")))?;
        Ok(changed > 0)
    }

    /// Give back a claim from [`Self::try_claim_settle_notify`] that turned out
    /// not to have poked anything.
    ///
    /// The claim is one-shot and the stamp for a still-`Complete` goal never
    /// moves, so spending it on a poke that did not happen (no cron handle
    /// attached yet, `run_job` refused, watcher not pokeable) retired the
    /// victory claim **permanently** for that completion — the paired watcher
    /// never reviewed it, and the inner 60s debounce rollback bought nothing
    /// because no second notification for that stamp could ever occur. Scoped
    /// to our own stamp so a newer completion's claim is never erased; the next
    /// observation of the same terminal row then re-claims and retries.
    pub fn release_settle_notify(&self, goal: &Goal) -> Result<bool> {
        let deleted = self
            .lock()
            .execute(
                "DELETE FROM settle_notified WHERE session_id = ?1 AND stamp = ?2",
                rusqlite::params![goal.session_id, settle_stamp(goal)],
            )
            .map_err(|e| AlephError::other(format!("goal settle-notify release: {e}")))?;
        Ok(deleted > 0)
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
    /// pending marker so no in-flight continuation resurrects it, and drops any
    /// wait barrier — a blocked goal is no longer waiting on anything (the wake
    /// service only ever looks at Active goals, so the leftover barrier could
    /// only render as a self-contradictory "parked (waiting)" on a Blocked row).
    /// `false` = there was nothing left to block.
    pub fn block_if_active(&self, session_id: &str, note: &str, now_ms: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(live) if live.is_active() => {
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

    /// Like [`Self::block_if_active`], but scoped to goals whose recovery
    /// actually hung on a crashed autonomous run — used by
    /// `ResumeCoordinator::abandon`. Blocks ONLY an Active-pursuit goal that is
    /// NOT parked on ANY wait barrier: a passive/interactive goal never depends
    /// on the continuation chain (its "recovery" is the user talking again),
    /// and a parked goal — task-barrier OR deadline-timer — is woken by
    /// `GoalWakeService` (`rearm_parked_goals` replays the stored timer marker
    /// on boot), not by the abandoned run. Blocking either would wrongly kill a
    /// healthy pursuit: the wake service filters on `is_active()`, so a blocked
    /// parked goal would be skipped forever with its barrier stranded.
    /// `false` = nothing eligible to block.
    pub fn block_if_abandonable(&self, session_id: &str, note: &str, now_ms: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(live)
                if live.is_active()
                    && matches!(live.pursuit, crate::goal::PursuitMode::Active { .. })
                    && !live.has_wait_barrier() =>
            {
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

    /// Pause a goal that is still being actively pursued, in one guard — the
    /// REVERSIBLE sibling of [`Self::block_if_active`]. Same atomicity and the
    /// same "never clobber a terminal goal" contract; the difference is intent:
    /// `Blocked` means the pursuit hit something it cannot get past and wants
    /// user guidance, `Paused` means a human is holding it deliberately and
    /// will resume it. Clears the pending-continuation marker so no in-flight
    /// continuation resurrects it, exactly as blocking does, and drops any wait
    /// barrier (a held pursuit is not waiting; resume re-parks explicitly if
    /// still needed). `false` = there was nothing active to pause.
    pub fn pause_if_active(&self, session_id: &str, note: &str, now_ms: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(live) if live.is_active() => {
                Self::put_locked(
                    &conn,
                    &live
                        .with_status(GoalStatus::Paused, now_ms)
                        .with_note(Some(note.to_string()), now_ms)
                        .without_wait(now_ms)
                        .with_pending_continuation(None),
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Kill switch: pause every actively-pursued goal across ALL sessions under
    /// ONE lock guard, returning the session keys actually paused. The bulk
    /// counterpart to [`Self::list_all`] and the goal-side twin of
    /// `LoopRegistry::stop_all`: a goal set on one channel was visible from
    /// anywhere (`goal(action='list')`) but only stoppable from its own session,
    /// so "quiet everything" was not expressible in conversation (R6 / R8).
    ///
    /// Pause, not clear: incident response wants the pursuits held, not their
    /// objectives and accumulated lessons destroyed. Each owner session resumes
    /// its own with `goal(action='update', status='active')`.
    pub fn pause_all_active(&self, note: &str, now_ms: u64) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT json FROM goals")
            .map_err(|e| AlephError::other(format!("goal pause_all prepare: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::other(format!("goal pause_all query: {e}")))?;
        let mut active = Vec::new();
        for row in rows {
            let json = row.map_err(|e| AlephError::other(format!("goal pause_all row: {e}")))?;
            // Corrupt rows are skipped, like `get` / `list_all` (fail-safe).
            if let Ok(goal) = serde_json::from_str::<Goal>(&json) {
                if goal.is_active() {
                    active.push(goal);
                }
            }
        }
        drop(stmt);
        let mut paused = Vec::with_capacity(active.len());
        for goal in active {
            let session = goal.session_id.clone();
            Self::put_locked(
                &conn,
                &goal
                    .with_status(GoalStatus::Paused, now_ms)
                    .with_note(Some(note.to_string()), now_ms)
                    .without_wait(now_ms)
                    .with_pending_continuation(None),
            )?;
            paused.push(session);
        }
        Ok(paused)
    }

    /// Deactivation freeze (spec §10): pause every actively-pursued goal
    /// OWNED BY `user_id`, under ONE lock guard. The scoped sibling of
    /// [`Self::pause_all_active`] — same full-scan+filter shape, narrowed by
    /// [`crate::goal::types::Goal::owner_user_id`] instead of taking
    /// everything. Returns the count paused.
    ///
    /// The predicate is exact equality against `Some(user_id)` — a legacy
    /// goal with `owner_user_id: None` belongs to the platform owner (spec
    /// §10: the owner account can never be deactivated), never to the user
    /// being deactivated here, so it is left untouched by construction. This
    /// is a one-way freeze: reactivating the user does not auto-resume its
    /// goals (spec is silent on auto-resume; each is a deliberate design
    /// choice recorded here rather than a client-side fallback guess).
    pub fn pause_all_owned_by(&self, user_id: &str) -> Result<usize> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT json FROM goals")
            .map_err(|e| AlephError::other(format!("goal pause_all_owned_by prepare: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::other(format!("goal pause_all_owned_by query: {e}")))?;
        let mut targets = Vec::new();
        for row in rows {
            let json =
                row.map_err(|e| AlephError::other(format!("goal pause_all_owned_by row: {e}")))?;
            // Corrupt rows are skipped, like `get` / `pause_all_active` (fail-safe).
            if let Ok(goal) = serde_json::from_str::<Goal>(&json) {
                if goal.is_active() && goal.owner_user_id.as_deref() == Some(user_id) {
                    targets.push(goal);
                }
            }
        }
        drop(stmt);
        let note = format!("Account '{user_id}' was deactivated — pursuit frozen.");
        let count = targets.len();
        for goal in targets {
            Self::put_locked(
                &conn,
                &goal
                    .with_status(GoalStatus::Paused, now_ms)
                    .with_note(Some(note.clone()), now_ms)
                    .without_wait(now_ms)
                    .with_pending_continuation(None),
            )?;
        }
        Ok(count)
    }

    /// Enroll a delegation session in the goal's shared token budget (tree
    /// budget v1), in one guard. First writer wins per member (the join-time
    /// baseline must not be re-stamped by a later delegation to the same
    /// session — that would erase spend already accrued); capped at
    /// [`crate::goal::types::MAX_BUDGET_MEMBERS`]. `false` = not enrolled
    /// (no goal / not active / no budget / self / cap reached) — the
    /// delegation itself proceeds regardless, only the accounting is skipped.
    pub fn register_budget_member(
        &self,
        owner_session: &str,
        member_session: &str,
        member_tokens_now: u64,
        now_ms: u64,
    ) -> Result<bool> {
        use crate::goal::types::{BudgetMember, MAX_BUDGET_MEMBERS};
        if owner_session == member_session {
            return Ok(false); // own session is already the tree's base term.
        }
        let conn = self.lock();
        match Self::get_locked(&conn, owner_session)? {
            Some(mut g) if g.is_active() && g.token_budget.is_some() => {
                if g.budget_members
                    .iter()
                    .any(|m| m.session_id == member_session)
                {
                    return Ok(true); // already enrolled — keep its original baseline.
                }
                if g.budget_members.len() >= MAX_BUDGET_MEMBERS {
                    tracing::warn!(
                        session = %owner_session,
                        cap = MAX_BUDGET_MEMBERS,
                        "goal tree budget: member cap reached; further delegations unaccounted"
                    );
                    return Ok(false);
                }
                g.budget_members.push(BudgetMember {
                    session_id: member_session.to_string(),
                    tokens_at_join: member_tokens_now,
                });
                g.updated_at_ms = now_ms;
                Self::put_locked(&conn, &g)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Supersede a claimed WAIT TIMER whose armed continuation is now stale
    /// (the user explicitly un-parked via `status='active'`, or re-parked with
    /// a fresh `wait_minutes`). Clears `pending_continuation_ms` ONLY when it
    /// still equals `armed_wake` — so the detached sleeping timer task's
    /// `confirm_fire` mismatches and skips, and the un-parking run's own
    /// `post_run` can claim a fresh continuation immediately instead of being
    /// stalled behind the old in-flight marker until the original wake. The
    /// equality guard is what keeps it from ever touching a NORMAL in-flight
    /// continuation (whose `wake_ms` is `now`, not the future barrier
    /// instant). `false` = no stale timer to supersede (never claimed, or
    /// already moved on). Idempotent and lock-atomic.
    pub fn supersede_wait_timer(&self, session_id: &str, armed_wake: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(g) if g.pending_continuation_ms == Some(armed_wake) => {
                Self::put_locked(&conn, &g.with_pending_continuation(None))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Clear an unsatisfied wait barrier (both kinds) on a still-Active goal,
    /// in one guard. Used by the goal-wake service when the awaited task
    /// settles (or the boot recheck finds it already settled / vanished —
    /// fail-open) before it claims the wake continuation. `false` = nothing
    /// to clear (goal gone, terminal, or barrier already dropped) — the wake
    /// is then a no-op, never a resurrection.
    pub fn clear_wait_barrier(&self, session_id: &str, now_ms: u64) -> Result<bool> {
        let conn = self.lock();
        match Self::get_locked(&conn, session_id)? {
            Some(g) if g.is_active() && g.has_wait_barrier() => {
                Self::put_locked(&conn, &g.without_wait(now_ms))?;
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
    /// Lifecycle CAS — `expected_status` is the status in the snapshot the tool
    /// derived `next` from. When the LIVE status no longer matches it, a
    /// concurrent lifecycle transition (exhaustion block, failure block, a
    /// remote pause from another session, a gate-confirmed complete) won the
    /// read→write gap, and writing the snapshot's stale lifecycle cluster back
    /// would RESURRECT the goal: the next post_run would re-block it and push a
    /// duplicate "⏹ cap reached" to the user's channel, or the tool's stale
    /// `Active` would silently undo a deliberate remote pause (loop's
    /// `commit_field_update` keeps live `status`/`stop_reason` for the same
    /// reason). On a mismatch the whole lifecycle cluster (status, note,
    /// gate_outcome, completed_at_ms, wait-barrier fields) is taken from the
    /// live row; the remaining field updates still commit, and the outcome
    /// tells the caller its status write was superseded.
    ///
    /// [`FieldUpdate::Gone`] — a no-op — when the goal vanished since the read,
    /// so the caller reports that honestly instead of silently re-creating it.
    pub fn commit_field_update(
        &self,
        next: &Goal,
        expected_status: GoalStatus,
    ) -> Result<FieldUpdate> {
        let conn = self.lock();
        match Self::get_locked(&conn, &next.session_id)? {
            Some(live) => {
                let mut merged = next.clone();
                merged.pending_continuation_ms = live.pending_continuation_ms;
                merged.continuations_used = live.continuations_used;
                merged.tokens_at_start = live.tokens_at_start;
                merged.baseline_captured = live.baseline_captured;
                // Enrollment is owned by the delegation seam
                // (`register_budget_member`), not the tool — a member
                // registered between the tool's read and this write must
                // survive it.
                merged.budget_members = live.budget_members;
                // The claiming run's project root is likewise pipeline-owned:
                // a tool update must never roll it back to the snapshot's.
                merged.workspace = live.workspace.clone();
                // Owner/scope attribution (P1 data isolation) is stamped once
                // at creation and never re-derived from a tool snapshot — same
                // ownership rule as `workspace`.
                merged.owner_user_id = live.owner_user_id.clone();
                merged.scope_id = live.scope_id.clone();
                let outcome = if live.status == expected_status {
                    FieldUpdate::Committed
                } else {
                    // A concurrent lifecycle transition won the gap — keep its
                    // verdict (and the note explaining it) over the snapshot's.
                    merged.status = live.status;
                    merged.note = live.note.clone();
                    merged.gate_outcome = live.gate_outcome;
                    merged.completed_at_ms = live.completed_at_ms;
                    merged.waiting_until_ms = live.waiting_until_ms;
                    merged.waiting_on_task = live.waiting_on_task.clone();
                    merged.waiting_reason = live.waiting_reason.clone();
                    FieldUpdate::StatusSuperseded(live.status)
                };
                Self::put_locked(&conn, &merged)?;
                Ok(outcome)
            }
            None => Ok(FieldUpdate::Gone),
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

/// The settle-notify stamp: the instant this goal transitioned INTO `Complete`
/// (legacy rows fall back to `updated_at_ms`). Single-sourced so claim and
/// release can never derive different keys for the same completion.
fn settle_stamp(goal: &Goal) -> String {
    format!(
        "{}@{}",
        goal.id,
        goal.completed_at_ms.unwrap_or(goal.updated_at_ms)
    )
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
    fn timer_barrier_claims_an_exact_wake_and_spends_the_iteration() {
        use crate::goal::PursuitMode;
        let (store, _d) = temp_store();
        let g = Goal::new("sess-wait", "obj", 0, 1_000)
            .with_pursuit(PursuitMode::Active { max_iterations: 5 })
            .with_wait_until(61_000, Some("cooldown".into()), 1_000);
        store.put(&g).unwrap();

        let ContinuationDecision::Fire {
            delay_ms,
            wake_ms,
            prompt,
        } = store
            .try_claim_continuation("sess-wait", None, 1_000, false, None)
            .unwrap()
        else {
            panic!("a parked timer barrier must claim its wake");
        };
        assert_eq!(wake_ms, 61_000);
        assert_eq!(delay_ms, 60_000);
        assert!(prompt.contains("wait elapsed"), "got: {prompt}");

        let live = store.get("sess-wait").unwrap().unwrap();
        assert_eq!(
            live.continuations_used, 1,
            "the wake run costs an iteration"
        );
        assert_eq!(live.pending_continuation_ms, Some(61_000));

        // While the timer sleeps, further claims stay Idle (fresh pending
        // marker in the future).
        assert!(matches!(
            store
                .try_claim_continuation("sess-wait", None, 30_000, false, None)
                .unwrap(),
            ContinuationDecision::Idle
        ));

        // Fire-time confirm matches the stored marker; the barrier itself is
        // lazily cleared on the NEXT claim after the wake elapsed.
        assert!(store.confirm_fire("sess-wait", 61_000).unwrap());
        let ContinuationDecision::Fire { delay_ms, .. } = store
            .try_claim_continuation("sess-wait", None, 62_000, false, None)
            .unwrap()
        else {
            panic!("post-wake claim proceeds normally");
        };
        assert_eq!(delay_ms, 0, "normal continuation after the barrier elapsed");
        let live = store.get("sess-wait").unwrap().unwrap();
        assert!(!live.has_wait_barrier(), "elapsed barrier lazily cleared");
    }

    #[test]
    fn supersede_wait_timer_unparks_an_armed_claim() {
        use crate::goal::PursuitMode;
        let (store, _d) = temp_store();
        let g = Goal::new("sess-unpark", "obj", 0, 1_000)
            .with_pursuit(PursuitMode::Active { max_iterations: 5 })
            .with_wait_until(61_000, None, 1_000);
        store.put(&g).unwrap();
        // Parking run's post_run claims the timer (pending marker = wake).
        let ContinuationDecision::Fire { wake_ms, .. } = store
            .try_claim_continuation("sess-unpark", None, 1_000, false, None)
            .unwrap()
        else {
            panic!("timer claim");
        };
        assert_eq!(wake_ms, 61_000);
        assert_eq!(
            store
                .get("sess-unpark")
                .unwrap()
                .unwrap()
                .pending_continuation_ms,
            Some(61_000)
        );

        // User un-parks: clears the barrier (tool) then supersedes the armed
        // marker. Without the supersede the next claim would gate on the future
        // pending marker and stay Idle until the ORIGINAL wake.
        assert!(store.clear_wait_barrier("sess-unpark", 2_000).unwrap());
        assert!(store.supersede_wait_timer("sess-unpark", 61_000).unwrap());
        assert_eq!(
            store
                .get("sess-unpark")
                .unwrap()
                .unwrap()
                .pending_continuation_ms,
            None,
            "the stale timer marker was cleared"
        );
        // Now the un-parking run's post_run claims a fresh IMMEDIATE continuation.
        let ContinuationDecision::Fire { delay_ms, .. } = store
            .try_claim_continuation("sess-unpark", None, 2_000, false, None)
            .unwrap()
        else {
            panic!("un-parked pursuit must resume immediately, not wait out the old wake");
        };
        assert_eq!(delay_ms, 0);

        // supersede is a precise CAS: a non-matching wake value is a no-op.
        assert!(!store.supersede_wait_timer("sess-unpark", 999_999).unwrap());
    }

    #[test]
    fn timer_barrier_with_no_runway_blocks_instead_of_parking_forever() {
        use crate::goal::PursuitMode;
        let (store, _d) = temp_store();
        // Iteration cap already spent, then a timer park is requested.
        let mut g = Goal::new("sess-noroom", "obj", 0, 1_000)
            .with_pursuit(PursuitMode::Active { max_iterations: 2 })
            .with_wait_until(61_000, Some("cooldown".into()), 1_000);
        g.continuations_used = 2; // exhausted
        store.put(&g).unwrap();
        // A parked timer with no runway must Block now, not arm a timer that
        // would only wake into an immediate Block.
        let ContinuationDecision::Exhausted { note } = store
            .try_claim_continuation("sess-noroom", None, 1_000, false, None)
            .unwrap()
        else {
            panic!("exhausted timer park must block, not park forever");
        };
        assert!(note.contains("iteration cap"), "got: {note}");
        let live = store.get("sess-noroom").unwrap().unwrap();
        assert_eq!(live.status, GoalStatus::Blocked);
        assert!(!live.has_wait_barrier(), "barrier cleared on the block");
    }

    #[test]
    fn task_barrier_parks_idle_until_cleared() {
        use crate::goal::PursuitMode;
        let (store, _d) = temp_store();
        let g = Goal::new("sess-task", "obj", 0, 1_000)
            .with_pursuit(PursuitMode::Active { max_iterations: 5 })
            .with_wait_on_task("task-42".into(), None, 1_000);
        store.put(&g).unwrap();

        assert!(matches!(
            store
                .try_claim_continuation("sess-task", None, 999_999, false, None)
                .unwrap(),
            ContinuationDecision::Idle
        ));
        let live = store.get("sess-task").unwrap().unwrap();
        assert_eq!(live.continuations_used, 0, "parking spends nothing");

        // The wake service clears the barrier, then the claim proceeds.
        assert!(store.clear_wait_barrier("sess-task", 2_000).unwrap());
        assert!(matches!(
            store
                .try_claim_continuation("sess-task", None, 2_000, false, None)
                .unwrap(),
            ContinuationDecision::Fire { .. }
        ));
        // Second clear is a no-op (CAS honesty).
        assert!(!store.clear_wait_barrier("sess-task", 3_000).unwrap());
    }

    #[test]
    fn budget_member_enrollment_is_first_writer_wins_and_gated() {
        use crate::goal::PursuitMode;
        let (store, _d) = temp_store();
        // No goal → not enrolled.
        assert!(!store
            .register_budget_member("sess-tree", "agent:child:main", 100, 1)
            .unwrap());
        // Budgeted active goal → enrolled once, baseline kept on re-register.
        let g = Goal::new("sess-tree", "obj", 0, 1)
            .with_pursuit(PursuitMode::Active { max_iterations: 5 })
            .with_budget(Some(10_000));
        store.put(&g).unwrap();
        assert!(store
            .register_budget_member("sess-tree", "agent:child:main", 100, 2)
            .unwrap());
        assert!(store
            .register_budget_member("sess-tree", "agent:child:main", 999, 3)
            .unwrap());
        let live = store.get("sess-tree").unwrap().unwrap();
        assert_eq!(live.budget_members.len(), 1);
        assert_eq!(
            live.budget_members[0].tokens_at_join, 100,
            "re-registration must not re-stamp the join baseline"
        );
        // Self-enrollment is refused (own session is the tree's base term).
        assert!(!store
            .register_budget_member("sess-tree", "sess-tree", 0, 4)
            .unwrap());
        // Unbudgeted goal → not enrolled.
        store.put(&Goal::new("sess-plain", "obj", 0, 1)).unwrap();
        assert!(!store
            .register_budget_member("sess-plain", "agent:child:main", 0, 5)
            .unwrap());
    }

    #[test]
    fn commit_field_update_preserves_live_budget_members() {
        let (store, _d) = temp_store();
        let g = Goal::new("sess-merge", "obj", 0, 1).with_budget(Some(10_000));
        store.put(&g).unwrap();
        // Tool snapshots the goal, then a delegation enrolls a member.
        let tool_snapshot = store.get("sess-merge").unwrap().unwrap();
        assert!(store
            .register_budget_member("sess-merge", "agent:child:main", 42, 2)
            .unwrap());
        // Tool commits its (member-less) snapshot — the live member survives.
        let snap_status = tool_snapshot.status;
        let next = tool_snapshot.with_note(Some("tool note".into()), 3);
        assert_eq!(
            store.commit_field_update(&next, snap_status).unwrap(),
            FieldUpdate::Committed
        );
        let live = store.get("sess-merge").unwrap().unwrap();
        assert_eq!(live.budget_members.len(), 1, "enrollment owned by the seam");
        assert_eq!(live.note.as_deref(), Some("tool note"));
    }

    #[test]
    fn clear_wait_barrier_never_touches_terminal_goals() {
        let (store, _d) = temp_store();
        let g = Goal::new("sess-done", "obj", 0, 0)
            .with_wait_on_task("t".into(), None, 1)
            .with_status(GoalStatus::Complete, 2);
        store.put(&g).unwrap();
        assert!(!store.clear_wait_barrier("sess-done", 3).unwrap());
        assert_eq!(
            store.get("sess-done").unwrap().unwrap().status,
            GoalStatus::Complete
        );
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
            .try_claim_continuation("s", None, 1_000, false, None)
            .unwrap();
        let ContinuationDecision::Fire { wake_ms, .. } = first else {
            panic!("expected Fire, got {first:?}");
        };
        assert_eq!(store.get("s").unwrap().unwrap().continuations_used, 1);

        // A second run completing while that continuation is in flight must NOT
        // claim another one (and must not spend another iteration).
        assert_eq!(
            store
                .try_claim_continuation("s", None, 1_100, false, None)
                .unwrap(),
            ContinuationDecision::Idle
        );
        assert_eq!(store.get("s").unwrap().unwrap().continuations_used, 1);

        // Once the claimed continuation fires, the chain is free to continue.
        assert!(store.confirm_fire("s", wake_ms).unwrap());
        assert!(matches!(
            store
                .try_claim_continuation("s", None, 1_200, false, None)
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
            .try_claim_continuation("s", None, 1_000, false, None)
            .unwrap()
        else {
            panic!("expected Fire");
        };
        // The spawned task died (daemon crash / panic) — its marker would block
        // the pursuit forever without the grace.
        let past_grace = wake_ms + PENDING_STALE_GRACE_MS + 1;
        assert!(matches!(
            store
                .try_claim_continuation("s", None, past_grace, false, None)
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
            .try_claim_continuation("s", None, 1_000, false, None)
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
            .try_claim_continuation("s", None, 1_000, false, None)
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
                .try_claim_continuation("s", None, 2_100, false, None)
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
            .try_claim_continuation("s", None, 1_000, false, None)
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
    fn awaiting_gate_stamps_an_in_flight_marker_and_dedups_a_second_gate() {
        let (store, _d) = temp_store();
        let g = pursuing("s", 5).with_status(GoalStatus::Complete, 1);
        store.put(&g).unwrap();

        // First claim: arbitrate the gate, stamping an in-flight marker. Gate
        // arbitration owns the terminal transition, so the iteration is NOT
        // spent and the status is untouched — only the marker moves.
        let d = store
            .try_claim_continuation("s", None, 1_000, true, None)
            .unwrap();
        assert!(matches!(d, ContinuationDecision::AwaitingGate(_)));
        let stored = store.get("s").unwrap().unwrap();
        assert_eq!(stored.pending_continuation_ms, Some(1_000));
        assert_eq!(stored.status, GoalStatus::Complete);
        assert_eq!(stored.continuations_used, g.continuations_used);

        // A user turn completing inside the gate window re-enters the hook; the
        // fresh marker suppresses a SECOND concurrent gate command.
        assert_eq!(
            store
                .try_claim_continuation("s", None, 1_500, true, None)
                .unwrap(),
            ContinuationDecision::Idle
        );

        // Past the gate grace the gate task is presumed dead → re-arbitrate.
        assert!(matches!(
            store
                .try_claim_continuation("s", None, 1_000 + GATE_STALE_GRACE_MS + 1, true, None)
                .unwrap(),
            ContinuationDecision::AwaitingGate(_)
        ));
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
            .try_claim_continuation("s", None, 1_000, false, None)
            .unwrap()
        else {
            panic!("expected Fire");
        };
        // …and the tool writes its (stale) snapshot back.
        let snap_status = snapshot.status;
        let edited = snapshot.with_note(Some("user note".into()), 2_000);
        assert_eq!(
            store.commit_field_update(&edited, snap_status).unwrap(),
            FieldUpdate::Committed
        );
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
        assert_eq!(
            store.commit_field_update(&g, g.status).unwrap(),
            FieldUpdate::Gone
        );
        assert!(
            store.get("s").unwrap().is_none(),
            "a cleared goal must not be re-created by a losing update"
        );
    }

    #[test]
    fn commit_field_update_status_cas_keeps_a_concurrent_block() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();
        // Tool snapshots an Active goal…
        let snapshot = store.get("s").unwrap().unwrap();
        // …a continuation failure blocks it in the read→write gap…
        assert!(store.block_if_active("s", "boom", 500).unwrap());
        // …and the tool commits its stale (still-Active) snapshot with a lesson.
        let edited = snapshot
            .clone()
            .with_lesson_appended("lesson".into(), 1_000);
        assert_eq!(
            store.commit_field_update(&edited, snapshot.status).unwrap(),
            FieldUpdate::StatusSuperseded(GoalStatus::Blocked)
        );
        let live = store.get("s").unwrap().unwrap();
        assert_eq!(
            live.status,
            GoalStatus::Blocked,
            "the concurrent block must not be overwritten by a stale snapshot"
        );
        assert_eq!(live.note.as_deref(), Some("boom"));
        assert!(
            live.lessons.iter().any(|l| l == "lesson"),
            "non-lifecycle fields still commit"
        );
    }

    #[test]
    fn commit_field_update_applies_a_status_change_when_the_cas_holds() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();
        let snapshot = store.get("s").unwrap().unwrap();
        let paused = snapshot.with_status(GoalStatus::Paused, 500);
        assert_eq!(
            store
                .commit_field_update(&paused, GoalStatus::Active)
                .unwrap(),
            FieldUpdate::Committed
        );
        assert_eq!(store.get("s").unwrap().unwrap().status, GoalStatus::Paused);
    }

    #[test]
    fn block_if_abandonable_exempts_parked_goals_of_both_barrier_kinds() {
        let (store, _d) = temp_store();
        // A deadline-timer-parked pursuit recovers via GoalWakeService, not the
        // abandoned run — blocking it would strand the barrier forever.
        store
            .put(&pursuing("sess-timer", 5).with_wait_until(9_999, None, 1))
            .unwrap();
        assert!(!store
            .block_if_abandonable("sess-timer", "abandoned", 100)
            .unwrap());
        assert_eq!(
            store.get("sess-timer").unwrap().unwrap().status,
            GoalStatus::Active
        );
        // Same for a task-barrier-parked pursuit.
        store
            .put(&pursuing("sess-task", 5).with_wait_on_task("t".into(), None, 1))
            .unwrap();
        assert!(!store
            .block_if_abandonable("sess-task", "abandoned", 100)
            .unwrap());
        assert_eq!(
            store.get("sess-task").unwrap().unwrap().status,
            GoalStatus::Active
        );
        // An unparked active pursuit IS abandonable — its recovery hung on the
        // crashed run.
        store.put(&pursuing("sess-live", 5)).unwrap();
        assert!(store
            .block_if_abandonable("sess-live", "abandoned", 100)
            .unwrap());
        assert_eq!(
            store.get("sess-live").unwrap().unwrap().status,
            GoalStatus::Blocked
        );
        // A passive goal is never abandonable (its recovery is the user).
        store.put(&Goal::new("sess-passive", "obj", 0, 1)).unwrap();
        assert!(!store
            .block_if_abandonable("sess-passive", "abandoned", 100)
            .unwrap());
    }

    #[test]
    fn block_and_pause_drop_the_wait_barrier() {
        let (store, _d) = temp_store();
        store
            .put(&pursuing("s-block", 5).with_wait_until(9_999, None, 1))
            .unwrap();
        assert!(store.block_if_active("s-block", "boom", 100).unwrap());
        assert!(
            !store.get("s-block").unwrap().unwrap().has_wait_barrier(),
            "a blocked goal is no longer waiting on anything"
        );
        store
            .put(&pursuing("s-pause", 5).with_wait_on_task("t".into(), None, 1))
            .unwrap();
        assert!(store.pause_if_active("s-pause", "hold", 100).unwrap());
        let paused = store.get("s-pause").unwrap().unwrap();
        assert!(!paused.has_wait_barrier());
        assert_eq!(paused.status, GoalStatus::Paused);
    }

    #[test]
    fn claim_records_workspace_and_tool_updates_preserve_it() {
        let (store, _d) = temp_store();
        store.put(&pursuing("s", 5)).unwrap();
        // A post-run claim records the originating run's project root.
        let d = store
            .try_claim_continuation("s", None, 1_000, false, Some("/proj/x"))
            .unwrap();
        assert!(matches!(d, ContinuationDecision::Fire { .. }));
        assert_eq!(
            store.get("s").unwrap().unwrap().workspace.as_deref(),
            Some("/proj/x")
        );
        // A wake-service re-claim (no workspace info) must NOT wipe it.
        let live = store.get("s").unwrap().unwrap();
        assert!(store
            .confirm_fire("s", live.pending_continuation_ms.unwrap())
            .unwrap());
        let d = store
            .try_claim_continuation("s", None, 2_000, false, None)
            .unwrap();
        assert!(matches!(d, ContinuationDecision::Fire { .. }));
        assert_eq!(
            store.get("s").unwrap().unwrap().workspace.as_deref(),
            Some("/proj/x"),
            "hook-less claims keep the recorded workspace"
        );
        // …nor may a tool field update roll it back.
        let snapshot = store.get("s").unwrap().unwrap();
        let edited = snapshot.with_note(Some("n".into()), 3_000);
        assert_eq!(
            store
                .commit_field_update(&edited, GoalStatus::Active)
                .unwrap(),
            FieldUpdate::Committed
        );
        assert_eq!(
            store.get("s").unwrap().unwrap().workspace.as_deref(),
            Some("/proj/x")
        );
    }

    #[test]
    fn commit_field_update_never_clobbers_owner_scope() {
        // Mirrors `claim_records_workspace_and_tool_updates_preserve_it`: a
        // tool-side field update (computed from a `get` snapshot) must not
        // roll back the owner/scope attribution stamped at creation.
        let (store, _d) = temp_store();
        let attr = crate::scope::ScopeAttribution::personal("u-alice");
        let owned = pursuing("s", 5).with_owner_scope(Some(&attr));
        store.put(&owned).unwrap();

        let snapshot = store.get("s").unwrap().unwrap();
        assert_eq!(snapshot.owner_user_id.as_deref(), Some("u-alice"));
        // Simulate a tool call that read a stale/bare snapshot (e.g. one built
        // fresh via Goal::new without re-stamping owner_scope) and wrote it back.
        let edited = snapshot.with_note(Some("n".into()), 3_000);
        assert_eq!(
            store
                .commit_field_update(&edited, GoalStatus::Active)
                .unwrap(),
            FieldUpdate::Committed
        );
        let after = store.get("s").unwrap().unwrap();
        assert_eq!(after.owner_user_id.as_deref(), Some("u-alice"));
        assert_eq!(after.scope_id.as_deref(), Some("personal:u-alice"));
        assert_eq!(
            after.note.as_deref(),
            Some("n"),
            "the actual edit still lands"
        );
    }

    #[test]
    fn pause_all_owned_by_pauses_exactly_that_users_active_goals() {
        let (store, _d) = temp_store();
        let alice = crate::scope::ScopeAttribution::personal("u-alice");
        let bob = crate::scope::ScopeAttribution::personal("u-bob");
        store
            .put(&pursuing("s-alice-1", 5).with_owner_scope(Some(&alice)))
            .unwrap();
        store
            .put(&pursuing("s-alice-2", 5).with_owner_scope(Some(&alice)))
            .unwrap();
        store
            .put(&pursuing("s-bob", 5).with_owner_scope(Some(&bob)))
            .unwrap();
        // A legacy (pre-P1) row: owner_user_id is None, never alice's.
        store.put(&pursuing("s-legacy", 5)).unwrap();

        let count = store.pause_all_owned_by("u-alice").unwrap();
        assert_eq!(count, 2, "only alice's two active goals are paused");

        assert_eq!(
            store.get("s-alice-1").unwrap().unwrap().status,
            GoalStatus::Paused
        );
        assert_eq!(
            store.get("s-alice-2").unwrap().unwrap().status,
            GoalStatus::Paused
        );
        assert_eq!(
            store.get("s-bob").unwrap().unwrap().status,
            GoalStatus::Active,
            "bob's goal must be untouched"
        );
        assert_eq!(
            store.get("s-legacy").unwrap().unwrap().status,
            GoalStatus::Active,
            "legacy None-owner rows belong to the platform owner, not alice — untouched"
        );
    }

    #[test]
    fn pause_all_owned_by_is_a_no_op_for_a_user_with_no_active_goals() {
        let (store, _d) = temp_store();
        let bob = crate::scope::ScopeAttribution::personal("u-bob");
        store
            .put(&pursuing("s-bob", 5).with_owner_scope(Some(&bob)))
            .unwrap();
        assert_eq!(store.pause_all_owned_by("u-alice").unwrap(), 0);
        assert_eq!(
            store.get("s-bob").unwrap().unwrap().status,
            GoalStatus::Active
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

    /// Regression (W1): the gate-less Idle arm re-observes the same terminal
    /// `Complete` row on every later turn — the settle-notify claim must fire
    /// exactly once per completion WRITE, not once per observation, and must
    /// fire again for a genuinely new completion (re-set or re-completed goal).
    #[test]
    fn settle_notify_fires_once_per_completion_write() {
        let (store, _d) = temp_store();
        let done = pursuing("s", 5).with_status(GoalStatus::Complete, 1_000);

        assert!(store.try_claim_settle_notify(&done).unwrap());
        assert!(
            !store.try_claim_settle_notify(&done).unwrap(),
            "re-observing the same completion write must stay silent"
        );

        // Resume → re-complete: a new completion write (new updated_at_ms)
        // is a genuinely new victory claim.
        let redone = done
            .clone()
            .with_status(GoalStatus::Active, 2_000)
            .with_status(GoalStatus::Complete, 3_000);
        assert!(store.try_claim_settle_notify(&redone).unwrap());
        assert!(!store.try_claim_settle_notify(&redone).unwrap());

        // A different session's goal is independent.
        let other = pursuing("s2", 5).with_status(GoalStatus::Complete, 1_000);
        assert!(store.try_claim_settle_notify(&other).unwrap());
    }

    /// A claim spent on a poke that did not happen must be recoverable.
    ///
    /// The claim is one-shot and a still-`Complete` goal's stamp never moves
    /// again, so without a release the victory review is retired **forever**
    /// for that completion — no cron handle at boot, or a `run_job` that
    /// refused, and the paired watcher simply never looked. Release is scoped
    /// to our own stamp so a newer completion's claim is never erased.
    #[test]
    fn releasing_an_unpoked_settle_claim_allows_exactly_one_retry() {
        let (store, _d) = temp_store();
        let done = pursuing("s", 5).with_status(GoalStatus::Complete, 1_000);

        assert!(store.try_claim_settle_notify(&done).unwrap());
        assert!(!store.try_claim_settle_notify(&done).unwrap());

        assert!(store.release_settle_notify(&done).unwrap());
        assert!(
            store.try_claim_settle_notify(&done).unwrap(),
            "the same completion must be claimable again after a released claim"
        );
        assert!(!store.try_claim_settle_notify(&done).unwrap());

        // Releasing a stamp we no longer hold is a no-op: a newer completion
        // has already taken the row and must keep its claim.
        let redone = done
            .clone()
            .with_status(GoalStatus::Active, 2_000)
            .with_status(GoalStatus::Complete, 3_000);
        assert!(store.try_claim_settle_notify(&redone).unwrap());
        assert!(
            !store.release_settle_notify(&done).unwrap(),
            "an older stamp must not release the newer completion's claim"
        );
        assert!(!store.try_claim_settle_notify(&redone).unwrap());
    }

    /// Regression (W1 review): a post-completion field edit (lesson/note
    /// append) bumps `updated_at_ms` but is NOT a new victory claim — the
    /// stamp keys on `completed_at_ms`, so the edited row must stay silent.
    #[test]
    fn settle_notify_ignores_post_completion_field_edits() {
        let (store, _d) = temp_store();
        let done = pursuing("s", 5).with_status(GoalStatus::Complete, 1_000);
        assert!(store.try_claim_settle_notify(&done).unwrap());

        let annotated = done
            .clone()
            .with_lesson_appended("lesson X".into(), 2_000)
            .with_note(Some("post-mortem".into()), 3_000);
        assert_eq!(annotated.completed_at_ms, Some(1_000));
        assert!(
            !store.try_claim_settle_notify(&annotated).unwrap(),
            "lesson/note edits on a still-Complete goal must not re-fire"
        );

        // Leaving Complete clears the stamp; a genuine re-completion mints a
        // fresh one and fires again.
        let reopened = annotated.with_status(GoalStatus::Active, 4_000);
        assert_eq!(reopened.completed_at_ms, None);
        let redone = reopened.with_status(GoalStatus::Complete, 5_000);
        assert_eq!(redone.completed_at_ms, Some(5_000));
        assert!(store.try_claim_settle_notify(&redone).unwrap());
    }
}
