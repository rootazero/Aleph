//! Desktop tool — sees and controls the desktop via the platform-native
//! `desktop/*` capability layer.

mod action_script;
mod ax;
mod browser_operator;
mod coord_resolve;
mod gui_locate;
mod interactable;
mod native;
mod perm;
mod safety;
pub mod session_lock;
mod set_of_marks;
mod types;
mod vision_bridge;
mod wait_visual;

#[cfg(test)]
mod tests;

pub use ax::{
    DesktopAxQueryByRole, DesktopAxQueryByRoleArgs, DesktopAxQueryFocused,
    DesktopAxQueryFocusedArgs, DesktopAxQueryTree, DesktopAxQueryTreeArgs, DesktopAxSnapshot,
    DesktopAxSnapshotArgs,
};
pub use browser_operator::{
    BrowserOperatorMode, DesktopBrowserOperator, DesktopBrowserOperatorArgs,
};
pub use gui_locate::{DesktopGuiLocate, DesktopGuiLocateArgs};
pub use perm::{DesktopCheckPermissions, DesktopCheckPermissionsArgs};
pub use set_of_marks::{DesktopSom, DesktopSomArgs};
pub use types::{DesktopArgs, DesktopOutput};
pub use vision_bridge::{Augmentation, VisionBridge};

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
    pub(super) escape_started: Arc<crate::sync_primitives::AtomicBool>,
    /// Optional vision bridge: turns a captured screenshot into OCR text + an
    /// optional scene description so text-only models can still act on it.
    /// `None` keeps the screenshot path byte-identical to its legacy behavior.
    pub(super) vision_bridge: Option<Arc<VisionBridge>>,
}

impl DesktopTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            approval_policy: None,
            platform: None,
            escape_started: Arc::new(crate::sync_primitives::AtomicBool::new(false)),
            vision_bridge: None,
        }
    }

    /// Attach a vision bridge so `screenshot` calls that pass `describe: true`
    /// receive an OCR text layer (and, when a multimodal provider is wired, a
    /// scene description) alongside the raw image — enabling text-only models
    /// to perform computer use.
    pub fn with_vision_bridge(mut self, bridge: Arc<VisionBridge>) -> Self {
        self.vision_bridge = Some(bridge);
        self
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
    /// When a policy is set, mutating actions (click, `type_text`, `key_combo`,
    /// `launch_app`, etc.) are checked before execution. Read-only actions
    /// (screenshot, ocr, `window_list`, `cursor_position`, etc.) are always allowed.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Acquire the computer-use session lock for a mutating action.
    ///
    /// The desktop tool is a shared singleton, so the controlling session is
    /// resolved at call time from the turn context — the same source the
    /// approval audit uses ([`audit_identity`]). The returned guard holds the
    /// lock until it drops (end of this `call`), giving cross-session mutual
    /// exclusion over the one physical desktop. Outside a scoped turn (direct
    /// calls, tests) there is no session to arbitrate, so locking is skipped.
    fn acquire_lock(
        &self,
    ) -> std::result::Result<Option<session_lock::SessionLockGuard>, DesktopOutput> {
        let session_id = match crate::tools::turn_context::current_turn_context() {
            Some(turn) => turn.session_key.to_key_string(),
            None => return Ok(None),
        };
        match session_lock::acquire_for_session(&session_id) {
            Ok(guard) => Ok(Some(guard)),
            Err(msg) => Err(DesktopOutput {
                success: false,
                data: None,
                message: Some(msg),
            }),
        }
    }

    /// Check escape abort and start listener on first call.
    fn check_escape(&self) -> std::result::Result<(), DesktopOutput> {
        if let Some(ref platform) = self.platform {
            if let Some(listener) = platform.escape_listener() {
                if !self
                    .escape_started
                    .load(crate::sync_primitives::Ordering::Acquire)
                {
                    // A failed start means the Escape abort hotkey is
                    // unavailable — log it once (the flag is still set below so
                    // we do not retry on every action).
                    if let Err(e) = listener.start() {
                        tracing::warn!(
                            error = %e,
                            "Failed to start desktop escape listener; abort hotkey unavailable"
                        );
                    }
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
        if args.actions.is_empty() {
            return Ok(DesktopOutput {
                success: false,
                data: None,
                message: Some("batch requires non-empty 'actions' array".into()),
            });
        }

        let mut results = Vec::new();
        for (i, batch_action) in args.actions.iter().enumerate() {
            // Check escape between actions
            if let Err(out) = self.check_escape() {
                results.push(serde_json::json!({
                    "index": i,
                    "aborted": true,
                    "message": out.message,
                }));
                break;
            }

            let mut sub_args: DesktopArgs = batch_action.into();

            // Inherit batch-level coord_space when the sub-action does not
            // override. This makes `{action:"batch", coord_space:"normalized",
            // actions:[{action:"click",x:500,y:500}]}` work as the obvious
            // reader would expect.
            if sub_args.coord_space.is_none() {
                sub_args.coord_space = args.coord_space.clone();
            }
            if sub_args.coord_factors.is_none() {
                sub_args.coord_factors = args.coord_factors;
            }

            // Prevent nested batch. The type system already prevents an
            // inner DesktopBatchAction from carrying its own actions list,
            // but we still reject `action == "batch"` here to surface a
            // clearer error than the empty-actions message that would
            // otherwise come back from a recursive call.
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
    ///
    /// The [`ActionRequest`] is stamped with the issuing agent's id and a
    /// human-readable audit context resolved from the agent-loop call context
    /// (see [`audit_identity`]), so `ApprovalPolicy::record` produces an
    /// accountable audit trail rather than blank entries.
    async fn check_approval(
        &self,
        action_type: ActionType,
        action: &str,
        target: &str,
    ) -> Option<DesktopOutput> {
        let policy = self.approval_policy.as_ref()?;

        let (agent_id, context) = crate::approval::audit_identity("desktop", action, target);
        let request = ActionRequest {
            action_type,
            target: target.to_string(),
            agent_id,
            context,
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

/// Actions that must hold the cross-session desktop lock + escape pre-flight,
/// even when they are approval-exempt.
///
/// `focus_window` mutates global window focus / Z-order and `screen_record`
/// holds the capture device for seconds, so a second session must not run them
/// while another holds the exclusive computer-use lock. Locking is deliberately
/// decoupled from approval ([`classify_approval`]) so these stay prompt-free.
fn requires_lock(args: &DesktopArgs) -> bool {
    classify_approval(args).is_some()
        || matches!(args.action.as_str(), "focus_window" | "screen_record")
}

/// Classify a desktop action for approval gating.
///
/// Returns `None` for read-only actions that skip approval,
/// or `Some((ActionType, target))` for actions that require approval.
fn classify_approval(args: &DesktopArgs) -> Option<(ActionType, String)> {
    match args.action.as_str() {
        "screenshot" | "ocr" | "window_list" | "cursor_position" | "clipboard_read"
        | "screen_record" | "focus_window" | "display_list" | "wait_visual" => None,

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
        // Hold/release a key or chord (UI-TARS press/release). Same approval
        // identity as key_combo — without this arm it fell through to the
        // `_ => "unknown"` branch and was audited as an unknown action.
        "key_button" => Some((
            ActionType::DesktopKeyCombo,
            args.keys.as_ref().map(|k| k.join("+")).unwrap_or_default(),
        )),

        "launch_app" | "quit_app" | "restart_app" => Some((
            ActionType::DesktopLaunchApp,
            args.bundle_id.clone().unwrap_or_default(),
        )),

        "move_window" => Some((
            ActionType::DesktopClick,
            format!(
                "move_window(#{},{},{})",
                args.window_id.unwrap_or(0),
                args.x.unwrap_or(0.0),
                args.y.unwrap_or(0.0)
            ),
        )),
        "resize_window" => Some((
            ActionType::DesktopClick,
            format!(
                "resize_window(#{},{}x{})",
                args.window_id.unwrap_or(0),
                args.width.unwrap_or(0),
                args.height.unwrap_or(0)
            ),
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

/// Unconditional content-level safety hard-block.
///
/// Runs *below* the approval policy: even an approval policy configured to
/// auto-allow must not let a handful of catastrophic input payloads (remote
/// code execution, root deletion, session log-out) reach a live desktop.
/// Returns `Some(output)` when the action must be refused outright.
///
/// `batch` is intentionally not inspected here — each sub-action is
/// dispatched through `AlephTool::call`, which re-runs this check.
fn check_hard_block(args: &DesktopArgs) -> Option<DesktopOutput> {
    let reason: Option<String> = match args.action.as_str() {
        // `clipboard_write` stages text that the model can then inject with a
        // manual `key_combo` Cmd/Ctrl+V — an equivalent capability to `paste`
        // that would otherwise sidestep this content gate. Hold it to the same
        // bar so the staged-then-pasted path cannot smuggle a catastrophic
        // payload onto a live desktop.
        "type_text" | "paste" | "clipboard_write" => args
            .text
            .as_deref()
            .and_then(|t| safety::check_typed_text(t).err()),
        // `key_button` presses its `keys` as a chord (full press+release),
        // reaching the same destructive capability as `key_combo` (e.g.
        // Cmd+Shift+Q logout). The UI-TARS `press`/`release`/`key_down`/
        // `key_up` verbs route here too, so it must pass the same content gate.
        "key_combo" | "key_button" => args
            .keys
            .as_deref()
            .and_then(|k| safety::check_key_combo(k).err()),
        _ => None,
    };
    reason.map(|message| DesktopOutput {
        success: false,
        data: None,
        message: Some(message),
    })
}

/// Honor UI-TARS loop-control verbs (`finished`, `call_user`).
///
/// These are not desktop mutations — a UI-TARS-finetuned model emits them to
/// signal task completion or to request user help. The harness's Think→Act
/// loop owns flow control (R10), so the tool simply acknowledges the signal
/// with a benign success result, surfacing any `content` the model attached
/// (parsed into `text` by [`action_script`]). Returns `None` for every other
/// action so normal dispatch proceeds unchanged.
fn loop_control_output(args: &DesktopArgs) -> Option<DesktopOutput> {
    match args.action.as_str() {
        "finished" => Some(DesktopOutput {
            success: true,
            data: Some(serde_json::json!({ "finished": true })),
            message: Some(
                args.text
                    .clone()
                    .unwrap_or_else(|| "Task marked finished.".into()),
            ),
        }),
        "call_user" => Some(DesktopOutput {
            success: true,
            data: Some(serde_json::json!({ "call_user": true })),
            message: Some(
                args.text
                    .clone()
                    .unwrap_or_else(|| "Handing control back to the user.".into()),
            ),
        }),
        _ => None,
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

For reliable clicking, call `desktop_ax_snapshot` first: it returns the app's
interactable elements with ready-to-use `center` [x, y] coordinates — far more
accurate than estimating pixels from a screenshot. Use a screenshot only when
you need to *see* visual state the accessibility tree cannot describe.

Actions:
- screenshot: Capture screen as base64. Optional region: {x,y,width,height}. Defaults to PNG; pass format/quality/max_width to re-encode (downscaling above 1920 hurts text legibility — keep max_width at 1920+ when you need to read text on screen). Oversized captures are auto-compressed to JPEG to stay within the result budget. Pass `describe:true` if you cannot see images — the result then also carries an `ocr_text` layer (and a scene `description` when a vision model is configured) so a text-only model can still read the screen and act.
- ocr: Extract text from screen with bounding boxes. Optional image_base64.
- click: Click at (x, y). Optional button (left/right/middle).
- double_click: Double-click at (x, y). Optional button.
- drag: Drag from (start_x, start_y) to (end_x, end_y). Optional duration_ms for animation.
- hover: Move mouse to (x, y) without clicking.
- cursor_position: Get current mouse cursor position.
- mouse_button: Press/release mouse at (x, y). Requires press_action (press/release/click).
- type_text: Type text at current cursor position.
- key_combo: Press key combination, e.g. keys=["cmd","c"].
- key_button: Hold or release a key/chord without auto-releasing — keys=["cmd"] plus press_action="press" (hold down) then later press_action="release". Distinct from key_combo, which presses and releases atomically. Useful for drag-with-modifier or chorded shortcuts.
- scroll: Scroll via delta_x/delta_y.
- launch_app: Launch app by bundle_id.
- quit_app: Close app by bundle_id.
- restart_app: Restart app by bundle_id (quit then relaunch).
- window_list: List open windows.
- focus_window: Bring window to front by window_id.
- move_window: Move a window's top-left corner to (x, y) by window_id.
- resize_window: Resize a window to width × height pixels by window_id.
- clipboard_read: Read clipboard text.
- clipboard_write: Write text to clipboard.
- screen_record: Record screen as MP4. Optional duration/fps/with_audio and region {x,y,width,height} (defaults to the full primary display). region honors coord_space:"normalized" like screenshot.
- display_list: List all connected displays with resolution and scale info.
- batch: Execute multiple actions sequentially. Requires actions array.
- paste: Paste text via clipboard (Cmd+V). Better for multiline text than type_text.
- wait_visual: Wait until the screen settles. Polls screenshots and returns when two consecutive captures match, or after `timeout_ms` (default 5000, max 60000). Use after navigation or clicks that trigger animation. Returns `{stable: bool, polls, elapsed_ms}` — `stable=false` means timeout, not failure.

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
{"action":"screen_record","duration":5.0,"fps":30,"region":{"x":0,"y":0,"width":1280,"height":720}}
{"action":"display_list"}
{"action":"move_window","window_id":1234,"x":100,"y":80}
{"action":"resize_window","window_id":1234,"width":1280,"height":800}
{"action":"batch","actions":[{"action":"click","x":100,"y":200},{"action":"type_text","text":"hello"}]}
{"action":"paste","text":"line1\nline2\nline3"}
{"action":"wait_visual","timeout_ms":3000}
{"action":"wait_visual","timeout_ms":8000,"region":{"x":0,"y":0,"width":1280,"height":800}}
{"action":"screenshot","format":"jpeg","quality":0.9,"max_width":1920}
{"action":"screenshot","describe":true}

Coordinate space — by default, `x` / `y` / `start_x` / `end_x` / `region` are pixels (top-left origin). To use UI-TARS-style normalized [0, 1000] × [0, 1000] coordinates that scale to any display, set `coord_space:"normalized"` (and optionally `coord_factors:[w,h]` to override the 1000×1000 default). The runtime rescales against the primary display (or `display_id`) before dispatch.

IMPORTANT — a full-screen `screenshot` is usually downscaled to fit the result budget, so the image you see is smaller than the real display. The screenshot result carries a `coordinate_space` block with the served `image_width`/`image_height`; to act on a point you saw at image pixel (px, py), send `coord_space:"normalized"` with `coord_factors:[image_width, image_height]` and `x=px, y=py`. Replaying raw image pixels as `pixel`-space coords will miss on Retina/downscaled captures.

{"action":"click","coord_space":"normalized","x":500,"y":500}
{"action":"batch","coord_space":"normalized","actions":[{"action":"click","x":300,"y":400},{"action":"type_text","text":"hi"}]}

Pythonic action script — UI-TARS-finetuned models can emit `script` containing one or more `Action: ...` lines instead of JSON. Supported verbs: click, left_double, right_single, middle_click, drag, hover, type, hotkey, press, release, scroll, wait, finished, call_user. A trailing `\n` in `type(content='…\n')` types the text then presses Enter (submit). `press(key='ctrl')` / `release(key='ctrl')` hold a key (or chord like `ctrl shift`) down and release it later — distinct from `hotkey`, which presses and releases atomically. Box formats `(x,y)`, `[x1,y1,x2,y2]`, `<point>x y</point>`, `<bbox>x1 y1 x2 y2</bbox>` all parse. Inherits `coord_space` / `coord_factors` from the same call.

{"action":"script","coord_space":"normalized","script":"Thought: open menu\nAction: click(start_box='(500,30)')"}
{"action":"script","coord_space":"normalized","script":"type(content='hello\\n'); wait(); click(start_box='[100,200,300,400]')"}"#;

    type Args = DesktopArgs;
    type Output = DesktopOutput;

    async fn call(&self, mut args: Self::Args) -> Result<Self::Output> {
        // 0. UI-TARS script expansion. When the caller (or model) sends a
        //    Pythonic action script, transparently rewrite into a batch so
        //    the rest of the pipeline (approval, safety, dispatch) handles
        //    each sub-action through the same path as JSON tool-calls.
        if args.script.is_some() {
            let script = args.script.take().unwrap_or_default();
            match action_script::parse_action_script(&script) {
                Ok(actions) => {
                    args.actions = actions;
                    args.action = "batch".to_string();
                }
                Err(e) => {
                    return Ok(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("action script parse error: {e}")),
                    });
                }
            }
        }

        // 1. Unconditional safety hard-block (sits below the approval policy).
        if let Some(out) = check_hard_block(&args) {
            return Ok(out);
        }

        // 1.5 Loop-control verbs (UI-TARS `finished` / `call_user`). The model
        //     emits these to signal task completion or to hand control back to
        //     the user; they touch no desktop state. Honor them with a benign
        //     terminal result *before* approval, session lock, and platform
        //     dispatch — otherwise they would be misclassified as mutating and
        //     ultimately reported as "unsupported action", which in a batch
        //     chain would wrongly fail an otherwise-successful task. R10: the
        //     host honors the signal, it does not drive a loop.
        if let Some(out) = loop_control_output(&args) {
            return Ok(out);
        }

        // 2. Normalize coordinate space for leaf actions. Batches defer per
        //    sub-action so each one inherits the batch-level `coord_space`
        //    via `execute_batch` and rescales against its own display.
        if args.action != "batch" {
            if let Some(ref platform) = self.platform {
                coord_resolve::maybe_normalize(&mut args, platform).await?;
            }
        }

        // Approval gating is for mutating actions; locking/escape covers a
        // slightly wider set (focus_window/screen_record mutate shared desktop
        // state but stay approval-exempt) — see `requires_lock`.
        let needs_lock = requires_lock(&args);

        // 3. Approval check
        if let Some((action_type, target)) = classify_approval(&args) {
            if let Some(out) = self
                .check_approval(action_type, &args.action, &target)
                .await
            {
                return Ok(out);
            }
        }

        // 4. Session lock (lock-requiring actions only). The guard is held for the
        //    rest of this call (and, for a batch, across all its sub-actions
        //    via re-entrant acquisition) and released on drop, so the desktop
        //    is freed for other sessions once the call returns.
        let _session_guard = if needs_lock {
            match self.acquire_lock() {
                Ok(guard) => guard,
                Err(out) => return Ok(out),
            }
        } else {
            None
        };

        // 5. Escape abort check (lock-requiring actions only)
        if needs_lock {
            if let Err(out) = self.check_escape() {
                return Ok(out);
            }
        }

        // 6. Handle batch at this level (needs recursive self.call())
        if args.action == "batch" {
            return self.execute_batch(&args).await;
        }

        // 7. Execute via platform
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
