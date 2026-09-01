//! Team management tools.

/// Default working directory for an ACP team member enrolled via the
/// `acp:<harness>[/<session>]` reference syntax, which carries no cwd. The
/// dispatcher force-fails any ACP member with no cwd, so prefer the active
/// project root, then the server's cwd, then `.`.
pub(crate) fn acp_default_cwd() -> String {
    crate::projects::current_project_root()
        .or_else(|| std::env::current_dir().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string())
}

/// Verify the calling agent has authority on `team_id` — i.e. is the team's
/// leader OR a member. Returns `Ok(())` on success; on failure returns an
/// error with the SAME shape as "team does not exist" so a non-member
/// cannot enumerate team ids by probing.
///
/// Used by every team tool that takes a `team_id` argument (snapshot,
/// session_*, plan_resolve, lifecycle_resolve_shutdown, set_protocol,
/// disband, status, team_digest, task_read_artifact). The gate that used to
/// live only on `task_team_reachable` — ownership — is now applied here
/// too: without it, any agent that knew a team's id could list its
/// snapshots, read its sessions, set its protocol, and disband it. The
/// cost is one `get_team` + one `get_members` call per authorization,
/// both of which the underlying tool would make anyway.
///
/// Fail-closed: a store error is a denial, never a pass.
pub(crate) async fn require_team_auth(
    store: &dyn crate::teams::TeamStore,
    team_id: &str,
    caller: &str,
) -> Result<(), crate::error::AlephError> {
    let team = store.get_team(team_id).await.map_err(|e| {
        crate::error::AlephError::other(format!("team auth: failed to load '{team_id}': {e}"))
    })?;
    let team = team
        .ok_or_else(|| crate::error::AlephError::NotFound(format!("team `{team_id}` not found")))?;
    if team.leader_id == caller {
        return Ok(());
    }
    let members = store.get_members(team_id).await.map_err(|e| {
        crate::error::AlephError::other(format!(
            "team auth: failed to load members of '{team_id}': {e}"
        ))
    })?;
    if members.iter().any(|m| m.agent_id == caller) {
        return Ok(());
    }
    Err(crate::error::AlephError::NotFound(format!(
        "team `{team_id}` not found"
    )))
}

pub mod acp_member;
mod create;
mod delegate;
mod disband;
pub mod from_template;
pub mod inbox_read;
pub mod lifecycle_idle;
pub mod lifecycle_request_shutdown;
pub mod lifecycle_resolve_shutdown;
mod member_add;
mod member_remove;
pub mod message_send;
pub mod plan_resolve;
pub mod plan_submit;
pub mod session_collaborate;
pub mod session_read;
pub mod session_turn;
mod set_protocol;
pub mod snapshot;
mod status;
pub mod task_comment;
pub mod task_control;
pub mod task_exit_journal;
pub mod task_read_artifact;
pub mod task_review;
pub mod task_submit;
mod team_digest;
pub mod usage;
pub mod workflow_canvas;
pub mod workflow_step;

pub use acp_member::{TeamAcpMemberArgs, TeamAcpMemberOutput, TeamAcpMemberTool};
pub use create::{
    CreateAgentSpec, EnrolledMember, MemberSpec, TeamCreateArgs, TeamCreateOutput, TeamCreateTool,
};
pub use delegate::{DelegateStatus, TeamDelegateArgs, TeamDelegateOutput, TeamDelegateTool};
pub use disband::{TeamDisbandArgs, TeamDisbandOutput, TeamDisbandTool};
pub use from_template::{TeamFromTemplateArgs, TeamFromTemplateOutput, TeamFromTemplateTool};
pub use inbox_read::{InboxReadArgs, InboxReadOutput, InboxReadTool};
pub use lifecycle_idle::{LifecycleIdleArgs, LifecycleIdleOutput, LifecycleIdleTool};
pub use lifecycle_request_shutdown::{
    LifecycleRequestShutdownArgs, LifecycleRequestShutdownOutput, LifecycleRequestShutdownTool,
};
pub use lifecycle_resolve_shutdown::{
    LifecycleResolveShutdownArgs, LifecycleResolveShutdownOutput, LifecycleResolveShutdownTool,
};
pub use member_add::{TeamMemberAddArgs, TeamMemberAddOutput, TeamMemberAddTool};
pub use member_remove::{TeamMemberRemoveArgs, TeamMemberRemoveOutput, TeamMemberRemoveTool};
pub use message_send::{MessageSendArgs, MessageSendOutput, MessageSendTool};
pub use plan_resolve::{PlanResolveArgs, PlanResolveOutput, PlanResolveTool};
pub use plan_submit::{PlanSubmitArgs, PlanSubmitOutput, PlanSubmitTool};

pub use session_collaborate::{
    SessionCollaborateArgs, SessionCollaborateOutput, SessionCollaborateTool,
};
pub use session_read::{SessionReadArgs, SessionReadOutput, SessionReadTool};
pub use session_turn::{SessionTurnArgs, SessionTurnOutput, SessionTurnTool};
pub use set_protocol::{TeamSetProtocolArgs, TeamSetProtocolOutput, TeamSetProtocolTool};
pub use snapshot::{SnapshotAction, TeamSnapshotArgs, TeamSnapshotOutput, TeamSnapshotTool};
pub use status::{MemberInfo, TaskInfo, TeamStatusArgs, TeamStatusOutput, TeamStatusTool};
pub use task_comment::{TaskCommentArgs, TaskCommentOutput, TaskCommentTool};
pub use task_control::{TeamTaskControlArgs, TeamTaskControlOutput, TeamTaskControlTool};
pub use task_exit_journal::{TaskExitJournalArgs, TaskExitJournalOutput, TaskExitJournalTool};
pub use task_read_artifact::{TaskReadArtifactArgs, TaskReadArtifactOutput, TaskReadArtifactTool};
pub use task_review::{ReviewDecision, TaskReviewArgs, TaskReviewOutput, TaskReviewTool};
pub use task_submit::{TaskSubmitArgs, TaskSubmitOutput, TaskSubmitTool};
pub use team_digest::{TeamDigestArgs, TeamDigestOutput, TeamDigestTool};
pub use usage::{TeamUsageArgs, TeamUsageOutput, TeamUsageTool, UsageTotal};
pub use workflow_canvas::{
    TeamWorkflowCanvasArgs, TeamWorkflowCanvasOutput, TeamWorkflowCanvasTool, WorkflowCanvasAction,
};
pub use workflow_step::{
    verdict_admissible, WorkflowStepReviewArgs, WorkflowStepReviewOutput, WorkflowStepReviewTool,
};
