//! Permission detection and request capability.

use async_trait::async_trait;

use aleph_protocol::desktop_bridge::methods::perm::{
    PermissionGuide, PermissionKind, PermissionStatus as ProtocolPermissionStatus,
};

use crate::permission_types::{PermissionInfo, TccPermission};
use crate::Result;

/// TCC permission detection and request.
///
/// Provides read-only status checks (`check`, `check_all`) and
/// interactive permission requests (`request`) that may show
/// system dialogs.
///
/// Three additional methods (`check_permission`, `guide_permission`,
/// `open_settings`) use the bridge-backed protocol types from
/// `aleph_protocol::desktop_bridge::methods::perm`.  They have default
/// implementations that return `NotImplemented` so existing platform
/// implementations that have not yet migrated continue to compile.
#[async_trait]
pub trait PermissionCapability: Send + Sync {
    /// Check status of one permission without prompting the user.
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo>;

    /// Check status of all managed permissions.
    async fn check_all(&self) -> Result<Vec<PermissionInfo>>;

    /// Request a permission, potentially showing a system prompt.
    /// Returns the updated status after the request attempt.
    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo>;

    // -----------------------------------------------------------------------
    // Bridge-backed protocol methods (Stage 4).
    // Default impls return NotImplemented so non-macOS stubs compile as-is.
    // -----------------------------------------------------------------------

    /// Query the current TCC status via the Swift bridge.
    ///
    /// Returns a `ProtocolPermissionStatus` (from `aleph_protocol`) which
    /// carries `granted`, `can_request_programmatically`, and `restricted`.
    async fn check_permission(
        &self,
        kind: PermissionKind,
    ) -> Result<ProtocolPermissionStatus> {
        let _ = kind;
        Err(crate::error::DesktopError::NotImplemented(
            "check_permission".into(),
        ))
    }

    /// Retrieve a full `PermissionGuide` (deep link + steps + rationale) via
    /// the Swift bridge.
    async fn guide_permission(
        &self,
        kind: PermissionKind,
    ) -> Result<PermissionGuide> {
        let _ = kind;
        Err(crate::error::DesktopError::NotImplemented(
            "guide_permission".into(),
        ))
    }

    /// Open System Settings to the pane for `kind` via the Swift bridge.
    /// Returns `true` if the URL was successfully opened.
    async fn open_settings(&self, kind: PermissionKind) -> Result<bool> {
        let _ = kind;
        Err(crate::error::DesktopError::NotImplemented(
            "open_settings".into(),
        ))
    }
}
