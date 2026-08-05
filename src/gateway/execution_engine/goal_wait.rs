//! Goal wait-barrier wake service — the event/boot half of the goal wait
//! barrier (hermes `GoalManager` wait parity, mapped onto Aleph's existing
//! machinery).
//!
//! The barrier itself is model-owned state on the [`crate::goal::Goal`] row
//! (`goal(update, wait_minutes=… | wait_for_task=…)`, R7/R8) and the DEADLINE
//! kind is woken entirely by the claim pipeline: `try_claim_continuation`
//! arms an exact timer through the normal `Fire` machinery at the next
//! `post_run`. What that leaves uncovered — and what this service exists for:
//!
//! 1. **Task barriers** have no wake instant: this service subscribes to the
//!    GlobalBus task-settle events (the same one-line pattern as
//!    `TeamDispatcher::subscribe_task_events`) and wakes any goal parked on
//!    the settling task.
//! 2. **Restarts** kill the sleeping timer task and may swallow a settle
//!    event: [`Self::rearm_parked_goals`] runs once at boot — re-arming
//!    claimed timers with their remaining delay (the stored pending marker IS
//!    the `confirm_fire` key, so the CAS still holds), claiming never-claimed
//!    timer barriers, and re-checking task barriers against the live
//!    `CoordTaskStore` (fail-open: a vanished task reads as settled, so a
//!    stale barrier can never wedge a pursuit forever — hermes' rule).
//!
//! Identity: a boot/event wake has no completing run to inherit policy
//! metadata from (the deadline path claimed at `post_run` does, and keeps
//! it). It shares `channel_policy::system_continuation_identity` with
//! `ResumeCoordinator::stamp_origin_identity`: a `guest` role floor PLUS the
//! origin channel's `tool_permissions` deny layer when an origin route exists,
//! `unattended` when none does — never fail-open operator, and never bypassing
//! a per-channel tool deny.
//!
//! Lives outside `src/harness/` (R10); holds no judgment — the decision to
//! park and what to do on wake are the model's, carried by the resume prompt
//! (R9).

use std::collections::HashMap;

use tracing::{info, warn};

use super::execute::{notify_origin, spawn_continuation_run, ContinuationKind};
use super::goal_continuation::{now_ms, origin_of};
use super::{ContinuationDeps, UNATTENDED_KEY};
use crate::agents::swarm::tasks::CoordTaskStore;
use crate::gateway::agent_instance::AgentInstance;
use crate::goal::{ContinuationDecision, Goal};
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;

pub struct GoalWakeService {
    deps: ContinuationDeps,
    /// For the boot recheck of task barriers. `None` → recheck skipped
    /// (fail-open wake instead: with no store the task can never settle).
    coord_store: Option<Arc<dyn CoordTaskStore>>,
}

impl GoalWakeService {
    #[must_use]
    pub fn new(deps: ContinuationDeps, coord_store: Option<Arc<dyn CoordTaskStore>>) -> Self {
        Self { deps, coord_store }
    }

    /// Subscribe to task-settle events on the GlobalBus. Call once at boot;
    /// the bus holds the callback (and this service) for the process
    /// lifetime. An event for a task nobody waits on is a cheap no-op scan.
    pub fn subscribe(self: &Arc<Self>) {
        use crate::event::{AlephEvent, EventFilter, EventType, GlobalBus};
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            GlobalBus::global()
                .subscribe_async(
                    EventFilter::new(vec![
                        EventType::TeamTaskCompleted,
                        EventType::TeamTaskFailed,
                        EventType::TeamTaskUpdated,
                    ]),
                    move |event| {
                        // Sync callback: extract (task_id, settled cause) and
                        // hand the async work to a task.
                        let settled = match &event.event {
                            AlephEvent::TeamTaskCompleted { task_id, .. } => {
                                Some((task_id.clone(), "completed".to_string()))
                            }
                            AlephEvent::TeamTaskFailed { task_id, .. } => {
                                Some((task_id.clone(), "failed".to_string()))
                            }
                            // `cancelled`/`skipped` are real emitted verbs (the
                            // `_` arm of emit_task_topic). `Unsatisfiable` is
                            // NOT emitted (it is derived at read time, never
                            // stored) — a goal parked on a task that derives
                            // Unsatisfiable is caught by the periodic recheck
                            // (`is_settled()` includes Unsatisfiable), not here.
                            AlephEvent::TeamTaskUpdated {
                                task_id, status, ..
                            } if matches!(status.as_str(), "cancelled" | "skipped") => {
                                Some((task_id.clone(), status.clone()))
                            }
                            _ => None,
                        };
                        if let Some((task_id, status)) = settled {
                            let svc = Arc::clone(&svc);
                            tokio::spawn(async move {
                                svc.wake_task_barriers(&task_id, &status).await;
                            });
                        }
                    },
                )
                .await;
            info!("GoalWakeService subscribed to task settle events (goal wait barriers)");
        });
    }

    /// Wake every goal parked on `task_id`. Cross-session scan is a cheap
    /// full read of the (small) goals table, only ever run on a settle event.
    async fn wake_task_barriers(&self, task_id: &str, status: &str) {
        let Some(store) = crate::goal::global() else {
            return;
        };
        let goals = match store.list_all() {
            Ok(g) => g,
            // A read failure must not silently drop every wake for this
            // settle event — log it so the loss is diagnosable.
            Err(e) => {
                warn!(error = %e, task = %task_id,
                    "goal wake: failed to list goals for task-barrier wake; skipping this event");
                return;
            }
        };
        let parked: Vec<Goal> = goals
            .into_iter()
            .filter(|g| g.is_active() && g.waiting_on_task.as_deref() == Some(task_id))
            .collect();
        for goal in parked {
            self.wake(&goal, &format!("task '{task_id}' settled ({status})"))
                .await;
        }
    }

    /// Boot sweep: re-establish wakes for goals parked across a restart.
    pub async fn rearm_parked_goals(&self) {
        let Some(store) = crate::goal::global() else {
            return;
        };
        let now = now_ms();
        let goals = match store.list_all() {
            Ok(g) => g,
            Err(e) => {
                warn!(error = %e, "goal wake: boot list_all failed; parked goals not re-armed");
                return;
            }
        };
        for goal in goals {
            if !goal.is_active() || !matches!(goal.pursuit, crate::goal::PursuitMode::Active { .. })
            {
                continue;
            }
            if goal.waiting_on_task.is_some() {
                // Task barrier: recheck against the live store (the shared
                // path the periodic tick also runs).
                self.recheck_task_barrier(&goal, "while the daemon was down")
                    .await;
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
                if goal.waiting_until_ms.is_some() {
                    self.spawn_wake_run(
                        &goal,
                        crate::goal::pursuit::wait_resume_prompt(&goal, "the wait elapsed"),
                        wake_ms.saturating_sub(now),
                        wake_ms,
                    )
                    .await;
                }
            } else if goal.has_wait_barrier() {
                // Timer barrier stamped but never claimed (crash between the
                // tool write and the next post_run), or one that elapsed
                // while we were down: claim now — the claim pipeline arms the
                // remaining timer or fires immediately as appropriate.
                self.claim_and_spawn(&goal, None).await;
            }
        }
    }

    /// Spawn a periodic task-barrier recheck loop. The GlobalBus subscription
    /// is the primary waker, but it can miss a settle that fired BEFORE the
    /// barrier row existed (the model parks on a task that just completed), a
    /// typo'd / deleted task id, and a task that derives to `Unsatisfiable`
    /// (never stored, so no event ever carries its id). This bounded sweep
    /// (fail-open, same store recheck as boot) is the backstop that keeps any
    /// of those from wedging a pursuit until the next restart. Cheap: one
    /// `list_all` + one `get_task` per genuinely task-parked goal.
    pub fn spawn_periodic_recheck(self: &Arc<Self>, interval_secs: u64) {
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(interval_secs.max(1)));
            ticker.tick().await; // consume the immediate first tick.
            loop {
                ticker.tick().await;
                let Some(store) = crate::goal::global() else {
                    continue;
                };
                let goals = match store.list_all() {
                    Ok(g) => g,
                    Err(e) => {
                        warn!(error = %e,
                            "goal wake: failed to list goals on periodic recheck; retrying next tick");
                        continue;
                    }
                };
                for goal in goals {
                    if goal.is_active()
                        && matches!(goal.pursuit, crate::goal::PursuitMode::Active { .. })
                        && goal.waiting_on_task.is_some()
                    {
                        svc.recheck_task_barrier(&goal, "on periodic recheck").await;
                    }
                }
            }
        });
    }

    /// Recheck one task-parked goal against the live `CoordTaskStore` and wake
    /// it if the awaited task has settled / vanished (fail-open — a task that
    /// cannot be found can never settle, so waking one step early beats
    /// parking forever, hermes' rule). No-op while the task is still live (the
    /// subscription covers that). Shared by the boot sweep and the periodic
    /// tick. `when` labels the wake cause for the resume prompt.
    async fn recheck_task_barrier(&self, goal: &Goal, when: &str) {
        let Some(task_id) = goal.waiting_on_task.clone() else {
            return;
        };
        let cause = match &self.coord_store {
            Some(cs) => match cs.get_task(&task_id).await {
                Ok(Some(t)) if t.status.is_settled() => {
                    Some(format!("task '{task_id}' settled ({}) {when}", t.status))
                }
                Ok(Some(_)) => None, // still live — the subscription covers it.
                Ok(None) => Some(format!("the awaited task '{task_id}' no longer exists")),
                Err(e) => {
                    warn!(session = %goal.session_id, error = %e,
                        "goal wake: task recheck failed; leaving parked");
                    None
                }
            },
            None => Some(format!(
                "the awaited task '{task_id}' cannot be tracked (no task store)"
            )),
        };
        if let Some(cause) = cause {
            self.wake(goal, &cause).await;
        }
    }

    /// Clear the barrier (CAS — a goal that moved on is left alone) and claim
    /// + spawn the wake continuation.
    async fn wake(&self, goal: &Goal, cause: &str) {
        let now = now_ms();
        match crate::goal::global().map(|s| s.clear_wait_barrier(&goal.session_id, now)) {
            Some(Ok(true)) => {}
            Some(Ok(false)) => {
                info!(session = %goal.session_id,
                    "goal wake: barrier already gone (goal moved on); skipping wake");
                return;
            }
            Some(Err(e)) => {
                warn!(session = %goal.session_id, error = %e, "goal wake: barrier clear failed");
                return;
            }
            None => return,
        }
        info!(session = %goal.session_id, cause = %cause, "goal wake: waking parked pursuit");
        self.claim_and_spawn(
            goal,
            Some(crate::goal::pursuit::wait_resume_prompt(goal, cause)),
        )
        .await;
    }

    /// Claim the next continuation through the normal atomic pipeline and
    /// spawn it. `override_prompt` replaces the claim's generic prompt (a
    /// wake should say WHY it fired — R9); `None` keeps the claim's own.
    async fn claim_and_spawn(&self, goal: &Goal, override_prompt: Option<String>) {
        let Some(store) = crate::goal::global() else {
            return;
        };
        let session = goal.session_id.clone();
        let gate_configured = self.deps.gate.is_some() || goal.gate_command.is_some();
        // No live token count here (no completing run): a token budget goes
        // unenforced for THIS claim; the next post_run re-enforces it.
        let decision =
            match store.try_claim_continuation(&session, None, now_ms(), gate_configured, None) {
                Ok(d) => d,
                Err(e) => {
                    warn!(session = %session, error = %e, "goal wake: claim failed");
                    return;
                }
            };
        match decision {
            ContinuationDecision::Fire {
                delay_ms,
                wake_ms,
                prompt,
            } => {
                self.spawn_wake_run(goal, override_prompt.unwrap_or(prompt), delay_ms, wake_ms)
                    .await;
            }
            ContinuationDecision::Exhausted { note } => {
                // The wake found no runway left — the store already blocked
                // the goal with its reason; push the notice like post_run
                // would (R5: an autonomous ending must not be silent).
                if let Some(key) = SessionKey::parse(&session) {
                    if let Some(agent) = self.deps.registry.get(key.agent_id()).await {
                        let origin = origin_of(&agent, &key).await;
                        notify_origin(origin.as_ref(), format!("⏹ {note}")).await;
                    }
                }
                info!(session = %session, note = %note,
                    "goal wake: pursuit exhausted at wake; goal blocked");
            }
            ContinuationDecision::Idle | ContinuationDecision::AwaitingGate(_) => {
                // Idle: another claim beat us (fine). AwaitingGate cannot
                // arise from an Active goal's wake; leave it to post_run.
            }
        }
    }

    /// Spawn the wake continuation run with re-derived (fail-closed) identity.
    async fn spawn_wake_run(&self, goal: &Goal, prompt: String, delay_ms: u64, wake_ms: u64) {
        let session = goal.session_id.clone();
        let Some(key) = SessionKey::parse(&session) else {
            warn!(session = %session, "goal wake: unparseable session key; cannot wake");
            return;
        };
        let Some(agent) = self.deps.registry.get(key.agent_id()).await else {
            warn!(session = %session, agent = %key.agent_id(),
                "goal wake: agent not registered; wake dropped (confirm_fire will lapse via stale grace)");
            return;
        };
        let policy_meta = rehydrate_owner_scope(goal, wake_identity(&agent, &key).await);
        // Rebuild the wake run in the project the last claiming run recorded
        // (the post-run hook writes it into the goal row); `None` falls back
        // to the agent workspace exactly as before.
        let workspace = goal.workspace.as_ref().map(std::path::PathBuf::from);
        spawn_continuation_run(
            self.deps.registry.clone(),
            self.deps.adapter.clone(),
            key,
            session,
            prompt,
            policy_meta,
            workspace,
            self.deps.event_bus.clone(),
            Some(delay_ms),
            ContinuationKind::Goal { wake_ms },
        );
    }
}

/// Rehydrate owner/scope attribution (P1 data isolation) into a wake run's
/// metadata from the goal's OWN persisted `owner_user_id`/`scope_id` fields.
///
/// `wake_identity` above has no completing run to inherit `carry_policy_metadata`
/// from — this IS the run with no predecessor, mirroring cron's fire path
/// (`executor::build_cron_metadata`) — so attribution must be reconstructed
/// from the durable row instead. Fail-closed via
/// [`crate::scope::ScopeAttribution::from_persisted`]: a legacy (pre-P1) goal
/// with neither column set, or an incoherent pair, emits nothing — the wake
/// stays unscoped, zero behavior change. Without this, a wake's
/// `memory.project_scoped` / retrieval / compaction reads would silently fall
/// back to the unscoped namespace even though the goal was created inside a
/// personal or project scope.
fn rehydrate_owner_scope(
    goal: &Goal,
    mut policy_meta: HashMap<String, String>,
) -> HashMap<String, String> {
    if let Some(attr) = crate::scope::ScopeAttribution::from_persisted(
        goal.owner_user_id.as_deref(),
        goal.scope_id.as_deref(),
    ) {
        crate::scope::stamp_metadata(&mut policy_meta, &attr);
    }
    policy_meta
}

/// Fail-closed identity for a wake run with no completing run to inherit from.
/// With an origin route: the shared `system_continuation_identity` stamp —
/// `guest` role floor PLUS the channel's `tool_permissions` DENY layer (so a
/// wake never bypasses a per-channel tool deny; the deny layer used to be
/// dropped here entirely). With none: `unattended`. Same stance, and now the
/// exact same derivation, as `ResumeCoordinator::stamp_origin_identity`.
async fn wake_identity(
    agent: &Arc<AgentInstance>,
    session_key: &SessionKey,
) -> HashMap<String, String> {
    match agent.origin_route(session_key).await {
        Some((channel, conversation)) => {
            crate::gateway::channel_policy::system_continuation_identity(&channel, &conversation)
        }
        None => {
            let mut meta = HashMap::new();
            meta.insert(UNATTENDED_KEY.to_string(), "true".to_string());
            meta
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned_goal() -> Goal {
        let attr = crate::scope::ScopeAttribution::personal("u-alice");
        Goal::new("agent:s:main", "obj", 0, 0).with_owner_scope(Some(&attr))
    }

    /// The gap this function exists to close: `spawn_wake_run` has no
    /// completing run to inherit `carry_policy_metadata` from, so it must
    /// rehydrate owner/scope from the goal's own persisted fields and emit
    /// both metadata keys into the run it builds.
    #[test]
    fn rehydrate_owner_scope_emits_both_keys_from_persisted_fields() {
        let meta = rehydrate_owner_scope(&owned_goal(), HashMap::new());
        assert_eq!(
            meta.get(crate::scope::OWNER_META_KEY).map(String::as_str),
            Some("u-alice")
        );
        assert_eq!(
            meta.get(crate::scope::SCOPE_META_KEY).map(String::as_str),
            Some("personal:u-alice")
        );
    }

    /// Existing keys (e.g. `wake_identity`'s unattended marker / channel
    /// permission layer) are preserved alongside the rehydrated attribution —
    /// this augments the wake's metadata, it does not replace it.
    #[test]
    fn rehydrate_owner_scope_preserves_existing_metadata_keys() {
        let mut base = HashMap::new();
        base.insert(UNATTENDED_KEY.to_string(), "true".to_string());
        let meta = rehydrate_owner_scope(&owned_goal(), base);
        assert_eq!(meta.get(UNATTENDED_KEY).map(String::as_str), Some("true"));
        assert_eq!(
            meta.get(crate::scope::OWNER_META_KEY).map(String::as_str),
            Some("u-alice")
        );
    }

    /// A legacy (pre-P1) goal with no owner/scope columns emits neither key —
    /// zero behavior change for goals that predate P1.
    #[test]
    fn rehydrate_owner_scope_is_a_no_op_for_a_legacy_goal() {
        let legacy = Goal::new("agent:s:main", "obj", 0, 0);
        let meta = rehydrate_owner_scope(&legacy, HashMap::new());
        assert!(!meta.contains_key(crate::scope::OWNER_META_KEY));
        assert!(!meta.contains_key(crate::scope::SCOPE_META_KEY));
    }
}
