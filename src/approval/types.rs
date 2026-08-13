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
    /// Open a fresh tab to a new URL — same trust surface as BrowserNavigate
    /// (a denied target would just be reached via `browser_open` if we did not
    /// gate it). Defaults to Ask because the user often wants to inspect the
    /// page that opened.
    BrowserOpen,
    /// Change a `<select>` value — single click on a picker; same policy
    /// surface as BrowserClick but typed so the prompt can read `select`
    /// clearly in the audit log.
    BrowserSelect,
    /// Accept or dismiss a native browser dialog (alert/confirm/prompt/
    /// beforeunload). Dismiss is benign; accept on a prompt can submit text
    /// into the page so it inherits BrowserType-style scrutiny.
    BrowserDialog,
    /// Press a single keyboard key (no payload, just a key code). Defaults to
    /// Allow because the user is normally the one driving it; tightening is
    /// a per-policy choice.
    BrowserPressKey,
    /// Scroll the page. Reading-related motion only; default Allow.
    BrowserScroll,
    /// Hover over a target. Read-only observation in practice; default Allow.
    BrowserHover,
    /// Drag from one element to another. Behaviour-changing and frequently
    /// abused for click spoofing on page coordinates; default Ask.
    BrowserDrag,
    /// Upload local files into a `<input type=file>` (or equivalent
    /// `chooser`). Sends host data to an arbitrary URL the page hosts the
    /// form on — privacy-sensitive, default Ask.
    BrowserUpload,
    /// Set / delete / clear cookies. A cookie value is a credential by
    /// design (session id, auth token); writing them is the highest-impact
    /// browser mutation, default Ask so a deny-by-default policy works.
    BrowserCookiesWrite,
    /// Attach caller-chosen HTTP headers to every request the page makes, or
    /// rewrite the user agent (`browser_emulate`). Canonically an
    /// `Authorization: Bearer …`, i.e. the same request-level credential
    /// write [`Self::BrowserCookiesWrite`] names — and it was classified as
    /// that variant for exactly that reason. The trust surface was right; the
    /// name an operator read in the policy file was not, and a policy file is
    /// read by people. The presentation-only overrides (color scheme,
    /// geolocation, network condition, CPU throttle) are NOT classified here:
    /// they carry no credential, and gating them would train the user to click
    /// through.
    BrowserIdentityOverride,
    /// Save or restore a whole browser storage state — every cookie plus
    /// localStorage — to or from a file (`browser_session`). `save` writes an
    /// entire authenticated identity to disk; `load` installs one into the
    /// live browser. Also formerly [`Self::BrowserCookiesWrite`]: leaving the
    /// bulk operation ungated while the single-cookie one asked would have
    /// made the gate trivially avoidable.
    BrowserSessionState,
    /// Edit / install / uninstall event hooks that fire arbitrary commands
    /// or HTTP requests on lifecycle events. Hooks are a control-plane
    /// write, hence operator-tier defaults already cover `hooks_manage` at
    /// the channel gate — this ActionType closes the loop so the policy
    /// engine can also gate it for any caller (even when channel tier is
    /// not in play).
    HooksManage,
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

impl ActionType {
    /// The action whose configured decision covers this one when a policy file
    /// does not name it.
    ///
    /// A policy file **replaces** the curated defaults rather than merging with
    /// them ([`ConfigApprovalPolicy::load_from`](super::ConfigApprovalPolicy)),
    /// so splitting a variant in two silently loosens every deployment that
    /// configured the old name: an operator with
    /// `"browser_cookies_write": "deny"` was denying header/user-agent
    /// overrides and storage-state moves too, and after the split those keys
    /// would be unmentioned and fall through to Ask. Inheriting keeps the
    /// operator's stated intent while letting them tighten or loosen the new
    /// names individually.
    ///
    /// Deliberately one level deep and acyclic: the chain exists to preserve a
    /// rename, not to build a taxonomy.
    #[must_use]
    pub fn inherited_from(&self) -> Option<Self> {
        match self {
            Self::BrowserIdentityOverride | Self::BrowserSessionState => {
                Some(Self::BrowserCookiesWrite)
            }
            _ => None,
        }
    }
}

impl fmt::Display for ActionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::BrowserNavigate => "browser navigate",
            Self::BrowserClick => "browser click",
            Self::BrowserType => "browser type",
            Self::BrowserFill => "browser fill",
            Self::BrowserEvaluate => "browser evaluate",
            Self::BrowserOpen => "browser open",
            Self::BrowserSelect => "browser select",
            Self::BrowserDialog => "browser dialog",
            Self::BrowserPressKey => "browser press key",
            Self::BrowserScroll => "browser scroll",
            Self::BrowserHover => "browser hover",
            Self::BrowserDrag => "browser drag",
            Self::BrowserUpload => "browser upload",
            Self::BrowserCookiesWrite => "browser cookies write",
            Self::BrowserIdentityOverride => "browser identity override",
            Self::BrowserSessionState => "browser session state",
            Self::HooksManage => "hooks manage",
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
