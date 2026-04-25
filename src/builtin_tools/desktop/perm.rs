//! `desktop_check_permissions` — query TCC permission status for the kinds
//! that Aleph commonly depends on.
//!
//! When a kind is not granted, subsequent desktop calls return errors whose
//! `data` field contains a full `PermissionGuide` with `deep_link`,
//! `human_readable_steps`, and `rationale`.  The LLM **must** relay those
//! fields to the user rather than simply saying "permission denied".

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
    /// most commonly needs: Accessibility, InputMonitoring, ScreenRecording,
    /// Camera, Microphone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<PermissionKind>>,
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// LLM tool: check macOS TCC permission status.
#[derive(Clone, Default)]
pub struct DesktopCheckPermissions {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopCheckPermissions {
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
        "Check macOS TCC permission status for the kinds Aleph depends on.\n",
        "\n",
        "Returns a list of `PermissionStatus` objects (one per kind) plus an\n",
        "`all_granted` boolean.  When a permission is not granted, subsequent\n",
        "desktop calls will return errors whose `data` field carries a full\n",
        "`PermissionGuide` with three actionable fields:\n",
        "  • `deep_link`            – x-apple.systempreferences: URL to open directly\n",
        "  • `human_readable_steps` – 3-5 step-by-step instructions\n",
        "  • `rationale`            – one sentence explaining why Aleph needs it\n",
        "\n",
        "When relaying a permission error to the user, ALWAYS include the rationale,\n",
        "the numbered steps, and the deep link.  Do NOT just say \"permission denied\"."
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

        let kinds: Vec<PermissionKind> = args
            .kinds
            .unwrap_or_else(|| DEFAULT_KINDS.to_vec());

        let mut statuses: Vec<ProtocolPermissionStatus> = Vec::with_capacity(kinds.len());
        for kind in &kinds {
            match perm_cap.check_permission(*kind).await {
                Ok(s) => statuses.push(s),
                Err(e) => {
                    // If bridge not implemented (non-macOS), surface a helpful message.
                    return Ok(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!(
                            "Permission check for {kind:?} failed: {e}"
                        )),
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
