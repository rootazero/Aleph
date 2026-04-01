//! Types for the approval module.
//!
//! Defines the core types used by the approval system for desktop and browser
//! action authorization: action classification, approval decisions, and action
//! request metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Classification of actions that require approval.
///
/// Each variant maps to a specific capability that an agent can invoke.
/// The serialization uses snake_case to match the JSON policy config format.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    BrowserNavigate,
    BrowserClick,
    BrowserType,
    BrowserFill,
    BrowserEvaluate,
    DesktopClick,
    DesktopType,
    DesktopKeyCombo,
    DesktopLaunchApp,
    ShellExec,
}

/// The result of an approval policy check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Action is allowed without user interaction.
    Allow,
    /// Action is denied outright.
    Deny { reason: String },
    /// Action requires explicit user confirmation.
    Ask { prompt: String },
}

/// Per-action-type default decision used in [`PolicyConfig`](super::PolicyConfig).
///
/// Serialized as lowercase (`"allow"`, `"deny"`, `"ask"`), so invalid values
/// like `"Deny"` are rejected at parse time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultDecision {
    Allow,
    Deny,
    Ask,
}

/// A request submitted to the approval system for authorization.
pub struct ActionRequest {
    /// What kind of action is being requested.
    pub action_type: ActionType,
    /// The target of the action: URL, app bundle id, shell command, etc.
    pub target: String,
    /// Identifier for the agent making the request.
    pub agent_id: String,
    /// Human-readable description of the action's purpose.
    pub context: String,
    /// When the request was created.
    pub timestamp: DateTime<Utc>,
}
