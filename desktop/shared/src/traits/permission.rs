//! Permission detection and request capability.

use async_trait::async_trait;

use aleph_protocol::desktop_bridge::methods::perm::{
    PermissionGuide, PermissionKind, PermissionStatus as ProtocolPermissionStatus,
};

use crate::permission_types::PermissionInfo;
use crate::Result;

/// Permission detection and request.
///
/// Provides read-only status checks (`check`, `check_all`) and
/// interactive permission requests (`request`) that may show
/// system dialogs.
///
/// `check` / `request` cover the TCC-managed subset (see
/// `permission_types::TCC_MANAGED`). For bridge-only kinds the implementation
/// returns `PermissionStatus::Unknown` — callers should use the richer
/// `check_permission` instead.
///
/// Three of the methods (`check_permission`, `guide_permission`,
/// `open_settings`) speak the protocol types from
/// `aleph_protocol::desktop_bridge::methods::perm`. All are REQUIRED: every
/// platform in the tree implements them (macOS via the Swift bridge, Linux via
/// freedesktop idioms, Windows via the `ConsentStore`). They once carried
/// `NotImplemented` default bodies so half-migrated platforms would still
/// compile; nothing is half-migrated any more, and a default that silently
/// answers `NotImplemented` would let a new platform ship a permission layer
/// that looks present and always fails.
#[async_trait]
pub trait PermissionCapability: Send + Sync {
    /// Check status of one permission without prompting the user.
    async fn check(&self, permission: PermissionKind) -> Result<PermissionInfo>;

    /// Check status of all TCC-managed permissions (see `TCC_MANAGED`).
    async fn check_all(&self) -> Result<Vec<PermissionInfo>>;

    /// Request a permission, potentially showing a system prompt.
    /// Returns the updated status after the request attempt.
    async fn request(&self, permission: PermissionKind) -> Result<PermissionInfo>;

    /// Query the current permission status.
    ///
    /// Returns a `ProtocolPermissionStatus` (from `aleph_protocol`) which
    /// carries `granted`, `can_request_programmatically`, and `restricted`.
    async fn check_permission(&self, kind: PermissionKind) -> Result<ProtocolPermissionStatus>;

    /// Retrieve a full `PermissionGuide` (deep link + steps + rationale).
    async fn guide_permission(&self, kind: PermissionKind) -> Result<PermissionGuide>;

    /// Open the OS settings pane for `kind`.
    /// Returns `true` if the URL was successfully opened.
    async fn open_settings(&self, kind: PermissionKind) -> Result<bool>;
}
