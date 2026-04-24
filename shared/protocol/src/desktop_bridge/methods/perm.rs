//! Permission detection and guidance — `perm.*` JSON-RPC method schemas.
//!
//! Three request methods + one notification:
//! - `perm.check`         → query current TCC status for a permission kind
//! - `perm.guide`         → get human-readable guidance + deep link for a kind
//! - `perm.open_settings` → open System Settings to the relevant pane
//! - `perm.status_changed` (notification) → pushed by the helper when TCC status changes

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_CHECK: &str = "perm.check";
pub const METHOD_GUIDE: &str = "perm.guide";
pub const METHOD_OPEN_SETTINGS: &str = "perm.open_settings";
pub const NOTIFY_STATUS_CHANGED: &str = "perm.status_changed";

/// Every macOS TCC permission kind that Aleph manages or depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Accessibility,
    InputMonitoring,
    ScreenRecording,
    FullDisk,
    Camera,
    Microphone,
    Automation,
    Contacts,
    Calendars,
    Reminders,
    Photos,
}

/// Current TCC status for one permission kind.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionStatus {
    pub kind: PermissionKind,
    pub granted: bool,
    /// `true` if the app can call a system API to request the permission;
    /// `false` if the user must grant it manually (e.g. Accessibility).
    pub can_request_programmatically: bool,
    /// `true` if an MDM or Parental Controls policy permanently blocks the permission.
    pub restricted: bool,
}

/// Actionable guidance the LLM can relay to the user when a permission is missing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionGuide {
    pub kind: PermissionKind,
    pub status: PermissionStatus,
    /// `x-apple.systempreferences:` deep link that opens directly to the relevant pane.
    pub deep_link: String,
    /// Step-by-step instructions in plain language.
    pub human_readable_steps: Vec<String>,
    /// One sentence explaining *why* Aleph needs this permission.
    pub rationale: String,
}

/// Parameters for `perm.check`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckParams {
    pub kind: PermissionKind,
}

/// Parameters for `perm.guide`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GuideParams {
    pub kind: PermissionKind,
}

/// Parameters for `perm.open_settings`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenSettingsParams {
    pub kind: PermissionKind,
}

/// Result for `perm.open_settings`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenSettingsResult {
    pub ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serialized_as_snake_case() {
        let j = serde_json::to_string(&PermissionKind::ScreenRecording).unwrap();
        assert_eq!(j, "\"screen_recording\"");
    }

    #[test]
    fn all_kinds_round_trip() {
        let kinds = [
            PermissionKind::Accessibility,
            PermissionKind::InputMonitoring,
            PermissionKind::ScreenRecording,
            PermissionKind::FullDisk,
            PermissionKind::Camera,
            PermissionKind::Microphone,
            PermissionKind::Automation,
            PermissionKind::Contacts,
            PermissionKind::Calendars,
            PermissionKind::Reminders,
            PermissionKind::Photos,
        ];
        for kind in kinds {
            let s = serde_json::to_string(&kind).unwrap();
            let back: PermissionKind = serde_json::from_str(&s).unwrap();
            assert_eq!(kind, back, "round-trip failed for {kind:?}");
        }
    }

    #[test]
    fn permission_status_serializes_fields() {
        let status = PermissionStatus {
            kind: PermissionKind::Camera,
            granted: false,
            can_request_programmatically: true,
            restricted: false,
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["kind"], "camera");
        assert_eq!(v["granted"], false);
        assert_eq!(v["can_request_programmatically"], true);
        assert_eq!(v["restricted"], false);
    }
}
