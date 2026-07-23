//! Approval request shape for command execution.

use super::analysis::CommandAnalysis;

/// Request for user approval
#[derive(Debug, Clone)]
pub struct ExecApprovalRequest {
    /// Unique request ID
    pub id: String,
    /// Full command string
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Command analysis result
    pub analysis: CommandAnalysis,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
    /// Why approval is being requested (escalation / confirmation context),
    /// surfaced to the resolving user. `None` when the command itself is the
    /// full context (plain exec approval).
    pub reason: Option<String>,
    /// Raw channel user id of the human whose message triggered this approval —
    /// the "originator". Set only for channel-originated tool runs; `None` for
    /// Panel / cron / internal / cluster approvals. Carried onto
    /// [`crate::exec::manager::ExecApprovalRecord`] so the channel button
    /// callback can refuse a resolution from anyone but the originator — closing
    /// the group-chat bypass where any paired member could approve another
    /// member's action. See `ManagerCallbackSink::handle_callback`.
    pub originator_user_id: Option<String>,
}

/// Backward compatibility type alias
pub type ApprovalRequest = ExecApprovalRequest;
