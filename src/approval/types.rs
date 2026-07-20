//! Types for the approval module.
//!
//! Defines the core types used by the approval system for desktop and browser
//! action authorization: action classification, approval decisions, and action
//! request metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Classification of actions that require approval.
///
/// Each variant maps to a specific capability that an agent can invoke.
/// The serialization uses `snake_case` to match the JSON policy config format.
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
    /// Run an automation script (AppleScript/JXA/shell/PowerShell) or a named
    /// Shortcut — arbitrary code execution on the host.
    DesktopAutomation,
    /// Write/mutate a personal-information store (Calendar, Reminders, Notes,
    /// Contacts) via the PIM tool.
    PimWrite,
    /// Capture from the camera or microphone (`camera_snap` / `camera_clip` /
    /// `record_audio`) via the media tool — a privacy-sensitive action that
    /// turns on a sensor. macOS TCC prompts only on first use; after grant (and
    /// on a headless/LAN daemon) capture is otherwise silent, so it gets its own
    /// gate. Device enumeration / speech-to-text on an existing file are
    /// read-only and are not classified here.
    MediaCapture,
}

impl fmt::Display for ActionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::BrowserNavigate => "browser navigate",
            Self::BrowserClick => "browser click",
            Self::BrowserType => "browser type",
            Self::BrowserFill => "browser fill",
            Self::BrowserEvaluate => "browser evaluate",
            Self::DesktopClick => "desktop click",
            Self::DesktopType => "desktop type",
            Self::DesktopKeyCombo => "desktop key combo",
            Self::DesktopLaunchApp => "desktop launch app",
            Self::DesktopAutomation => "desktop automation script",
            Self::PimWrite => "personal-information write",
            Self::MediaCapture => "camera/microphone capture",
        };
        write!(f, "{s}")
    }
}

/// The result of an approval policy check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    /// What kind of action is being requested.
    pub action_type: ActionType,
    /// The target of the action: URL, app bundle id, shell command, etc.
    pub target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_target: String,
    /// Identifier for the agent making the request.
    pub agent_id: String,
    /// Human-readable description of the action's purpose.
    pub context: String,
    /// When the request was created.
    pub timestamp: DateTime<Utc>,
}
