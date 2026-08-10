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
    workflow_origin, WORKFLOW_NAME_KEY, WORKFLOW_NOTIFIED_KEY, WORKFLOW_RUN_ID_KEY,
    WORKFLOW_STEP_KEY,
};

use super::select::is_dispatcher_managed;
use super::TeamDispatcher;

/// Minimum age (seconds) of a `workflow_notified` stamp before an unsettled
/// marked run is treated as REOPENED (retry) rather than mid-cancel. The
/// `cancel` action stamps before its status writes; a human retry happens on
/// a much longer timescale.
const REOPEN_REARM_GRACE_SECS: u64 = 120;

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
                // Grace-gated on the marker's age: the `cancel` action stamps
                // the marker BEFORE its status writes land, so for a few
                // moments a marked run legitimately has unsettled tasks —
                // clearing in that window would defeat the cancel
                // suppression. A human retry-reopen happens minutes later.
                let marker_age = anchor
                    .metadata
                    .get(WORKFLOW_NOTIFIED_KEY)
                    .and_then(|v| v.as_u64())
                    .map_or(u64::MAX, |stamped| {
                        Self::now_epoch().saturating_sub(stamped)
                    });
                // The grace protects the `cancel` stamper's mid-write window
                // and nothing else. THIS sweep only stamps after observing the
                // run fully settled, so its own marker has no window — waiting
                // out the grace on it just loses the commonest reopen of all
                // ("it failed" → the user replies "retry" within seconds), and
                // the corrected run's real outcome is then never announced.
                // Unknown provenance (rows stamped before the key existed)
                // keeps the age rule, i.e. the previous behaviour.
                let stamped_after_settled = anchor
                    .metadata
                    .get(crate::workflow::WORKFLOW_NOTIFIED_BY_KEY)
                    .and_then(|v| v.as_str())
                    == Some(crate::workflow::NOTIFIED_BY_SETTLE);
                if !all_settled && (stamped_after_settled || marker_age >= REOPEN_REARM_GRACE_SECS)
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
}
