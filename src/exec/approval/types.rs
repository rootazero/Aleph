use serde::{Deserialize, Serialize};

/// Approval request delivered to a channel's approval capability.
///
/// Kept as a tagged enum rather than a bare [`CommandApprovalRequest`] so the
/// serde shape (`{"type": "command", ...}`) stays stable for any consumer
/// holding serialized payloads. The `Capability` variant was removed: no
/// production code ever constructed it — only tests did.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalRequest {
    Command(CommandApprovalRequest),
}

/// Command approval request (placeholder for existing type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandApprovalRequest {
    pub command: String,
    pub cwd: Option<String>,
    /// Why this approval is being requested (escalation/confirmation context).
    /// Rendered to the user so they can make an informed decision; absent on
    /// payloads serialized before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The decision tiers this request permits (see
    /// [`crate::exec::allowed_decisions`]). Renderers consult this instead of
    /// hardcoding a button set; the serde default backfills pre-existing
    /// payloads with the historical unconstrained set.
    #[serde(default = "crate::exec::allowed_decisions::full_set")]
    pub allowed_decisions: Vec<crate::exec::socket::ApprovalDecisionType>,
}
