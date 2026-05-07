//! Desktop tool — sees and controls the desktop via the platform-native
//! `desktop/*` capability layer.

mod ax;
mod native;
mod perm;
pub mod session_lock;
mod types;

#[cfg(test)]
mod tests;

pub use ax::{
    DesktopAxQueryByRole, DesktopAxQueryByRoleArgs, DesktopAxQueryFocused,
    DesktopAxQueryFocusedArgs, DesktopAxQueryTree, DesktopAxQueryTreeArgs,
};
pub use perm::{DesktopCheckPermissions, DesktopCheckPermissionsArgs};
pub use types::{DesktopArgs, DesktopOutput};

use crate::sync_primitives::{Arc, Mutex};

use async_trait::async_trait;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::error::Result;
use crate::tools::AlephTool;

use session_lock::ComputerUseLock;

/// Desktop tool — gives the AI agent eyes and hands on the desktop.
#[derive(Clone)]
pub struct DesktopTool {
    pub(super) approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
    pub(super) session_lock: Option<Arc<Mutex<ComputerUseLock>>>,
    pub(super) escape_started: Arc<crate::sync_primitives::AtomicBool>,
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            approval_policy: None,
            platform: None,
            session_lock: None,
            escape_started: Arc::new(crate::sync_primitives::AtomicBool::new(false)),
        }
    }

    /// Attach a desktop platform for in-process execution via capability traits.
    ///
    /// When set, all screen actions are dispatched through
    /// `platform.screen()` (the `ScreenCapability` trait).
    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }

    /// Attach an approval policy to gate sensitive actions.
    ///
    /// When a policy is set, mutating actions (click, type_text, key_combo,
    /// launch_app, etc.) are checked before execution. Read-only actions
    /// (screenshot, ocr, window_list, cursor_position, etc.) are always allowed.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Attach a session ID for computer-use locking.
    pub fn with_session_id(mut self, session_id: &str) -> Self {
        self.session_lock = Some(Arc::new(Mutex::new(ComputerUseLock::new(session_id))));
        self
    }

    /// Acquire session lock for mutating actions.
    fn acquire_lock(&self) -> std::result::Result<(), DesktopOutput> {
        if let Some(ref lock) = self.session_lock {
            let mut guard = lock.lock().unwrap_or_else(|e| {
                tracing::warn!("Recovering from poisoned computer-use lock");
                e.into_inner()
            });
            if let Err(msg) = guard.acquire() {
                return Err(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(msg),
                });
            }
        }
        Ok(())
    }

    /// Check escape abort and start listener on first call.
    fn check_escape(&self) -> std::result::Result<(), DesktopOutput> {
        if let Some(ref platform) = self.platform {
            if let Some(listener) = platform.escape_listener() {
                if !self
                    .escape_started
                    .load(crate::sync_primitives::Ordering::Acquire)
                {
                    let _ = listener.start();
                    self.escape_started
                        .store(true, crate::sync_primitives::Ordering::Release);
                }
                if listener.is_aborted() {
                    return Err(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some("Computer use aborted by user (Escape pressed)".into()),
                    });
                }
            }
        }
        Ok(())
    }

    /// Execute a batch of desktop actions sequentially.
    async fn execute_batch(&self, args: &DesktopArgs) -> Result<DesktopOutput> {
        let actions = match &args.actions {
            Some(list) if !list.is_empty() => list,
            _ => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some("batch requires non-empty 'actions' array".into()),
                })
            }
        };

        let mut results = Vec::new();
        for (i, action_json) in actions.iter().enumerate() {
            // Check escape between actions
            if let Err(out) = self.check_escape() {
                results.push(serde_json::json!({
                    "index": i,
                    "aborted": true,
                    "message": out.message,
                }));
                break;
            }

            let sub_args: DesktopArgs = match serde_json::from_value(action_json.clone()) {
                Ok(a) => a,
                Err(e) => {
                    results.push(serde_json::json!({
                        "index": i,
                        "success": false,
                        "message": format!("Invalid action: {e}"),
                    }));
                    break;
                }
            };

            // Prevent nested batch
            if sub_args.action == "batch" {
                results.push(serde_json::json!({
                    "index": i,
                    "success": false,
                    "message": "Nested batch not allowed",
                }));
                break;
            }

            let output = AlephTool::call(self, sub_args).await?;
            let success = output.success;
            results.push(serde_json::json!({
                "index": i,
                "success": success,
                "data": output.data,
                "message": output.message,
            }));

            if !success {
                break;
            }
        }

        let overall_success = results
            .last()
            .and_then(|r| r["success"].as_bool())
            .unwrap_or(false);

        Ok(DesktopOutput {
            success: overall_success,
            data: Some(serde_json::json!({"results": results})),
            message: None,
        })
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
        DesktopOutput {
            success: false,
            data: None,
            message: Some(format!(
                "Desktop action '{}' is not supported on this platform.",
                args.action
            )),
        }
    }
}

/// Classify a desktop action for approval gating.
///
/// Returns `None` for read-only actions that skip approval,
/// or `Some((ActionType, target))` for actions that require approval.
fn classify_approval(args: &DesktopArgs) -> Option<(ActionType, String)> {
    match args.action.as_str() {
        "screenshot" | "ocr" | "window_list" | "cursor_position" | "clipboard_read"
        | "screen_record" | "focus_window" | "display_list" => None,

        "click" | "double_click" | "hover" | "mouse_button" => Some((
            ActionType::DesktopClick,
            format!(
                "{}({},{})",
                args.action,
                args.x.unwrap_or(0.0),
                args.y.unwrap_or(0.0)
            ),
        )),
        "drag" => Some((ActionType::DesktopClick, "drag".into())),
        "scroll" => Some((ActionType::DesktopClick, "scroll".into())),

        "type_text" | "clipboard_write" => Some((
            ActionType::DesktopType,
            args.text.clone().unwrap_or_default(),
        )),
        "key_combo" => Some((
            ActionType::DesktopKeyCombo,
            args.keys.as_ref().map(|k| k.join("+")).unwrap_or_default(),
        )),

        "launch_app" | "quit_app" => Some((
            ActionType::DesktopLaunchApp,
            args.bundle_id.clone().unwrap_or_default(),
        )),

        "batch" => Some((ActionType::DesktopClick, "batch operation".into())),
        "paste" => Some((
            ActionType::DesktopType,
            args.text.clone().unwrap_or_default(),
        )),

        _ => Some((
            ActionType::DesktopClick,
            format!("unknown: {}", args.action),
        )),
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
    const DESCRIPTION: &'static str = r#"Control the desktop — see the screen and interact with it.

Actions:
- screenshot: Capture screen as base64 PNG. Optional region: {x,y,width,height}
- ocr: Extract text from screen with bounding boxes. Optional image_base64.
- click: Click at (x, y). Optional button (left/right/middle).
- double_click: Double-click at (x, y). Optional button.
- drag: Drag from (start_x, start_y) to (end_x, end_y). Optional duration_ms for animation.
- hover: Move mouse to (x, y) without clicking.
- cursor_position: Get current mouse cursor position.
- mouse_button: Press/release mouse at (x, y). Requires press_action (press/release/click).
- type_text: Type text at current cursor position.
- key_combo: Press key combination, e.g. keys=["cmd","c"].
- scroll: Scroll via delta_x/delta_y.
- launch_app: Launch app by bundle_id.
- quit_app: Close app by bundle_id.
- window_list: List open windows.
- focus_window: Bring window to front by window_id.
- clipboard_read: Read clipboard text.
- clipboard_write: Write text to clipboard.
- screen_record: Record screen as MP4. Optional duration/fps/with_audio.
- display_list: List all connected displays with resolution and scale info.
- batch: Execute multiple actions sequentially. Requires actions array.
- paste: Paste text via clipboard (Cmd+V). Better for multiline text than type_text.

Examples:
{"action":"click","x":500,"y":300}
{"action":"double_click","x":500,"y":300}
{"action":"drag","start_x":100,"start_y":100,"end_x":500,"end_y":500,"duration_ms":300}
{"action":"hover","x":250,"y":250}
{"action":"cursor_position"}
{"action":"clipboard_read"}
{"action":"scroll","delta_y":-300}
{"action":"type_text","text":"Hello"}
{"action":"screen_record","duration":3.0,"fps":30}
{"action":"display_list"}
{"action":"batch","actions":[{"action":"click","x":100,"y":200},{"action":"type_text","text":"hello"}]}
{"action":"paste","text":"line1\nline2\nline3"}
{"action":"screenshot","format":"jpeg","quality":0.75,"max_width":1280}"#;

    type Args = DesktopArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let is_mutating = classify_approval(&args).is_some();

        // 1. Approval check
        if let Some((action_type, target)) = classify_approval(&args) {
            if let Some(out) = self.check_approval(action_type, &target).await {
                return Ok(out);
            }
        }

        // 2. Session lock (mutating actions only)
        if is_mutating {
            if let Err(out) = self.acquire_lock() {
                return Ok(out);
            }
        }

        // 3. Escape abort check (mutating actions only)
        if is_mutating {
            if let Err(out) = self.check_escape() {
                return Ok(out);
            }
        }

        // 4. Handle batch at this level (needs recursive self.call())
        if args.action == "batch" {
            return self.execute_batch(&args).await;
        }

        // 5. Execute via platform
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
