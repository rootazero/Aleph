//! Types for TCC permission management.

use aleph_protocol::desktop_bridge::methods::perm::PermissionKind;
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

    /// Map this native TCC kind to its protocol counterpart, if any.
    ///
    /// Returns `None` for `SpeechRecognition` and `Notifications` — neither
    /// is currently represented in `PermissionKind` (the bridge-side enum
    /// covers UI-flow permissions only).
    pub fn to_protocol_kind(self) -> Option<PermissionKind> {
        match self {
            TccPermission::ScreenRecording => Some(PermissionKind::ScreenRecording),
            TccPermission::Camera => Some(PermissionKind::Camera),
            TccPermission::Microphone => Some(PermissionKind::Microphone),
            TccPermission::Accessibility => Some(PermissionKind::Accessibility),
            TccPermission::SpeechRecognition | TccPermission::Notifications => None,
        }
    }
}

/// Map a protocol `PermissionKind` to its native TCC counterpart, if any.
///
/// Returns `None` for kinds that have no cheap native-side check — the
/// bridge path is authoritative for those (FullDisk, InputMonitoring,
/// Automation, Contacts, Calendars, Reminders, Photos, Location).
pub fn permission_kind_to_tcc(kind: PermissionKind) -> Option<TccPermission> {
    match kind {
        PermissionKind::ScreenRecording => Some(TccPermission::ScreenRecording),
        PermissionKind::Camera => Some(TccPermission::Camera),
        PermissionKind::Microphone => Some(TccPermission::Microphone),
        PermissionKind::Accessibility => Some(TccPermission::Accessibility),
        PermissionKind::InputMonitoring
        | PermissionKind::FullDisk
        | PermissionKind::Automation
        | PermissionKind::Contacts
        | PermissionKind::Calendars
        | PermissionKind::Reminders
        | PermissionKind::Photos
        | PermissionKind::Location => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcc_round_trips_through_protocol_for_overlapping_kinds() {
        let overlapping = [
            TccPermission::ScreenRecording,
            TccPermission::Camera,
            TccPermission::Microphone,
            TccPermission::Accessibility,
        ];
        for tcc in overlapping {
            let kind = tcc.to_protocol_kind().expect("must map");
            let back = permission_kind_to_tcc(kind).expect("must map back");
            assert_eq!(back, tcc, "round-trip failed for {tcc:?}");
        }
    }

    #[test]
    fn tcc_only_kinds_return_none_on_protocol_map() {
        assert!(TccPermission::SpeechRecognition.to_protocol_kind().is_none());
        assert!(TccPermission::Notifications.to_protocol_kind().is_none());
    }

    #[test]
    fn bridge_only_kinds_return_none_on_tcc_map() {
        let bridge_only = [
            PermissionKind::InputMonitoring,
            PermissionKind::FullDisk,
            PermissionKind::Automation,
            PermissionKind::Contacts,
            PermissionKind::Calendars,
            PermissionKind::Reminders,
            PermissionKind::Photos,
            PermissionKind::Location,
        ];
        for kind in bridge_only {
            assert!(
                permission_kind_to_tcc(kind).is_none(),
                "expected None for bridge-only kind {kind:?}"
            );
        }
    }

    #[test]
    fn all_tcc_variants_handled_by_forward_map() {
        // Drift guard — if a new TccPermission variant is added without
        // updating `to_protocol_kind`, this test exercises every variant
        // and the match in `to_protocol_kind` is non-exhaustive → compile
        // fails before this assertion is even reached.
        for &tcc in TccPermission::ALL {
            let _ = tcc.to_protocol_kind();
        }
    }
}
