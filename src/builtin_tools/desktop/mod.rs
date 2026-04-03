//! Desktop tool — sees and controls the desktop via the platform-native
//! `desktop/*` capability layer.

mod native;
mod types;

#[cfg(test)]
mod tests;

pub use types::{DesktopArgs, DesktopOutput};

use crate::sync_primitives::Arc;

use async_trait::async_trait;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::error::Result;
use crate::tools::AlephTool;

/// Desktop tool — gives the AI agent eyes and hands on the desktop.
#[derive(Clone)]
pub struct DesktopTool {
    pub(super) approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            approval_policy: None,
            platform: None,
        }
    }

    /// Attach a desktop platform for in-process execution via capability traits.
    ///
    /// When set, supported screen actions are dispatched through
    /// `platform.screen()` (the `ScreenCapability` trait) before falling back
    /// to the in-process `DesktopCapability` path.
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
    async fn check_approval(&self, action_type: ActionType, target: &str) -> Option<DesktopOutput> {
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

    fn no_capability_output(&self) -> DesktopOutput {
        DesktopOutput {
            success: false,
            data: None,
            message: Some(
                "Desktop platform capability is not configured for this server build.".to_string(),
            ),
        }
    }

    fn unsupported_action_output(&self, args: &DesktopArgs) -> DesktopOutput {
        let message = match args.action.as_str() {
            "click" if args.ref_id.is_some() => {
                "Ref-based desktop click is not available in the current `desktop/*` platform capability layer. Use explicit x/y coordinates.".to_string()
            }
            "type_text" if args.ref_id.is_some() => {
                "Ref-based desktop typing is not available in the current `desktop/*` platform capability layer. Focus the target first, then call `type_text`.".to_string()
            }
            "scroll" if args.ref_id.is_some() => {
                "Ref-based desktop scrolling is not available in the current `desktop/*` platform capability layer. Use delta_x/delta_y without ref targeting.".to_string()
            }
            "click" => "Desktop action 'click' requires explicit x/y coordinates in the current `desktop/*` platform capability layer.".to_string(),
            "snapshot" | "ax_tree" | "canvas_show" | "canvas_hide" | "canvas_update" => {
                format!(
                    "Desktop action '{}' is a legacy desktop bridge feature and is not implemented by the current `desktop/*` platform capability layer.",
                    args.action
                )
            }
            "double_click" | "drag" | "hover" | "paste" => {
                format!(
                    "Desktop action '{}' is not implemented by the current `desktop/*` platform capability layer.",
                    args.action
                )
            }
            _ => format!(
                "Desktop action '{}' is not supported on this platform/build.",
                args.action
            ),
        };

        DesktopOutput {
            success: false,
            data: None,
            message: Some(message),
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
    const DESCRIPTION: &'static str = r#"Control the desktop via platform-native `desktop/*` capabilities.

Currently supported:
- screenshot: Capture screen as base64 PNG. Optional region: {x,y,width,height}
- ocr: Extract text from screen or a provided image_base64
- click: Click explicit x/y coordinates. Optional button.
- scroll: Scroll via delta_x/delta_y
- type_text: Type text at the current focus target
- key_combo: Press key combo, e.g. keys=["cmd","c"]
- launch_app: Launch by bundle_id
- window_list: List open windows
- focus_window: Bring a window to front
- screen_record: Record screen as MP4. Optional duration/fps/with_audio

Legacy bridge-only actions such as snapshot, ax_tree, canvas_*, ref-based targeting,
double_click, drag, hover, and paste are not implemented by the current platform layer.

Examples:
{"action":"click","x":500,"y":300}
{"action":"scroll","delta_y":-300}
{"action":"type_text","text":"Hello"}
{"action":"screen_record","duration":3.0,"fps":30}"#;

    type Args = DesktopArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Check approval for sensitive (mutating) actions before attempting
        // desktop execution. Read-only actions skip approval.
        let approval_check = match args.action.as_str() {
            "click" => Some((
                ActionType::DesktopClick,
                format!("({},{})", args.x.unwrap_or(0.0), args.y.unwrap_or(0.0)),
            )),
            "type_text" => Some((
                ActionType::DesktopType,
                args.text.clone().unwrap_or_default(),
            )),
            "key_combo" => Some((
                ActionType::DesktopKeyCombo,
                args.keys.as_ref().map(|k| k.join("+")).unwrap_or_default(),
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
            "drag" => Some((ActionType::DesktopClick, "drag operation".to_string())),
            "hover" => Some((
                ActionType::DesktopClick,
                format!("hover({},{})", args.x.unwrap_or(0.0), args.y.unwrap_or(0.0)),
            )),
            "scroll" => Some((ActionType::DesktopClick, "scroll operation".to_string())),
            "paste" => Some((
                ActionType::DesktopType,
                args.text.clone().unwrap_or_default(),
            )),
            // Read-only actions skip approval
            "screenshot" | "ocr" | "ax_tree" | "window_list" | "focus_window" | "canvas_show"
            | "canvas_hide" | "canvas_update" | "snapshot" => None,
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

        // Execute via platform (single path).
        if let Some(ref platform) = self.platform {
            if let Some(output) = self.call_via_platform(platform, &args).await? {
                return Ok(output);
            }
        }

        if self.platform.is_none() {
            return Ok(self.no_capability_output());
        }

        Ok(self.unsupported_action_output(&args))
    }
}
