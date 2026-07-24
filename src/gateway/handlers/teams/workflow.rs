//! Templates, ACP members, workflow step review, and task-level control handlers.

use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use super::crud::TeamIdParams;
use super::tasks::TaskIdParams;
use crate::agents::swarm::tasks::{
    CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, ReviewVerdict, ReviewerKind,
};
use crate::sync_primitives::Arc;
use crate::teams::{NewTeamMember, TeamMemberKind, TeamStore};

use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};

// =============================================================================
// teams.list_templates — discover available team templates (built-in + user)
// =============================================================================

/// Handle `teams.list_templates` — no params; returns the discoverable
/// templates' metadata so a UI can render a picker. Materialization itself
/// happens via the `team_from_template` builtin tool (R8 — tool-for-everything)
/// so we don't double-plumb the heavy dep set through the gateway.
pub async fn handle_list_templates(request: JsonRpcRequest) -> JsonRpcResponse {
    debug!("Handling teams.list_templates request");

    let registry = crate::teams::templates::TemplateRegistry::discover(
        &crate::teams::templates::loader::default_user_dir(),
    );

    let entries: Vec<serde_json::Value> = registry
        .list()
        .map(|tpl| {
            json!({
                "name": tpl.name,
                "description": tpl.description,
                "default_goal": tpl.default_goal,
                "leader_id": tpl.leader.id,
                "leader_role": tpl.leader.role,
                "member_count": tpl.members.len(),
                "task_count": tpl.tasks.len(),
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "templates": entries }))
}

// =============================================================================
// teams.snapshot.* — snapshot lifecycle as direct RPC
//
// Mirrors the `team_snapshot` builtin tool so panels and external callers can
// hit the snapshot store without going through tool-invoke. Same backing
// functions (capture_snapshot / restore_snapshot + SnapshotStore methods) so
// behaviour, dry-run defaults, and edge-restoration semantics are identical.
// =============================================================================

// =============================================================================
// ACP Member Operations (teams.acp_member.{add,remove,list})
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AcpMemberAddParams {
    pub team_id: String,
    pub harness_id: String,
    pub cwd: String,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default = "default_acp_role")]
    pub role: String,
}

fn default_acp_role() -> String {
    "acp-worker".to_string()
}

#[derive(Debug, Deserialize)]
pub struct AcpMemberRemoveParams {
    pub team_id: String,
    /// The synthetic id returned by `teams.acp_member.add` — also the value
    /// stored as `coord_tasks.owner` for tasks assigned to this member.
    pub agent_id: String,
}

/// `teams.acp_member.add` — register an external coding CLI session as a
/// team member. Subsequent tasks created with `owner = <agent_id>` will be
/// dispatched through the ACP adapter pool instead of the in-process agent
/// registry. Idempotent: re-adding the same (team, harness, cwd, name)
/// returns the existing row.
pub async fn handle_acp_member_add(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.acp_member.add request");
    let params: AcpMemberAddParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let new_member = NewTeamMember::for_acp_session(
        params.team_id.clone(),
        params.harness_id,
        params.cwd,
        params.session_name,
        params.role,
    );

    match store.add_member(new_member).await {
        Ok(member) => JsonRpcResponse::success(request.id, json!({ "member": member })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to add ACP member to team '{}': {}",
                params.team_id, e
            ),
        ),
    }
}

/// `teams.acp_member.remove` — detach an ACP-backed member from a team. The
/// underlying ACP session in the pool is **not** killed (other teams may
/// still reference it); use `acp.sessions.shutdown` for that.
pub async fn handle_acp_member_remove(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.acp_member.remove request");
    let params: AcpMemberRemoveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Guard: refuse to remove a non-ACP row via this RPC so the caller
    // doesn't accidentally drop an in-process agent.
    let members = match store.get_members(&params.team_id).await {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read team '{}' members: {}", params.team_id, e),
            )
        }
    };
    match members.iter().find(|m| m.agent_id == params.agent_id) {
        Some(m) if m.kind == TeamMemberKind::AcpSession => {}
        Some(_) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Member '{}' is not ACP-backed; use teams.remove_member",
                    params.agent_id
                ),
            )
        }
        None => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!(
                    "Member '{}' not found in team '{}'",
                    params.agent_id, params.team_id
                ),
            )
        }
    }

    match store.remove_member(&params.team_id, &params.agent_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to remove ACP member: {e}"),
        ),
    }
}

/// `teams.acp_member.list` — return only the ACP-backed members of a team.
/// Convenience filter — `teams.get` returns mixed-kind members.
pub async fn handle_acp_member_list(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.acp_member.list request");
    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match store.get_members(&params.team_id).await {
        Ok(members) => {
            let acp_only: Vec<_> = members
                .into_iter()
                .filter(|m| m.kind == TeamMemberKind::AcpSession)
                .collect();
            JsonRpcResponse::success(request.id, json!({ "members": acp_only }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to list ACP members for team '{}': {}",
                params.team_id, e
            ),
        ),
    }
}

// =============================================================================
// Workflow Step Review (Phase C — openteams-parity)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct WorkflowStepReviewParams {
    pub task_id: String,
    /// Reviewer category. `user` is set by the panel; the lead agent
    /// uses `lead_agent`; system auto-approval uses `auto`.
    #[serde(default = "default_reviewer_kind")]
    pub reviewer_kind: String,
    #[serde(default)]
    pub reviewer_id: Option<String>,
    /// Optional free-text comment appended as a task comment.
    #[serde(default)]
    pub comment: Option<String>,
}

fn default_reviewer_kind() -> String {
    "user".to_string()
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRetryStepParams {
    pub task_id: String,
}

fn parse_reviewer_kind(s: &str) -> Result<ReviewerKind, &'static str> {
    ReviewerKind::from_stored(s).ok_or("reviewer_kind must be one of: user, lead_agent, auto")
}

/// `teams.workflow.approve_step` — stamp the latest run as approved and
/// transition the task to Completed so downstream dependents unblock.
/// Idempotent on Completed tasks (re-approve is a no-op). Refuses to
/// approve tasks that have not yet finished a run.
pub async fn handle_workflow_approve_step(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.approve_step request");
    let params: WorkflowStepReviewParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let reviewer_kind = match parse_reviewer_kind(&params.reviewer_kind) {
        Ok(k) => k,
        Err(msg) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, msg.to_string()),
    };

    if let Err(e) = coord_store
        .record_run_review(
            &params.task_id,
            ReviewVerdict::Approved,
            reviewer_kind,
            params.reviewer_id.as_deref(),
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to record review: {e}"),
        );
    }

    // Transition status: WaitingReview / InProgress → Completed.
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to mark task completed: {e}"),
        );
    }

    if let Some(comment) = params.comment.as_deref().filter(|c| !c.trim().is_empty()) {
        let author = params
            .reviewer_id
            .clone()
            .unwrap_or_else(|| format!("review:{}", reviewer_kind.as_str()));
        if let Err(e) = coord_store
            .add_task_comment(&params.task_id, &author, comment)
            .await
        {
            tracing::warn!(error = %e, "approve_step: failed to record review comment");
        }
    }

    JsonRpcResponse::success(request.id, json!({ "status": "completed" }))
}

/// `teams.workflow.reject_step` — stamp the latest run as rejected and
/// transition the task to Failed. Downstream dependents stay blocked.
pub async fn handle_workflow_reject_step(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.reject_step request");
    let params: WorkflowStepReviewParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let reviewer_kind = match parse_reviewer_kind(&params.reviewer_kind) {
        Ok(k) => k,
        Err(msg) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, msg.to_string()),
    };

    if let Err(e) = coord_store
        .record_run_review(
            &params.task_id,
            ReviewVerdict::Rejected,
            reviewer_kind,
            params.reviewer_id.as_deref(),
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to record review: {e}"),
        );
    }
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Failed),
                result: params.comment.clone(),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to mark task failed: {e}"),
        );
    }
    if let Some(comment) = params.comment.as_deref().filter(|c| !c.trim().is_empty()) {
        let author = params
            .reviewer_id
            .clone()
            .unwrap_or_else(|| format!("review:{}", reviewer_kind.as_str()));
        let _ = coord_store
            .add_task_comment(&params.task_id, &author, comment)
            .await;
    }
    JsonRpcResponse::success(request.id, json!({ "status": "failed" }))
}

/// `teams.workflow.retry_step` — re-queue a failed / rejected step. Clears
/// the lock + result fields and resets status to Pending so the
/// dispatcher (or `team_delegate`) can re-run. Prior runs stay in history.
pub async fn handle_workflow_retry_step(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.workflow.retry_step request");
    let params: WorkflowRetryStepParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    // Pre-fetch once: metadata basis for the budget re-arm stamp + the
    // leftover lock holder for the release below. A missing/unreadable task
    // skips the stamp and lets update_task surface the real error below.
    let snapshot = coord_store.get_task(&params.task_id).await.ok().flatten();
    // A deliberate re-queue re-arms the automatic retry budget: stamp the
    // anchor onto the task's current metadata so only failures from here on
    // count against max_retries (mirrors `workflow_step_review.retry`).
    let metadata = snapshot.as_ref().map(|t| {
        crate::agents::swarm::tasks::retry::with_retry_budget_reset_at(
            t.metadata.clone(),
            chrono::Utc::now().timestamp().max(0) as u64,
        )
    });
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Pending),
                result: Some(String::new()),
                metadata,
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reset task to pending: {e}"),
        );
    }
    // Release any leftover claim with its ACTUAL holder — releasing with ""
    // never clears a genuinely held lock (the store checks holder equality),
    // which would leave the retried task Pending-but-unschedulable until
    // release_stale_locks fires. Best-effort: failure is non-fatal.
    if let Some(holder) = snapshot.as_ref().and_then(|t| t.locked_by.as_deref()) {
        if let Err(e) = coord_store.release_lock(&params.task_id, holder).await {
            tracing::warn!(task_id = %params.task_id, holder = %holder, error = %e, "workflow step retry: could not release leftover lock");
        }
    }
    JsonRpcResponse::success(request.id, json!({ "status": "pending" }))
}

// =============================================================================
// Task-level Control (R3 — pause / resume / retry / skip)
// =============================================================================
//
// These complement `teams.workflow.{approve,reject,retry}_step` which are
// reviewer-context handlers (require a finished run). The task-control
// surface here is admin-context: it works on ANY task state, lets an
// operator suspend a still-pending task before it runs, resume it, or
// hard-retry a terminal task (Completed / Failed / Cancelled / Skipped /
// WaitingReview) without going through the review flow.
//
// Wiring choice: we expose these as `teams.task.*` RPCs and the
// `team_task_control` builtin tool (R8 — everything is a tool). The
// dispatcher needs no changes — it only claims tasks with status
// 'pending', so Paused tasks are naturally skipped.
//
// `TaskIdParams` is already defined above (used by list_task_runs /
// list_task_comments / list_task_events). We reuse it.

/// teams.task.pause — manually suspend a task so the dispatcher will not
/// claim it. Valid from Pending / Blocked / Unsatisfiable. `InProgress` is
/// rejected because the in-flight run is unsafe to silently abandon (use
/// `teams.task.skip` or wait for the run to finish); `WaitingReview` is
/// rejected because resume returns a task to Pending, which would re-run a
/// review-gated task and discard its completed work + pending verdict.
pub async fn handle_task_pause(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.task.pause request");
    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let current = match coord_store.get_task(&params.task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Task '{}' not found", params.task_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to fetch task: {e}"),
            )
        }
    };
    match current.status {
        // WaitingReview is deliberately NOT pausable: resume always returns a
        // task to Pending, which would re-schedule a review-gated task for a
        // FRESH run — discarding both the already-completed work product and the
        // pending lead verdict. A review-gated task is already idle (awaiting
        // approve/reject); its lifecycle verbs are review, not pause.
        CoordTaskStatus::Pending
        | CoordTaskStatus::Blocked
        | CoordTaskStatus::Unsatisfiable => {}
        CoordTaskStatus::Paused => {
            return JsonRpcResponse::success(request.id, json!({ "status": "paused" }));
        }
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Cannot pause task in status '{}' — only pending/blocked/unsatisfiable may be paused",
                    current.status
                ),
            )
        }
    }
    // WaitingReview is structurally unpausable on this face (the match above
    // rejects it), so the pause origin is always Pending/Blocked/Unsatisfiable
    // — no origin stamp needed. Write an explicit null so a stale stamp from
    // an earlier pause→retry cycle can never mis-restore this pause to
    // WaitingReview (mirror of `team_task_control`).
    let metadata = Some(crate::agents::swarm::tasks::merge_metadata_patch(
        &current.metadata,
        json!({ crate::agents::swarm::tasks::PAUSED_FROM_KEY: serde_json::Value::Null }),
    ));
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Paused),
                metadata,
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to pause task: {e}"),
        );
    }
    JsonRpcResponse::success(request.id, json!({ "status": "paused" }))
}

/// teams.task.resume — undo a Paused state, returning the task to Pending
/// so the dispatcher can pick it up on the next tick.
pub async fn handle_task_resume(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.task.resume request");
    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let current = match coord_store.get_task(&params.task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Task '{}' not found", params.task_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to fetch task: {e}"),
            )
        }
    };
    if current.status != CoordTaskStatus::Paused {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Cannot resume task in status '{}' — only paused tasks may be resumed",
                current.status
            ),
        );
    }
    // Restore the pause-origin status (WaitingReview for review-parked
    // tasks; Pending otherwise) and clear the stamp in the same write —
    // mirror of `team_task_control` resume.
    let restore = match crate::agents::swarm::tasks::paused_from(&current.metadata) {
        Some("waiting_review") => CoordTaskStatus::WaitingReview,
        _ => CoordTaskStatus::Pending,
    };
    let cleared = crate::agents::swarm::tasks::merge_metadata_patch(
        &current.metadata,
        json!({ crate::agents::swarm::tasks::PAUSED_FROM_KEY: serde_json::Value::Null }),
    );
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(restore),
                metadata: Some(cleared),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to resume task: {e}"),
        );
    }
    JsonRpcResponse::success(request.id, json!({ "status": restore.to_string() }))
}

/// teams.task.retry — hard-retry a terminal task. Unlike
/// `teams.workflow.retry_step` (which is review-context), this works for
/// any non-Pending status. Clears `result`, releases any stale lock, and
/// resets status to Pending. Prior `coord_task_runs` history is
/// preserved — a fresh attempt is started on next dispatcher tick.
pub async fn handle_task_retry(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.task.retry request");
    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let current = match coord_store.get_task(&params.task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Task '{}' not found", params.task_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to fetch task: {e}"),
            )
        }
    };
    if matches!(current.status, CoordTaskStatus::InProgress) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Cannot retry an in-progress task — cancel it first".to_string(),
        );
    }
    // Deliberate hard-retry → re-arm the automatic retry budget (see
    // `team_task_control.retry` — same anchor, same rationale).
    let metadata = crate::agents::swarm::tasks::retry::with_retry_budget_reset_at(
        current.metadata,
        chrono::Utc::now().timestamp().max(0) as u64,
    );
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Pending),
                result: Some(String::new()),
                metadata: Some(metadata),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to retry task: {e}"),
        );
    }
    // Same actual-holder release as the reviewer retry above — "" can never
    // clear a genuinely held lock.
    if let Some(holder) = current.locked_by.as_deref() {
        if let Err(e) = coord_store.release_lock(&params.task_id, holder).await {
            tracing::warn!(task_id = %params.task_id, holder = %holder, error = %e, "task retry: could not release leftover lock");
        }
    }
    JsonRpcResponse::success(request.id, json!({ "status": "pending" }))
}

/// teams.task.skip — admin-context skip. Equivalent to the reviewer
/// `workflow_step_review` skip but works without requiring a finished
/// run. Marks the task as Skipped so downstream dependents unblock.
pub async fn handle_task_skip(
    request: JsonRpcRequest,
    coord_store: Arc<dyn CoordTaskStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.task.skip request");
    let params: TaskIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err(e) = coord_store
        .update_task(
            &params.task_id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Skipped),
                ..Default::default()
            },
        )
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to skip task: {e}"),
        );
    }
    JsonRpcResponse::success(request.id, json!({ "status": "skipped" }))
}
