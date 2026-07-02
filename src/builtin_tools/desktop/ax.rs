//! Accessibility (AX) query tools — LLM-callable wrappers over
//! `DesktopPlatform::ax()` for the three read-only AX operations.
//!
//! Each tool is a separate [`AlephTool`] so the LLM sees a small, focused
//! surface (`desktop_ax_query_focused`, `desktop_ax_query_tree`,
//! `desktop_ax_query_by_role`) rather than a catch-all `desktop_ax` verb.
//!
//! All tools degrade gracefully when no `DesktopPlatform` is configured
//! (e.g. headless server builds) or when the platform does not implement
//! `AccessibilityCapability` (non-macOS today) — they return a structured
//! `DesktopOutput { success: false, message: "..." }` instead of erroring.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryByRoleParams, QueryTreeParams};

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::interactable::{collect_interactable, usable_bounds};
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
        message: Some(super::recovery::with_hint(format!("AX query failed: {err}"))),
    }
}

// ── desktop_ax_query_focused ────────────────────────────────────────────────

/// LLM tool: return the currently focused AX element.
#[derive(Clone, Default)]
pub struct DesktopAxQueryFocused {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryFocused {
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
impl AlephTool for DesktopAxQueryFocused {
    const NAME: &'static str = "desktop_ax_query_focused";
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

// ── desktop_ax_query_tree ───────────────────────────────────────────────────

/// LLM tool: return the full AX subtree rooted at a process.
#[derive(Clone, Default)]
pub struct DesktopAxQueryTree {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryTree {
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
impl AlephTool for DesktopAxQueryTree {
    const NAME: &'static str = "desktop_ax_query_tree";
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

// ── desktop_ax_query_by_role ────────────────────────────────────────────────

/// LLM tool: collect all AX elements matching a given role.
#[derive(Clone, Default)]
pub struct DesktopAxQueryByRole {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryByRole {
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
impl AlephTool for DesktopAxQueryByRole {
    const NAME: &'static str = "desktop_ax_query_by_role";
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

// ── desktop_ax_snapshot ─────────────────────────────────────────────────────

const SNAPSHOT_DEFAULT_DEPTH: u32 = 16;
const SNAPSHOT_MAX_DEPTH: u32 = 32;
const SNAPSHOT_DEFAULT_ELEMENTS: usize = 200;
const SNAPSHOT_MAX_ELEMENTS: usize = 500;
const SNAPSHOT_TEXT_CAP: usize = 120;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxSnapshotArgs {
    /// Target process ID. Omit to snapshot the frontmost application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Maximum AX subtree depth to walk (default 16, capped at 32).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum number of elements to return (default 200, capped at 500).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_elements: Option<usize>,
}

/// Round to one decimal place — click targets do not need sub-pixel precision
/// and shorter numbers keep the response compact.
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Char-safe truncation with an ellipsis marker.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let kept: String = s.chars().take(max_chars).collect();
        format!("{kept}…")
    } else {
        s.to_string()
    }
}

/// Render one element as a compact JSON object with a pre-computed click
/// `center`, so the model never has to derive coordinates itself.
fn element_to_json(index: usize, el: &AxElement) -> serde_json::Value {
    let (x, y, w, h) = usable_bounds(el).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let mut map = serde_json::Map::new();
    map.insert("index".into(), json!(index));
    map.insert("role".into(), json!(el.role));
    map.insert(
        "center".into(),
        json!([round1(x + w / 2.0), round1(y + h / 2.0)]),
    );
    if let Some(name) = el.title.as_deref().filter(|s| !s.trim().is_empty()) {
        map.insert("name".into(), json!(truncate(name, SNAPSHOT_TEXT_CAP)));
    }
    if let Some(value) = el.value.as_deref().filter(|s| !s.trim().is_empty()) {
        map.insert("value".into(), json!(truncate(value, SNAPSHOT_TEXT_CAP)));
    }
    serde_json::Value::Object(map)
}

/// Flatten an AX tree into an indexed list of interactable elements,
/// returning `(elements, truncated)` where `truncated` is set when the list
/// was cut to `max_elements`.
fn flatten_interactable(root: &AxElement, max_elements: usize) -> (Vec<serde_json::Value>, bool) {
    let mut collected: Vec<&AxElement> = Vec::new();
    collect_interactable(root, &mut collected);
    let truncated = collected.len() > max_elements;
    let elements = collected
        .into_iter()
        .take(max_elements)
        .enumerate()
        .map(|(index, el)| element_to_json(index, el))
        .collect();
    (elements, truncated)
}

/// LLM tool: snapshot an app's interactable UI as a flat, indexed element
/// list with ready-to-click coordinates (a "set of marks" for the GUI).
#[derive(Clone, Default)]
pub struct DesktopAxSnapshot {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxSnapshot {
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
impl AlephTool for DesktopAxSnapshot {
    const NAME: &'static str = "desktop_ax_snapshot";
    const DESCRIPTION: &'static str =
        "Snapshot an application's interactable UI as a flat, indexed element list — the \
         reliable way to drive a GUI. Each element carries its accessibility `role`, a \
         `name`/`value`, and a pre-computed `center` [x, y]; pass that `center` straight \
         to the `desktop` tool's `click` / `double_click` / `hover` actions. Prefer this \
         over eyeballing pixel coordinates from a screenshot. Omit `pid` to snapshot the \
         frontmost app. macOS only — requires Accessibility permission.";

    type Args = DesktopAxSnapshotArgs;
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
        let max_depth = args
            .max_depth
            .unwrap_or(SNAPSHOT_DEFAULT_DEPTH)
            .min(SNAPSHOT_MAX_DEPTH);
        let max_elements = args
            .max_elements
            .unwrap_or(SNAPSHOT_DEFAULT_ELEMENTS)
            .min(SNAPSHOT_MAX_ELEMENTS);
        let params = QueryTreeParams {
            pid: args.pid,
            max_depth,
        };
        match ax.query_tree(params).await {
            Ok(Some(root)) => {
                let (elements, truncated) = flatten_interactable(&root, max_elements);
                Ok(DesktopOutput {
                    success: true,
                    data: Some(json!({
                        "app_pid": root.pid,
                        "count": elements.len(),
                        "truncated": truncated,
                        "elements": elements,
                    })),
                    message: None,
                })
            }
            Ok(None) => Ok(DesktopOutput {
                success: true,
                data: Some(json!({ "count": 0, "truncated": false, "elements": [] })),
                message: Some("No accessible UI tree for the target application.".to_string()),
            }),
            Err(e) => Ok(bridge_err_output(e)),
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::desktop_bridge::methods::screen::Region;

    fn leaf(role: &str, title: Option<&str>, bounds: Option<Region>) -> AxElement {
        AxElement {
            role: role.to_string(),
            title: title.map(str::to_string),
            value: None,
            bounds,
            pid: 1,
            children: Vec::new(),
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

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

    #[tokio::test]
    async fn snapshot_without_platform_returns_message() {
        let tool = DesktopAxSnapshot::new();
        let out = tool
            .call(DesktopAxSnapshotArgs {
                pid: None,
                max_depth: None,
                max_elements: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[test]
    fn flatten_keeps_only_interactable_elements_with_bounds() {
        let tree = AxElement {
            role: "AXWindow".to_string(),
            title: None,
            value: None,
            bounds: Some(rect(0.0, 0.0, 800.0, 600.0)),
            pid: 7,
            children: vec![
                leaf(
                    "AXButton",
                    Some("Save"),
                    Some(rect(100.0, 200.0, 80.0, 40.0)),
                ),
                // Decorative text — not interactable, dropped.
                leaf(
                    "AXStaticText",
                    Some("label"),
                    Some(rect(0.0, 0.0, 50.0, 20.0)),
                ),
                // Interactable role but no bounds — cannot be clicked, dropped.
                leaf("AXTextField", None, None),
                // Interactable role but degenerate rect — dropped.
                leaf("AXButton", Some("ghost"), Some(rect(5.0, 5.0, 0.0, 0.0))),
            ],
        };
        let (elements, truncated) = flatten_interactable(&tree, 200);
        assert!(!truncated);
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0]["role"], "AXButton");
        assert_eq!(elements[0]["name"], "Save");
        assert_eq!(elements[0]["index"], 0);
        // center = top-left + size/2 = (100+40, 200+20).
        assert_eq!(elements[0]["center"], json!([140.0, 220.0]));
    }

    #[test]
    fn flatten_truncates_and_flags_at_limit() {
        let children: Vec<AxElement> = (0..5)
            .map(|i| {
                leaf(
                    "AXButton",
                    Some("btn"),
                    Some(rect(i as f64 * 10.0, 0.0, 10.0, 10.0)),
                )
            })
            .collect();
        let tree = AxElement {
            role: "AXWindow".to_string(),
            title: None,
            value: None,
            bounds: None,
            pid: 1,
            children,
        };
        let (elements, truncated) = flatten_interactable(&tree, 3);
        assert!(truncated);
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[2]["index"], 2);
    }
}
