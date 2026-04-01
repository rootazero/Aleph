//! Types for TCC permission management.

use serde::{Deserialize, Serialize};

/// A macOS TCC permission type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TccPermission {
    ScreenRecording,
    Camera,
    Microphone,
    SpeechRecognition,
    Accessibility,
    Notifications,
}

impl TccPermission {
    /// All managed TCC permissions.
    pub const ALL: &'static [TccPermission] = &[
        TccPermission::ScreenRecording,
        TccPermission::Camera,
        TccPermission::Microphone,
        TccPermission::SpeechRecognition,
        TccPermission::Accessibility,
        TccPermission::Notifications,
    ];
}

/// Authorization status of a TCC permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    /// Permission granted.
    Granted,
    /// Permission denied (user explicitly denied or revoked).
    Denied,
    /// Permission not yet determined (never prompted).
    NotDetermined,
    /// Permission restricted by system policy (MDM, parental controls).
    Restricted,
    /// Cannot determine status on this platform.
    Unknown,
}

/// Information about a TCC permission's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionInfo {
    pub permission: TccPermission,
    pub status: PermissionStatus,
    /// Whether calling `request` will show a system prompt.
    pub can_request: bool,
}
