//! Approval request shape for command execution.

use super::analysis::CommandAnalysis;

/// Request for user approval
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
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
}
