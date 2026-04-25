//! Accessibility (AX) query tools — LLM-callable wrappers over
//! `DesktopPlatform::ax()` for the three read-only AX operations.
//!
//! Each tool is a separate [`AlephTool`] so the LLM sees a small, focused
//! surface (`desktop.ax_query_focused`, `desktop.ax_query_tree`,
//! `desktop.ax_query_by_role`) rather than a catch-all `desktop.ax` verb.
//!
//! All tools degrade gracefully when no `DesktopPlatform` is configured
//! (e.g. headless server builds) or when the platform does not implement
//! `AccessibilityCapability` (non-macOS today) — they return a structured
//! `DesktopOutput { success: false, message: "..." }` instead of erroring.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use aleph_protocol::desktop_bridge::methods::ax::{QueryByRoleParams, QueryTreeParams};

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::types::DesktopOutput;

// ── Argument types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxQueryFocusedArgs {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxQueryTreeArgs {
    /// Target process ID. Omit to query the frontmost application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Maximum subtree depth (default 6).  Larger values produce more JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxQueryByRoleArgs {
    /// AX role string to collect, e.g. `"AXButton"`, `"AXTextField"`.
    pub role: String,
    /// Target process ID. Omit to query the frontmost application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

// ── Shared helpers ───────────────────────────────────────────────────────────

fn no_platform_output() -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(
            "Desktop platform capability is not configured for this server build.".to_string(),
        ),
    }
}

fn no_ax_capability_output() -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some("Accessibility (AX) capability is not available on this platform.".into()),
    }
}

fn bridge_err_output(err: impl std::fmt::Display) -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(format!("AX query failed: {err}")),
    }
}

// ── desktop.ax_query_focused ────────────────────────────────────────────────

/// LLM tool: return the currently focused AX element.
#[derive(Clone, Default)]
pub struct DesktopAxQueryFocused {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryFocused {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopAxQueryFocused {
    const NAME: &'static str = "desktop.ax_query_focused";
    const DESCRIPTION: &'static str =
        "Return the UI element currently holding keyboard focus via the OS accessibility API. \
         Response contains an `element` field (null if no focused element). \
         macOS only — requires Accessibility permission.";

    type Args = DesktopAxQueryFocusedArgs;
    type Output = DesktopOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => return Ok(no_platform_output()),
        };
        let ax = match platform.ax() {
            Some(a) => a,
            None => return Ok(no_ax_capability_output()),
        };
        match ax.query_focused().await {
            Ok(element) => Ok(DesktopOutput {
                success: true,
                data: Some(json!({ "element": element })),
                message: None,
            }),
            Err(e) => Ok(bridge_err_output(e)),
        }
    }
}

// ── desktop.ax_query_tree ───────────────────────────────────────────────────

/// LLM tool: return the full AX subtree rooted at a process.
#[derive(Clone, Default)]
pub struct DesktopAxQueryTree {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopAxQueryTree {
    const NAME: &'static str = "desktop.ax_query_tree";
    const DESCRIPTION: &'static str =
        "Return the AX element tree for a process (the frontmost app if `pid` is omitted). \
         Bounded by `max_depth` (default 6). Response contains an `element` field \
         with nested `children`. macOS only — requires Accessibility permission.";

    type Args = DesktopAxQueryTreeArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => return Ok(no_platform_output()),
        };
        let ax = match platform.ax() {
            Some(a) => a,
            None => return Ok(no_ax_capability_output()),
        };
        let params = QueryTreeParams {
            pid: args.pid,
            max_depth: args.max_depth.unwrap_or(6),
        };
        match ax.query_tree(params).await {
            Ok(element) => Ok(DesktopOutput {
                success: true,
                data: Some(json!({ "element": element })),
                message: None,
            }),
            Err(e) => Ok(bridge_err_output(e)),
        }
    }
}

// ── desktop.ax_query_by_role ────────────────────────────────────────────────

/// LLM tool: collect all AX elements matching a given role.
#[derive(Clone, Default)]
pub struct DesktopAxQueryByRole {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryByRole {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopAxQueryByRole {
    const NAME: &'static str = "desktop.ax_query_by_role";
    const DESCRIPTION: &'static str =
        "Collect all AX elements whose role matches `role` (e.g. \"AXButton\") in a process. \
         If `pid` is omitted, the frontmost application is queried. Response contains an \
         `elements` array. macOS only — requires Accessibility permission.";

    type Args = DesktopAxQueryByRoleArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => return Ok(no_platform_output()),
        };
        let ax = match platform.ax() {
            Some(a) => a,
            None => return Ok(no_ax_capability_output()),
        };
        let params = QueryByRoleParams {
            role: args.role,
            pid: args.pid,
        };
        match ax.query_by_role(params).await {
            Ok(elements) => Ok(DesktopOutput {
                success: true,
                data: Some(json!({ "elements": elements })),
                message: None,
            }),
            Err(e) => Ok(bridge_err_output(e)),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn query_focused_without_platform_returns_message() {
        let tool = DesktopAxQueryFocused::new();
        let out = tool.call(DesktopAxQueryFocusedArgs {}).await.unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[tokio::test]
    async fn query_tree_without_platform_returns_message() {
        let tool = DesktopAxQueryTree::new();
        let out = tool
            .call(DesktopAxQueryTreeArgs {
                pid: None,
                max_depth: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[tokio::test]
    async fn query_by_role_without_platform_returns_message() {
        let tool = DesktopAxQueryByRole::new();
        let out = tool
            .call(DesktopAxQueryByRoleArgs {
                role: "AXButton".to_string(),
                pid: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }
}
