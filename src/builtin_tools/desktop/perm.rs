//! `desktop_check_permissions` — query desktop permission status from the OS.
//! Use the `permission` tool's `guide` action for platform-specific guidance.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use aleph_protocol::desktop_bridge::methods::perm::{
    PermissionKind, PermissionStatus as ProtocolPermissionStatus,
};

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::types::DesktopOutput;

// ── Argument type ────────────────────────────────────────────────────────────

/// Default set of permission kinds checked when `kinds` is omitted.
const DEFAULT_KINDS: &[PermissionKind] = &[
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
    PermissionKind::Camera,
    PermissionKind::Microphone,
];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopCheckPermissionsArgs {
    /// Which permission kinds to check.  Omit to check the five kinds Aleph
    /// most commonly needs: Accessibility, `InputMonitoring`, `ScreenRecording`,
    /// Camera, Microphone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<PermissionKind>>,
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// LLM tool: check desktop permission status on the current OS.
#[derive(Clone, Default)]
pub struct DesktopCheckPermissions {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopCheckPermissions {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopCheckPermissions {
    const NAME: &'static str = "desktop_check_permissions";
    const DESCRIPTION: &'static str = concat!(
        "Check desktop permission status on this OS (including TCC on macOS).\n",
        "Returns one status per kind plus `all_granted`. An `unknown` status is not a grant.\n",
        "For platform-specific instructions, call `permission` with `action:\"guide\"` and the ",
        "permission kind. Relay the rationale, steps, and settings link when provided."
    );

    type Args = DesktopCheckPermissionsArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(
                        "Desktop platform capability is not configured for this server build."
                            .to_string(),
                    ),
                });
            }
        };

        let perm_cap = match platform.permission() {
            Some(p) => p,
            None => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(
                        "Permission capability is not available on this platform.".to_string(),
                    ),
                });
            }
        };

        let kinds: Vec<PermissionKind> = args.kinds.unwrap_or_else(|| DEFAULT_KINDS.to_vec());

        let mut statuses: Vec<ProtocolPermissionStatus> = Vec::with_capacity(kinds.len());
        for kind in &kinds {
            match perm_cap.check_permission(*kind).await {
                Ok(s) => statuses.push(s),
                Err(e) => {
                    // Keep a failed check distinct from an OS-reported status.
                    return Ok(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Permission check for {kind:?} failed: {e}")),
                    });
                }
            }
        }

        let all_granted = statuses.iter().all(|s| s.granted);
        Ok(DesktopOutput {
            success: true,
            data: Some(json!({
                "statuses": statuses,
                "all_granted": all_granted,
            })),
            message: None,
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn check_permissions_without_platform_returns_message() {
        let tool = DesktopCheckPermissions::new();
        let out = tool
            .call(DesktopCheckPermissionsArgs { kinds: None })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[tokio::test]
    async fn default_kinds_constant_has_five_entries() {
        assert_eq!(DEFAULT_KINDS.len(), 5);
    }
}
