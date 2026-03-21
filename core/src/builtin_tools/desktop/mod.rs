//! Desktop Bridge tool — sees and controls the desktop via the Desktop Bridge.
//!
//! Requires the Aleph Desktop Bridge to be connected. When the bridge is absent,
//! all operations return a friendly message instead of an error.

mod native;
mod request;
mod types;

#[cfg(test)]
mod tests;

pub use types::{DesktopArgs, DesktopOutput};

use crate::sync_primitives::Arc;

use async_trait::async_trait;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::desktop::DesktopBridgeClient;
use crate::error::Result;
use crate::tools::AlephTool;

/// Desktop Bridge tool — gives the AI agent eyes and hands on the desktop.
#[derive(Clone)]
pub struct DesktopTool {
    pub(super) client: DesktopBridgeClient,
    pub(super) approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    pub(super) native: Option<Arc<dyn aleph_desktop::DesktopCapability>>,
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
            approval_policy: None,
            native: None,
            platform: None,
        }
    }

    /// Attach a native desktop capability for in-process execution.
    ///
    /// When set, supported actions (screenshot, ocr, click, type_text,
    /// key_combo, scroll, window_list, focus_window, launch_app) use the
    /// native in-process path instead of IPC to the Desktop Bridge.
    /// Unsupported actions (canvas_*, snapshot, PIM, etc.) fall through
    /// to the IPC bridge automatically.
    pub fn with_native(mut self, native: Arc<dyn aleph_desktop::DesktopCapability>) -> Self {
        self.native = Some(native);
        self
    }

    /// Attach a desktop platform for in-process execution via capability traits.
    ///
    /// When set, supported screen actions are dispatched through
    /// `platform.screen()` (the `ScreenCapability` trait) before falling back
    /// to the legacy `DesktopCapability` path or the IPC bridge.
    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }

    /// Attach an approval policy to gate sensitive actions.
    ///
    /// When a policy is set, mutating actions (click, type_text, key_combo,
    /// launch_app) are checked before execution. Read-only actions (screenshot,
    /// ocr, ax_tree, window_list, focus_window, canvas_*) are always allowed.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Check the approval policy for a sensitive action.
    ///
    /// Returns `None` if the action is allowed (or no policy is configured),
    /// or `Some(DesktopOutput)` if the action is denied or requires user
    /// confirmation.
    async fn check_approval(
        &self,
        action_type: ActionType,
        target: &str,
    ) -> Option<DesktopOutput> {
        let policy = self.approval_policy.as_ref()?;

        let request = ActionRequest {
            action_type,
            target: target.to_string(),
            agent_id: String::new(), // TODO: plumb agent_id from agent loop call context
            context: String::new(),  // TODO: populate with action description for audit
            timestamp: chrono::Utc::now(),
        };

        let decision = policy.check(&request).await;

        match decision {
            ApprovalDecision::Allow => {
                policy.record(&request, &decision).await;
                None
            }
            ApprovalDecision::Deny { ref reason } => {
                policy.record(&request, &decision).await;
                Some(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Action denied by approval policy: {reason}")),
                })
            }
            ApprovalDecision::Ask { ref prompt } => {
                // Don't record yet — record() should be called after user responds
                Some(DesktopOutput {
                    success: false,
                    data: Some(serde_json::json!({
                        "approval_required": true,
                        "prompt": prompt,
                    })),
                    message: Some(format!("Approval required: {prompt}")),
                })
            }
        }
    }
}

impl Default for DesktopTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for DesktopTool {
    const NAME: &'static str = "desktop";
    const DESCRIPTION: &'static str = r#"Control the desktop: screenshot, OCR, UI snapshot with element refs, keyboard/mouse, canvas overlays.

Requires the Aleph Desktop Bridge (starts automatically with the server).

Perception:
- snapshot: Capture UI structure with element refs. Returns tree (text), refs map, interactive list. Use BEFORE click/scroll/drag to target elements by ref.
- screenshot: Capture screen as base64 PNG. Optional region: {x,y,width,height}
- ocr: Extract text from screen (or provided image_base64). Returns {text, lines[]}
- ax_tree: Raw accessibility tree (for debugging, prefer snapshot)

Actions (support ref OR x/y targeting):
- click: Click element. ref="e3" (from snapshot) or x/y coordinates. Optional button.
- double_click: Double-click element. Same targeting as click.
- scroll: Scroll at element. ref or x/y + delta_x/delta_y (pixels, negative=up/left).
- drag: Drag between elements. start_ref/end_ref or start_x,y/end_x,y + duration_ms.
- hover: Move mouse to element without clicking. ref or x/y.
- type_text: Type text string. Optional ref to focus element first.
- key_combo: Press key combo, e.g. keys=["cmd","c"]
- paste: Write text to clipboard and paste (Cmd+V).
- launch_app: Launch by bundle_id
- window_list: List open windows
- focus_window: Bring window to front

Canvas:
- canvas_show/canvas_hide/canvas_update: HTML overlay with A2UI patches.

Examples:
{"action":"snapshot"}
{"action":"click","ref":"e3"}
{"action":"click","x":500,"y":300}
{"action":"scroll","ref":"e7","delta_y":-300}
{"action":"double_click","ref":"e1"}
{"action":"drag","start_ref":"e5","end_ref":"e12"}
{"action":"hover","ref":"e3"}
{"action":"type_text","ref":"e1","text":"Hello"}
{"action":"paste","text":"clipboard content"}"#;

    type Args = DesktopArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Check approval for sensitive (mutating) actions BEFORE touching
        // the bridge. A denied action should be rejected immediately
        // regardless of bridge availability. Read-only actions (screenshot,
        // ocr, ax_tree, window_list, focus_window, canvas_*) skip approval.
        let approval_check = match args.action.as_str() {
            "click" => Some((
                ActionType::DesktopClick,
                format!(
                    "({},{})",
                    args.x.unwrap_or(0.0),
                    args.y.unwrap_or(0.0)
                ),
            )),
            "type_text" => Some((
                ActionType::DesktopType,
                args.text.clone().unwrap_or_default(),
            )),
            "key_combo" => Some((
                ActionType::DesktopKeyCombo,
                args.keys
                    .as_ref()
                    .map(|k| k.join("+"))
                    .unwrap_or_default(),
            )),
            "launch_app" => Some((
                ActionType::DesktopLaunchApp,
                args.bundle_id.clone().unwrap_or_default(),
            )),
            // All other mutating actions require approval through DesktopClick as catch-all.
            // Read-only actions (screenshot, ocr, ax_tree, window_list, focus_window, canvas_*)
            // are listed explicitly below to skip approval.
            "double_click" => Some((
                ActionType::DesktopClick,
                format!(
                    "double_click({},{})",
                    args.x.unwrap_or(0.0),
                    args.y.unwrap_or(0.0)
                ),
            )),
            "drag" => Some((
                ActionType::DesktopClick,
                "drag operation".to_string(),
            )),
            "hover" => Some((
                ActionType::DesktopClick,
                format!(
                    "hover({},{})",
                    args.x.unwrap_or(0.0),
                    args.y.unwrap_or(0.0)
                ),
            )),
            "scroll" => Some((
                ActionType::DesktopClick,
                "scroll operation".to_string(),
            )),
            "paste" => Some((
                ActionType::DesktopType,
                args.text.clone().unwrap_or_default(),
            )),
            // Read-only actions skip approval
            "screenshot" | "ocr" | "ax_tree" | "window_list" | "focus_window"
            | "canvas_show" | "canvas_hide" | "canvas_update" | "snapshot" => None,
            // Unknown actions default to requiring approval for safety
            _ => Some((
                ActionType::DesktopClick,
                format!("unknown action: {}", args.action),
            )),
        };

        if let Some((action_type, target)) = approval_check {
            if let Some(out) = self.check_approval(action_type, &target).await {
                return Ok(out);
            }
        }

        // Prefer DesktopPlatform.screen() for screen operations (new path).
        // If the action is not handled (returns Ok(None)), fall through.
        if let Some(ref platform) = self.platform {
            if let Some(output) = self.call_via_platform(platform, &args).await? {
                return Ok(output);
            }
        }

        // Legacy fallback: try the DesktopCapability in-process path.
        // If the action is not supported natively (call_native returns
        // Ok(None)), fall through to the IPC bridge below.
        if let Some(ref native) = self.native {
            if let Some(output) = self.call_native(native, &args).await? {
                return Ok(output);
            }
        }

        // Gracefully handle the case where the Desktop Bridge is not connected.
        if !self.client.is_available() {
            return Ok(DesktopOutput {
                success: false,
                data: None,
                message: Some(
                    "Desktop bridge not connected. The bridge provides screenshot, OCR, \
                     keyboard, and UI automation capabilities. It starts automatically \
                     with aleph, or can be run standalone via aleph-tauri."
                        .to_string(),
                ),
            });
        }

        let request = match request::build_request(&args) {
            Ok(r) => r,
            Err(msg) => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(msg),
                });
            }
        };

        match self.client.send(request).await {
            Ok(result) => Ok(DesktopOutput {
                success: true,
                data: Some(result),
                message: None,
            }),
            Err(e) => Ok(DesktopOutput {
                success: false,
                data: None,
                message: Some(e.to_string()),
            }),
        }
    }
}
