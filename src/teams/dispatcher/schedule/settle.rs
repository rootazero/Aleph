//! Settle sweep: push a workflow run's terminal summary to its origin channel.
//!
//! A workflow run executes autonomously on the dispatcher long after the
//! launching turn ended. Every other autonomous unit (loop, goal) pushes its
//! terminal state to the origin channel (R5 — "AI comes to you"; autonomous
//! ends never die silently); before this sweep a workflow run's only terminal
//! wire was the leader-inbox "Team work complete" message, which no mechanism
//! wakes an agent to read and which expires after its 15-minute TTL — from
//! the launching user's point of view a 10-step run finished in total
//! silence, discoverable only by polling `workflow(action='status')`.
//!
//! The sweep is a janitor pass (`dispatch_once` step 2d, sibling to
//! `warn_stale_reviews`): group dispatcher-managed tasks by the
//! `workflow_run_id` stamped at materialisation; a run whose every task
//! [`is_settled`](crate::agents::swarm::tasks::CoordTaskStatus::is_settled)
//! and which carries a `workflow_origin` stamp (interactive launch) gets ONE
//! summary pushed through the same channel registry the clarify step uses.
//! Delivery is made once-only by two layers: an in-process claim set for the
//! live send window, and a durable [`WORKFLOW_NOTIFIED_KEY`] metadata marker
//! that silences the sweep across daemon restarts. The `workflow` tool's
//! `cancel` action stamps the marker itself — the cancelling user already
//! knows, so no redundant push.
//!
//! A settled run now has TWO terminal exits, and this sweep is the single
//! collection point for both:
//! 1. the origin-channel push above (the user-facing one), and
//! 2. the loop_graph victory-claim poke (`notify_workflow_settled`) — a
//!    `workflow:<template>` node paired with a `watches` watcher gets its
//!    immediate review at the moment the run settles, exactly like a goal
//!    Complete or a team disband.
//!
//! Only INTERACTIVE runs (those carrying a `workflow_origin` stamp) reach the
//! candidate loop, so only they poke. A non-interactive run — launched by a
//! cron job — deliberately does not: the cron that launched it is itself the
//! observation surface watching that schedule, and poking from here would
//! double-review what the launching loop already sees.
//!
//! Mechanical throughout (R7/R10): pure status aggregation, no judgement of
//! whether the outcome is "good"; interpreting the summary is the user's /
//! model's job.

use std::collections::HashMap;

use crate::agents::swarm::tasks::{
    merge_metadata_patch, CoordTask, CoordTaskFilter, CoordTaskUpdate,
};
use crate::gateway::channel::{ChannelId, OutboundMessage};
use crate::sync_primitives::Arc;
use crate::workflow::{
    workflow_origin, NOTIFIED_BY_CANCEL, WORKFLOW_NAME_KEY, WORKFLOW_NOTIFIED_BY_KEY,
    WORKFLOW_NOTIFIED_KEY, WORKFLOW_RUN_ID_KEY, WORKFLOW_STEP_KEY,
};

use super::select::is_dispatcher_managed;
use super::TeamDispatcher;

/// Minimum age (seconds) of a `workflow_notified` stamp before an unsettled
/// marked run is treated as REOPENED (retry) rather than mid-cancel. The
/// `cancel` action stamps before its status writes; a human retry happens on
/// a much longer timescale.
const REOPEN_REARM_GRACE_SECS: u64 = 120;

/// Was `notified_anchor` written by the `workflow` tool's `cancel` action?
///
/// The re-arm rule needs to distinguish two stampers of
/// [`WORKFLOW_NOTIFIED_KEY`]:
///
/// - The settle sweep itself (which only stamps AFTER observing the run
///   fully settled — no mid-write window to protect).
/// - The `cancel` action (which stamps BEFORE its status writes land, so a
///   marked run legitimately has unsettled tasks for a moment).
///
/// Unknown provenance (rows stamped before [`WORKFLOW_NOTIFIED_BY_KEY`] was
/// introduced) returns `false` — callers fall back to the age-only grace
/// rule, which is the previous behaviour. **Pure** — no I/O, no clock.
#[must_use]
pub(crate) fn is_cancel_provenance(task: &CoordTask) -> bool {
    task.metadata
        .get(WORKFLOW_NOTIFIED_BY_KEY)
        .and_then(|v| v.as_str())
        == Some(NOTIFIED_BY_CANCEL)
}

/// Should a marked-and-now-unsettled run be re-armed (its `workflow_notified`
/// marker cleared) so the sweep delivers the corrected terminal outcome?
///
/// Caller has already determined "this run is no longer all settled"; this
/// function answers the *only* remaining question — is the durable stamp
/// still authoritative, or has it been overtaken by a real reopen?
///
/// Rule (preserves the inline logic this function was extracted from):
///
/// - Settle-sweep provenance → stamp written after full settlement, no
///   window to protect, re-arm unconditionally.
/// - Cancel provenance → stamp may sit mid-cancel for a few seconds; re-arm
///   only once `marker_age >= grace_secs` (so a cancel in flight is not
///   raced by a re-arm).
/// - Unknown provenance (no `_by` key) → fall back to the age rule, i.e.
///   re-arm only after the grace window — the previous behaviour.
///
/// `grace_secs` is [`REOPEN_REARM_GRACE_SECS`] in production; tests substitute
/// other values to exercise the boundary without sleeping. **Pure** — no I/O,
/// no clock; the caller passes `now_epoch`.
#[must_use]
pub(crate) fn should_rearm_on_reopen(
    notified_anchor: &CoordTask,
    now_epoch: u64,
    grace_secs: u64,
) -> bool {
    // A task with no stamp is not a re-arm target — the dispatcher's caller
    // already filters for stamped anchors before asking, but the function is
    // also total on a no-stamp input so future callers cannot trip on it. A
    // no-stamp task carries no durable marker to clear, so re-arming would
    // be a no-op at best and a future-confusing write at worst.
    let stamped = match notified_anchor
        .metadata
        .get(WORKFLOW_NOTIFIED_KEY)
        .and_then(|v| v.as_u64())
    {
        Some(s) => s,
        None => return false,
    };
    let marker_age = now_epoch.saturating_sub(stamped);

    if is_cancel_provenance(notified_anchor) {
        // Cancel provenance is the only stamper with a real mid-write window
        // to protect. Past the grace, the stamp has either landed (cancel
        // succeeded) or been clobbered by a real reopen — either way, the
        // marker no longer describes reality.
        marker_age >= grace_secs
    } else if notified_anchor
        .metadata
        .get(WORKFLOW_NOTIFIED_BY_KEY)
        .is_some()
    {
        // Settle provenance explicitly recorded → no grace needed. The
        // settle sweep only stamps after observing full settlement, so
        // there is no mid-write window to protect.
        true
    } else {
        // Unknown provenance (no `_by` key, rows stamped before
        // WORKFLOW_NOTIFIED_BY_KEY existed) → age rule, matching the
        // pre-provenance behaviour: only re-arm past the grace.
        marker_age >= grace_secs
    }
}

/// Render the terminal summary for one settled run (pure aggregation).
///
/// `✅` when every task satisfies its dependents (completed/skipped), `⚠️`
/// otherwise — a structural classification, not a judgement. Failed steps are
/// listed (bounded) so the user can react without a follow-up `status` call.
fn render_run_summary(name: &str, run_id: &str, team_id: &str, tasks: &[&CoordTask]) -> String {
    // Status counts, first-seen order.
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for task in tasks {
        let key = task.status.as_str();
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
    }
    let breakdown: Vec<String> = counts
        .into_iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect();

    let all_clean = tasks.iter().all(|t| t.status.satisfies_dependency());
    let icon = if all_clean { "✅" } else { "⚠️" };
    let rid_short: String = run_id.chars().take(8).collect();

    let mut out = format!(
        "{icon} Workflow '{name}' finished (run {rid_short}): {} step(s) — {}",
        tasks.len(),
        breakdown.join(", ")
    );

    // Failed-step details, bounded: at most 3 lines, 160 chars of error each.
    let mut failed_lines = 0usize;
    for task in tasks {
        if task.status != crate::agents::swarm::tasks::CoordTaskStatus::Failed {
            continue;
        }
        if failed_lines == 3 {
            out.push_str("\n- (more failed steps omitted — see status)");
            break;
        }
        let step = task
            .metadata
            .get(WORKFLOW_STEP_KEY)
            .and_then(|v| v.as_str())
            .unwrap_or(&task.subject);
        let err = task.result.as_deref().unwrap_or("").trim();
        let err_bounded = match err.char_indices().nth(160) {
            Some((idx, _)) => format!("{}…", &err[..idx]),
            None => err.to_string(),
        };
        out.push_str(&format!("\n- step '{step}' failed: {err_bounded}"));
        failed_lines += 1;
    }

    out.push_str(&format!(
        "\nInspect: workflow(action='status', name='{name}', team_id='{team_id}', run_id='{run_id}')"
    ));
    out
}

impl TeamDispatcher {
    /// One settle pass: notify the origin channel of every workflow run whose
    /// tasks have all settled and that has not been notified yet. Best-effort
    /// end to end — a failed send is logged and claimed in-process (no retry
    /// storm; the durable marker is only stamped after a successful send, so
    /// a daemon restart gets one more attempt).
    pub(crate) async fn notify_settled_workflow_runs(self: &Arc<Self>) {
        // No channel registry → nowhere to deliver; skip the scan entirely.
        let Some(channels) = self.channels.as_ref().and_then(|c| c.get()).cloned() else {
            return;
        };

        let tasks = match self
            .coord_store
            .list_tasks(CoordTaskFilter::default())
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher: settle sweep list_tasks failed");
                return;
            }
        };

        // Group dispatcher-managed workflow tasks by run id.
        let mut runs: HashMap<&str, Vec<&CoordTask>> = HashMap::new();
        for task in &tasks {
            if !is_dispatcher_managed(task) {
                continue;
            }
            let Some(rid) = task
                .metadata
                .get(WORKFLOW_RUN_ID_KEY)
                .and_then(|v| v.as_str())
                .filter(|r| !r.is_empty())
            else {
                continue;
            };
            runs.entry(rid).or_default().push(task);
        }

        // Candidates: fully settled, not durably notified, interactive origin.
        let mut candidates: Vec<(String, Vec<&CoordTask>)> = Vec::new();
        let mut live_unnotified: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for (rid, run_tasks) in runs {
            let all_settled = run_tasks.iter().all(|t| t.status.is_settled());
            let notified_anchor = run_tasks
                .iter()
                .find(|t| t.metadata.get(WORKFLOW_NOTIFIED_KEY).is_some());
            if let Some(anchor) = notified_anchor {
                // A notified run that is no longer fully settled was REOPENED
                // (a step retried via workflow_step_review / task_control).
                // Clear the marker so its true final outcome notifies again —
                // otherwise every post-retry ending is silent forever.
                // `should_rearm_on_reopen` encodes the full grace / provenance
                // rule (cancel vs settle vs unknown); see its doc comment for
                // why each provenance takes the path it does.
                if !all_settled
                    && should_rearm_on_reopen(
                        anchor,
                        Self::now_epoch(),
                        REOPEN_REARM_GRACE_SECS,
                    )
                {
                    let cleared = merge_metadata_patch(
                        &anchor.metadata,
                        serde_json::json!({
                            WORKFLOW_NOTIFIED_KEY: serde_json::Value::Null,
                            // Clear the provenance with the stamp it describes;
                            // a `_by` outliving its marker would answer for the
                            // NEXT stamper.
                            crate::workflow::WORKFLOW_NOTIFIED_BY_KEY: serde_json::Value::Null,
                        }),
                    );
                    if let Err(e) = self
                        .coord_store
                        .update_task(
                            &anchor.id,
                            CoordTaskUpdate {
                                metadata: Some(cleared),
                                ..Default::default()
                            },
                        )
                        .await
                    {
                        tracing::warn!(run_id = %rid, error = %e, "dispatcher: failed to re-arm workflow_notified marker after reopen");
                    } else {
                        tracing::info!(run_id = %rid, "dispatcher: workflow run reopened — terminal notification re-armed");
                        self.notified_workflow_runs.lock().await.remove(rid);
                    }
                }
                continue;
            }
            live_unnotified.insert(rid.to_string());
            if !all_settled {
                continue;
            }
            if run_tasks
                .iter()
                .find_map(|t| workflow_origin(&t.metadata))
                .is_none()
            {
                continue; // non-interactive launch — nobody to reach
            }
            candidates.push((rid.to_string(), run_tasks));
        }

        // Prune the in-process claim set to runs still live and unnotified so
        // it stays bounded (stamped runs are filtered by the durable marker
        // before ever reaching the set again).
        {
            let mut claimed = self.notified_workflow_runs.lock().await;
            claimed.retain(|rid| live_unnotified.contains(rid));
        }

        for (rid, run_tasks) in candidates {
            // In-process claim: exactly one tick wins the send window.
            {
                let mut claimed = self.notified_workflow_runs.lock().await;
                if !claimed.insert(rid.clone()) {
                    continue; // already attempted this daemon lifetime
                }
            }

            let Some((channel_id, conversation_id)) =
                run_tasks.iter().find_map(|t| workflow_origin(&t.metadata))
            else {
                continue; // unreachable: filtered above
            };
            let name = run_tasks
                .iter()
                .find_map(|t| t.metadata.get(WORKFLOW_NAME_KEY).and_then(|v| v.as_str()))
                .unwrap_or("(unknown)");
            let team_id = run_tasks
                .iter()
                .find_map(|t| t.team_id.as_deref())
                .unwrap_or_default();

            let text = render_run_summary(name, &rid, team_id, &run_tasks);
            let message = OutboundMessage::text(conversation_id.clone(), text.clone());
            let sent = match channels.send(&ChannelId::new(&channel_id), message).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    // The Panel's `gui:chat` is a pseudo-channel that is never
                    // registered in the ChannelRegistry, so the channel
                    // transport denies EVERY Panel-launched run — the summary
                    // this whole sweep exists to deliver could not reach the
                    // most common launch surface at all. Fall back to the
                    // team's own live topic, which the Panel already renders
                    // as a centred system chip (same wire `post_system` uses);
                    // mirrors ask_user's channel → event-bus fallback.
                    if team_id.is_empty() {
                        Err(e)
                    } else {
                        crate::gateway::event_emitter::team_fanout::publish_team_event(
                            team_id,
                            "system",
                            serde_json::json!({ "text": text }),
                        );
                        tracing::info!(
                            run_id = %rid,
                            channel = %channel_id,
                            error = %e,
                            "dispatcher: channel refused the workflow summary; delivered on the team topic instead"
                        );
                        Ok(())
                    }
                }
            };
            match sent {
                Ok(()) => {
                    tracing::info!(
                        run_id = %rid,
                        workflow = %name,
                        channel = %channel_id,
                        "dispatcher: workflow run terminal summary delivered"
                    );
                    // Durable once-only marker: stamp one task (smallest id —
                    // deterministic) so the sweep stays silent across
                    // restarts. Stamp failure is non-fatal: the in-process
                    // claim still suppresses re-sends this lifetime; a
                    // restart may then re-notify once (benign).
                    if let Some(anchor) = run_tasks.iter().min_by(|a, b| a.id.cmp(&b.id)) {
                        let merged = merge_metadata_patch(
                            &anchor.metadata,
                            serde_json::json!({
                                WORKFLOW_NOTIFIED_KEY: Self::now_epoch(),
                                // Provenance, so the re-arm rule does not have
                                // to guess it from the clock. This stamp is
                                // written only after the run was observed fully
                                // settled — there is no mid-write window to
                                // protect, so a later unsettled task is a real
                                // reopen no matter how recent the stamp.
                                crate::workflow::WORKFLOW_NOTIFIED_BY_KEY:
                                    crate::workflow::NOTIFIED_BY_SETTLE,
                            }),
                        );
                        if let Err(e) = self
                            .coord_store
                            .update_task(
                                &anchor.id,
                                CoordTaskUpdate {
                                    metadata: Some(merged),
                                    ..Default::default()
                                },
                            )
                            .await
                        {
                            tracing::warn!(run_id = %rid, error = %e, "dispatcher: failed to stamp workflow_notified marker");
                        }
                    }
                    // Terminal exit #2: the loop_graph victory-claim poke — a
                    // `watches`-paired cron watcher gets its immediate review
                    // now, not whenever its own cadence comes round. Guarded on
                    // a real template name: "(unknown)" means the metadata
                    // never carried one, and poking `workflow:(unknown)` could
                    // only wake a watcher for a node nobody can register.
                    //
                    // Best-effort, and deliberately NOT claim-guarded like the
                    // goal path is. The goal settle-notify CAS exists FOR the
                    // poke (release on failure ⇒ retry next observation). The
                    // `workflow_notified` marker above exists for the CHANNEL
                    // PUSH — which already succeeded — so releasing it over a
                    // failed poke would re-send a summary the user already got
                    // (and the reopen logic would then fight the re-stamp).
                    // The missed immediate review is backstopped by the
                    // watcher's periodic cadence, which is the same deal a
                    // debounced goal settle accepts. No graph / no watchers →
                    // `notify_workflow_settled` is a no-op returning true.
                    if name != "(unknown)" {
                        let poked = crate::loop_graph::service::notify_workflow_settled(name).await;
                        if !poked {
                            tracing::warn!(
                                run_id = %rid,
                                workflow = %name,
                                "dispatcher: loop_graph watcher poke failed — \
                                 watcher's periodic cadence backstops"
                            );
                        }
                    }
                }
                Err(e) => {
                    // Claimed but not stamped: no retry storm this lifetime,
                    // one fresh attempt after a restart.
                    tracing::warn!(
                        run_id = %rid,
                        channel = %channel_id,
                        error = %e,
                        "dispatcher: workflow run terminal summary delivery failed"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{CoordTaskStatus, Priority};

    fn task(id: &str, step: &str, status: CoordTaskStatus, result: Option<&str>) -> CoordTask {
        CoordTask {
            id: id.to_string(),
            team_id: Some("team-1".into()),
            subject: format!("wf:{step}"),
            description: String::new(),
            status,
            owner: Some("worker".into()),
            priority: Priority::Normal,
            result: result.map(str::to_string),
            metadata: serde_json::json!({ WORKFLOW_STEP_KEY: step }),
            dependencies: vec![],
            created_at: 0,
            started_at: None,
            completed_at: None,
            locked_by: None,
            locked_at: None,
        }
    }

    #[test]
    fn summary_all_clean_uses_check_icon_and_counts() {
        let a = task("t1", "gather", CoordTaskStatus::Completed, None);
        let b = task("t2", "write", CoordTaskStatus::Skipped, None);
        let s = render_run_summary("report", "run-1234567890", "team-1", &[&a, &b]);
        assert!(s.starts_with("✅"), "clean run gets ✅: {s}");
        assert!(s.contains("2 step(s)"));
        assert!(s.contains("1 completed"));
        assert!(s.contains("1 skipped"));
        assert!(s.contains("run run-1234"), "run id shortened to 8 chars");
        assert!(s.contains("action='status'"), "inspect hint present");
    }

    #[test]
    fn summary_with_failure_lists_step_and_bounded_error() {
        let a = task("t1", "gather", CoordTaskStatus::Completed, None);
        let long_err = "x".repeat(500);
        let b = task("t2", "write", CoordTaskStatus::Failed, Some(&long_err));
        let c = task("t3", "publish", CoordTaskStatus::Unsatisfiable, None);
        let s = render_run_summary("report", "r1", "team-1", &[&a, &b, &c]);
        assert!(s.starts_with("⚠️"), "failed run gets ⚠️: {s}");
        assert!(s.contains("step 'write' failed"));
        assert!(s.contains('…'), "long error is truncated");
        assert!(s.contains("1 unsatisfiable"));
        // Bounded: the 500-char error must not appear verbatim.
        assert!(!s.contains(&long_err));
    }

    #[test]
    fn summary_caps_failed_step_lines_at_three() {
        let tasks: Vec<CoordTask> = (0..5)
            .map(|i| {
                task(
                    &format!("t{i}"),
                    &format!("s{i}"),
                    CoordTaskStatus::Failed,
                    Some("boom"),
                )
            })
            .collect();
        let refs: Vec<&CoordTask> = tasks.iter().collect();
        let s = render_run_summary("wf", "r1", "team-1", &refs);
        assert_eq!(
            s.matches("failed: boom").count(),
            3,
            "at most 3 detail lines"
        );
        assert!(s.contains("more failed steps omitted"));
    }

    // --- D4 regression tests for the WORKFLOW_NOTIFIED_KEY re-arm rule ---
    //
    // The settle sweep's once-only notification must survive a `cancel` mid-
    // write window but re-arm when a real reopen leaves the run unsettled
    // past the grace. These tests pin the invariants so a future refactor
    // cannot silently break the dual-layer protection (in-process claim set
    // + durable stamp) by, say, removing the grace or applying it to settle-
    // stamped markers. Pure functions on a `CoordTask` snapshot — no
    // dispatcher fixture, no clock.

    /// A notified anchor carrying only the stamp (no `_by` key, no
    /// provenance) — the "unknown provenance" branch.
    fn stamped_anchor(stamp: u64) -> CoordTask {
        task(
            "t-anchor",
            "step",
            CoordTaskStatus::InProgress,
            None,
        )
        .with_metadata(serde_json::json!({
            WORKFLOW_STEP_KEY: "step",
            WORKFLOW_NOTIFIED_KEY: stamp,
        }))
    }

    /// A notified anchor with cancel provenance (stamped by the `cancel`
    /// action before its status writes land).
    fn cancel_stamped_anchor(stamp: u64) -> CoordTask {
        task(
            "t-anchor",
            "step",
            CoordTaskStatus::InProgress,
            None,
        )
        .with_metadata(serde_json::json!({
            WORKFLOW_STEP_KEY: "step",
            WORKFLOW_NOTIFIED_KEY: stamp,
            WORKFLOW_NOTIFIED_BY_KEY: NOTIFIED_BY_CANCEL,
        }))
    }

    /// A notified anchor with settle provenance (stamped by the settle sweep
    /// after observing the run fully settled).
    fn settle_stamped_anchor(stamp: u64) -> CoordTask {
        task(
            "t-anchor",
            "step",
            CoordTaskStatus::InProgress,
            None,
        )
        .with_metadata(serde_json::json!({
            WORKFLOW_STEP_KEY: "step",
            WORKFLOW_NOTIFIED_KEY: stamp,
            WORKFLOW_NOTIFIED_BY_KEY: crate::workflow::NOTIFIED_BY_SETTLE,
        }))
    }

    #[test]
    fn cancel_stamp_within_grace_does_not_rearm() {
        // A cancel stamper writes the marker mid-write; for a few seconds
        // the run legitimately has unsettled tasks. Re-arming here would
        // defeat the cancel suppression and double-push the user a summary
        // they already produced.
        let stamped_at = 1_000u64;
        let anchor = cancel_stamped_anchor(stamped_at);
        // 5 seconds after stamp → well within REOPEN_REARM_GRACE_SECS (120).
        assert!(!should_rearm_on_reopen(&anchor, stamped_at + 5, REOPEN_REARM_GRACE_SECS));
        // 119 seconds after stamp → still within grace.
        assert!(!should_rearm_on_reopen(
            &anchor,
            stamped_at + 119,
            REOPEN_REARM_GRACE_SECS
        ));
    }

    #[test]
    fn cancel_stamp_past_grace_arms_reopen() {
        // Past the grace window, the cancel stamper has either landed or
        // been clobbered by a real reopen. Either way the marker no longer
        // describes reality, so re-arm.
        let stamped_at = 1_000u64;
        let anchor = cancel_stamped_anchor(stamped_at);
        // Boundary: stamp_age == grace → re-arm (saturating_sub handles this).
        assert!(should_rearm_on_reopen(
            &anchor,
            stamped_at + REOPEN_REARM_GRACE_SECS,
            REOPEN_REARM_GRACE_SECS
        ));
        // Well past grace → re-arm.
        assert!(should_rearm_on_reopen(&anchor, stamped_at + 130, REOPEN_REARM_GRACE_SECS));
    }

    #[test]
    fn settle_stamp_does_not_apply_grace() {
        // The settle sweep only stamps after observing the run fully
        // settled, so there is no mid-write window to protect. A 1-second-
        // old settle stamp must still re-arm: this is the commonest reopen
        // of all ("it failed" → user replies retry within seconds), and the
        // grace must not blind the sweep to it.
        let stamped_at = 1_000u64;
        let anchor = settle_stamped_anchor(stamped_at);
        assert!(should_rearm_on_reopen(&anchor, stamped_at + 1, REOPEN_REARM_GRACE_SECS));
        // Even an immediate re-check (now == stamp) re-arms, since there is
        // no grace for settle provenance.
        assert!(should_rearm_on_reopen(&anchor, stamped_at, REOPEN_REARM_GRACE_SECS));
    }

    #[test]
    fn unknown_provenance_keeps_age_rule() {
        // Rows stamped before WORKFLOW_NOTIFIED_BY_KEY existed carry only the
        // stamp; the rule for those is the previous (age-only) behaviour,
        // preserved exactly. Within grace → no re-arm; past grace → re-arm.
        let stamped_at = 1_000u64;
        let anchor = stamped_anchor(stamped_at);
        assert!(!should_rearm_on_reopen(&anchor, stamped_at + 5, REOPEN_REARM_GRACE_SECS));
        assert!(should_rearm_on_reopen(
            &anchor,
            stamped_at + REOPEN_REARM_GRACE_SECS,
            REOPEN_REARM_GRACE_SECS
        ));
        assert!(should_rearm_on_reopen(&anchor, stamped_at + 200, REOPEN_REARM_GRACE_SECS));
    }

    #[test]
    fn no_notification_anchor_is_not_a_rearm_target() {
        // A task that never carried the stamp is not a rearm target: the
        // sweep's caller only asks `should_rearm_on_reopen` of tasks that
        // already have the stamp. A no-stamp task returns false here so
        // the function is total over its input domain.
        let anchor = task("t-no-stamp", "step", CoordTaskStatus::InProgress, None);
        assert!(!should_rearm_on_reopen(&anchor, 1_000, REOPEN_REARM_GRACE_SECS));
    }

    #[test]
    fn is_cancel_provenance_detects_by_value() {
        // Only the exact `NOTIFIED_BY_CANCEL` string counts as cancel
        // provenance. Anything else (settle, unknown, typo) is not cancel.
        let cancel = cancel_stamped_anchor(1_000);
        assert!(is_cancel_provenance(&cancel));
        let settle = settle_stamped_anchor(1_000);
        assert!(!is_cancel_provenance(&settle));
        let unknown = stamped_anchor(1_000);
        assert!(!is_cancel_provenance(&unknown));
        // Typo / wrong-case provenance is NOT cancel provenance — guards
        // against drift if the constant is ever reworded.
        let typo = task("t-typo", "step", CoordTaskStatus::InProgress, None)
            .with_metadata(serde_json::json!({
                WORKFLOW_STEP_KEY: "step",
                WORKFLOW_NOTIFIED_KEY: 1_000u64,
                WORKFLOW_NOTIFIED_BY_KEY: "Cancel", // capital C
            }));
        assert!(!is_cancel_provenance(&typo));
        // A non-string _by (e.g. accidental null) is not cancel provenance.
        let wrong_shape = task("t-shape", "step", CoordTaskStatus::InProgress, None)
            .with_metadata(serde_json::json!({
                WORKFLOW_STEP_KEY: "step",
                WORKFLOW_NOTIFIED_KEY: 1_000u64,
                WORKFLOW_NOTIFIED_BY_KEY: serde_json::Value::Null,
            }));
        assert!(!is_cancel_provenance(&wrong_shape));
    }

    #[test]
    fn is_cancel_provenance_is_false_for_settle_provenance() {
        // Explicit regression assertion for the boundary case the grace rule
        // depends on: settle-stamped runs must NEVER be treated as cancel
        // provenance (and therefore must never go through the cancel grace).
        let settle = settle_stamped_anchor(1_000);
        assert!(!is_cancel_provenance(&settle));
        // And the full rearm path confirms it: a settle-stamped anchor at
        // stamp_age == 1 re-arms, where a cancel-stamped one would not.
        assert!(should_rearm_on_reopen(&settle, 1_001, REOPEN_REARM_GRACE_SECS));
        let cancel = cancel_stamped_anchor(1_000);
        assert!(!should_rearm_on_reopen(&cancel, 1_001, REOPEN_REARM_GRACE_SECS));
    }

    // Helper for building a task with arbitrary metadata. Lives below the
    // tests so the existing `task(...)` helper above is not touched.
    trait CoordTaskExt {
        fn with_metadata(self, metadata: serde_json::Value) -> CoordTask;
    }
    impl CoordTaskExt for CoordTask {
        fn with_metadata(mut self, metadata: serde_json::Value) -> CoordTask {
            self.metadata = metadata;
            self
        }
    }
}
