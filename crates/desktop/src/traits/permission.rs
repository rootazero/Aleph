//! Permission detection and request capability.

use async_trait::async_trait;

use crate::permission_types::{PermissionInfo, TccPermission};
use crate::Result;

/// TCC permission detection and request.
///
/// Provides read-only status checks (`check`, `check_all`) and
/// interactive permission requests (`request`) that may show
/// system dialogs.
#[async_trait]
pub trait PermissionCapability: Send + Sync {
    /// Check status of one permission without prompting the user.
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo>;

    /// Check status of all managed permissions.
    async fn check_all(&self) -> Result<Vec<PermissionInfo>>;

    /// Request a permission, potentially showing a system prompt.
    /// Returns the updated status after the request attempt.
    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo>;
}
