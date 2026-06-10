//! Permission identity + native-side authorization status types.
//!
//! Permission identity is centralized in [`aleph_protocol::desktop_bridge::methods::perm::PermissionKind`].
//! This module re-exports `PermissionKind` and exposes:
//! - [`PermissionStatus`] — authorization state (Granted/Denied/...).
//! - [`PermissionInfo`] — `{ permission: PermissionKind, status, can_request }`.
//! - [`TCC_MANAGED`] — the subset of kinds the native TCC path actually handles.
//!
//! Bridge-only kinds (Automation, `FullDisk`, Contacts, ...) are routed via the
//! Swift helper's `perm.*` RPCs; the native trait methods return `Unknown` for
//! those, so callers know to ask the bridge.

pub use aleph_protocol::desktop_bridge::methods::perm::PermissionKind;
use serde::{Deserialize, Serialize};

/// Subset of [`PermissionKind`] that the native macOS TCC path checks directly.
///
/// The remaining variants (`InputMonitoring`, `FullDisk`, `Automation`,
/// `Contacts`, `Calendars`, `Reminders`, `Photos`, `Location`) are handled via
/// the Swift helper's `perm.check` RPC.
pub const TCC_MANAGED: &[PermissionKind] = &[
    PermissionKind::ScreenRecording,
    PermissionKind::Camera,
    PermissionKind::Microphone,
    PermissionKind::SpeechRecognition,
    PermissionKind::Accessibility,
    PermissionKind::Notifications,
];

/// `true` if `kind` is one of the [`TCC_MANAGED`] permissions.
#[must_use]
pub fn is_tcc_managed(kind: PermissionKind) -> bool {
    TCC_MANAGED.contains(&kind)
}

/// Authorization status of a permission.
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
    /// Cannot determine status on this platform — usually means this kind is
    /// bridge-only and the caller should use the `perm.check` RPC instead.
    Unknown,
}

/// Information about a permission's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionInfo {
    pub permission: PermissionKind,
    pub status: PermissionStatus,
    /// Whether calling `request` will show a system prompt.
    pub can_request: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcc_managed_covers_six_native_kinds() {
        assert_eq!(TCC_MANAGED.len(), 6);
        for &kind in TCC_MANAGED {
            assert!(is_tcc_managed(kind));
        }
    }

    #[test]
    fn bridge_only_kinds_are_not_tcc_managed() {
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
                !is_tcc_managed(kind),
                "expected bridge-only kind {kind:?} to not be TCC-managed",
            );
        }
    }

    #[test]
    fn permission_info_serializes_with_kind() {
        let info = PermissionInfo {
            permission: PermissionKind::Microphone,
            status: PermissionStatus::Granted,
            can_request: false,
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["permission"], "microphone");
        assert_eq!(v["status"], "granted");
        assert_eq!(v["can_request"], false);
    }
}
