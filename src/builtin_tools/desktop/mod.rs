//! Desktop tool — sees and controls the desktop via the platform-native
//! `desktop/*` capability layer.

mod action_script;
mod ax;
mod coord_resolve;
mod focus_gate;
mod gui_locate;
mod held_inputs;
mod interactable;
mod native;
mod observe;
mod perm;
mod recovery;
mod safety;
pub mod session_lock;
mod set_of_marks;
mod types;
mod verify_state;
mod vision_bridge;
mod wait_visual;

#[cfg(test)]
mod tests;

pub use ax::{
    DesktopAxQueryByRole, DesktopAxQueryByRoleArgs, DesktopAxQueryFocused,
    DesktopAxQueryFocusedArgs, DesktopAxQueryTree, DesktopAxQueryTreeArgs, DesktopAxSnapshot,
    DesktopAxSnapshotArgs,
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
    /// Run id of the last turn seen by [`DesktopTool::check_escape`]. The tool
    /// is a shared singleton, so the abort scope cannot be a struct field fixed
    /// at construction — it is tracked here and compared per call.
    pub(super) last_run_id: Arc<crate::sync_primitives::Mutex<String>>,
    /// Optional vision bridge: turns a captured screenshot into OCR text + an
    /// optional scene description so text-only models can still act on it.
    /// `None` keeps the screenshot path byte-identical to its legacy behavior.
    pub(super) vision_bridge: Option<Arc<VisionBridge>>,
    /// `[desktop] allow_global_pointer` — permit coordinate actions that name no
    /// target process, and therefore run on the global input tap: they drag the
    /// user's physical cursor and need the target app frontmost.
    ///
    /// Default `false` (fail closed). Only consulted on a platform that *has* a
    /// background rail (see [`native`]); elsewhere it is inert.
    pub(super) allow_global_pointer: bool,
    pub(super) clipboard_enabled: bool,
}

impl DesktopTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            approval_policy: None,
            platform: None,
            escape_started: Arc::new(crate::sync_primitives::AtomicBool::new(false)),
            last_run_id: Arc::new(crate::sync_primitives::Mutex::new(String::new())),
            vision_bridge: None,
            allow_global_pointer: false,
            clipboard_enabled: true,
        }
    }

    /// Permit the intrusive global input rail for actions that name no target
    /// process (`[desktop] allow_global_pointer`).
    ///
    /// Left at its default, an untargeted click on a platform that could have
    /// delivered it in the background is refused instead of moving the user's
    /// cursor — see [`native`] for why that refusal is fail-closed rather than a
    /// silent fallback.
    #[must_use]
    pub fn with_allow_global_pointer(mut self, allow: bool) -> Self {
        self.allow_global_pointer = allow;
        self
    }

    #[must_use]
    pub fn with_clipboard_enabled(mut self, enabled: bool) -> Self {
        self.clipboard_enabled = enabled;
        self
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

    /// True when this call is the first desktop action of a run we have not
    /// seen before — the boundary at which the Escape abort flag is reset.
    ///
    /// Outside a gateway run (`run_id` empty — cron, internal, tests) there is
    /// no run identity to compare against, so every call would look like a
    /// fresh boundary and would clear the flag the user had just set, disarming
    /// Escape for exactly the unattended runs where an emergency stop matters
    /// most. Those callers therefore keep today's sticky flag; because the
    /// platform listener's flag is process-wide, the next gateway run's
    /// boundary clears it for them too.
    fn run_boundary_crossed(&self) -> bool {
        let run_id = crate::tools::turn_context::current_turn_context()
            .map(|turn| turn.run_id)
            .unwrap_or_default();
        if run_id.is_empty() {
            return false;
        }
        let mut last = self.last_run_id.lock().unwrap_or_else(|e| e.into_inner());
        if *last == run_id {
            return false;
        }
        *last = run_id;
        true
    }

    /// Check escape abort, starting the listener on first call and resetting the
    /// abort flag at each run boundary.
    ///
    /// The platform abort flag is process-wide and set by a *global* key
    /// monitor: any Escape press, in any app, for any reason, raises it. Without
    /// a boundary reset it would never fall again, and a single Escape typed
    /// hours ago to dismiss a dialog would refuse every desktop action of every
    /// session until the server restarts. Escape aborts the run it was pressed
    /// during — a new run resets the flag; within one run, once aborted, stays
    /// aborted.
    async fn check_escape(&self) -> std::result::Result<(), DesktopOutput> {
        let Some(platform) = self.platform.as_ref() else {
            return Ok(());
        };
        let Some(listener) = platform.escape_listener() else {
            return Ok(());
        };

        if !self
            .escape_started
            .load(crate::sync_primitives::Ordering::Acquire)
        {
            // A failed start means the Escape abort hotkey is unavailable — log
            // it once (the flag is still set below so we do not retry on every
            // action).
            if let Err(e) = listener.start() {
                tracing::warn!(
                    error = %e,
                    "Failed to start desktop escape listener; abort hotkey unavailable"
                );
            }
            self.escape_started
                .store(true, crate::sync_primitives::Ordering::Release);
        }

        if self.run_boundary_crossed() {
            listener.reset();
            // A previous run may have died holding a modifier or mid-drag; the
            // new run must not inherit its stuck inputs.
            if let Some(screen) = platform.screen() {
                held_inputs::release_all(&held_inputs::current_session_id(), screen).await;
            }
        }

        if listener.is_aborted() {
            // Escape is the user's emergency stop on the one physical desktop:
            // hand back every held key/button, not just this session's.
            if let Some(screen) = platform.screen() {
                held_inputs::release_all_sessions(screen).await;
            }
            return Err(DesktopOutput {
                success: false,
                data: None,
                message: Some("Computer use aborted by user (Escape pressed)".into()),
            });
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
            if let Err(out) = self.check_escape().await {
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
            if sub_args.observe.is_none() {
                sub_args.observe = args.observe.clone();
            }

            // Same for the target process: a batch names the app it is driving
            // once, at the top. Without this, every sub-action of
            // `{action:"batch", app:"Notes", actions:[…]}` would look untargeted
            // and be refused by the pointer policy — the batch would be the one
            // shape of call that could not use the background rail.
            if sub_args.app.is_none() {
                sub_args.app = args.app.clone();
            }
            if sub_args.pid.is_none() {
                sub_args.pid = args.pid;
            }
            if sub_args.window_id.is_none() {
                sub_args.window_id = args.window_id;
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
            display_target: String::new(),
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

    /// Hard-refuse mutating actions aimed at a credential vault, and refuse
    /// launching/quitting/focusing one. Fail-open: if the app cannot be
    /// determined, proceed — this guard is defense-in-depth on top of approval +
    /// content hard-blocks, not the only line.
    ///
    /// *Which* app it guards depends on where the event goes. A global event
    /// lands wherever focus is, so the frontmost app is the one at risk. A
    /// targeted event is posted into one process and never touches the frontmost
    /// app — guarding that app would both miss the real target (1Password driven
    /// in the background while Safari is frontmost) and refuse harmless work
    /// (typing into Notes while the *user* has 1Password open). So the guard
    /// follows the event.
    async fn check_blocked_app(&self, args: &DesktopArgs) -> Option<DesktopOutput> {
        let platform = self.platform.as_ref()?;

        // Target guard: launching / quitting / restarting a blocked app.
        if matches!(
            args.action.as_str(),
            "launch_app" | "quit_app" | "restart_app"
        ) {
            if let Some(bid) = args.bundle_id.as_deref() {
                if let Some(reason) = safety::blocked_app_reason(bid, bid) {
                    return Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(reason),
                    });
                }
            }
            return None; // launch of a non-blocked app needs no frontmost check
        }

        let system = platform.system()?;
        let apps = system.list_running_apps().await.ok()?;

        // The app the event will actually reach. `resolve_rail` costs nothing
        // when the caller named no target (it never queries), and its Err is
        // ignored here on purpose: the same call re-runs at dispatch and refuses
        // there with the full message — this guard only decides *whose* identity
        // to check.
        let rail = match platform.screen() {
            Some(screen) => {
                native::resolve_rail(self.allow_global_pointer, platform, screen, args).await
            }
            // No screen at all: nothing is delivered anywhere, but the frontmost
            // guard is what this platform had before, so keep it.
            None => Ok(native::Rail::Global),
        };
        let target = match rail {
            Ok(native::Rail::Targeted(pid)) => {
                let pid = u64::try_from(pid).ok()?;
                apps.iter().find(|a| a.pid == Some(pid))?
            }
            Ok(native::Rail::Global) | Err(_) => apps.iter().find(|a| a.is_active)?,
        };

        safety::blocked_app_reason(&target.name, &target.bundle_id).map(|reason| DesktopOutput {
            success: false,
            data: None,
            message: Some(reason),
        })
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
        | "screen_record" | "focus_window" | "display_list" | "wait_visual" | "verify_state" => {
            None
        }

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

        "set_value" => Some((
            ActionType::DesktopType,
            args.text.clone().unwrap_or_default(),
        )),
        "ax_action" => Some((
            ActionType::DesktopClick,
            format!(
                "ax_action({})",
                args.ax_action_name.as_deref().unwrap_or("?")
            ),
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
        "type_text" | "paste" | "clipboard_write" | "set_value" => args
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

ALWAYS SAY WHICH APP YOU ARE DRIVING. Pass `app` (name or bundle id, e.g.
"Notes" / "com.apple.Notes"), or `pid`, or `window_id` on every click, drag,
hover, scroll, type_text, key_combo, key_button, mouse_button and paste. The event is then
delivered straight into that process: the user's physical mouse pointer never
moves, and the app does not have to be frontmost — you work in the background
while the user keeps using their machine. This is the preferred path.

Name no app and the event goes to the global input tap instead: it PHYSICALLY
DRAGS THE USER'S CURSOR across the screen and only lands wherever focus already
is. That path is refused by default (the operator can allow it with
`[desktop] allow_global_pointer = true`). Every result says which rail ran, as
`delivery: "targeted" | "global"`.

For reliable clicking, call `desktop_ax_snapshot` first: it returns the app's
interactable elements with ready-to-use `center` [x, y] coordinates — far more
accurate than estimating pixels from a screenshot — and its `app_pid` is the
`pid` to pass back here. `set_value` / `ax_action` are better still for text
fields and buttons: they address the element directly, verify the write, and are
likewise cursor-free. Use a screenshot only when you need to *see* visual state
the accessibility tree cannot describe.

Driving a web page? Reach for the `browser_*` tools first — they read the DOM, so
they are faster and exact. Come back to this tool for what the DOM cannot reach:
canvas/WebGL widgets, shadow-DOM and cross-origin frames, a page that actively
defeats DOM automation, and anything outside the page itself (download bars, OS
file pickers, permission sheets). `desktop_gui_locate` grounds a visible label to
coordinates when no DOM ref exists. After any click, type or scroll, wait for the
UI to settle (`wait_visual`, or the browser's own wait) before you read state.

Actions:
- screenshot: Capture screen as base64. Optional region: {x,y,width,height}. Pass `window_id` (from window_list) to capture ONE window instead of the display — it works even when the window is partly covered or not frontmost, and nothing else on screen leaks into the shot. A window capture's result carries a `coordinate_space` block: address points in it with `coord_space:"window"` (see Coordinate space below). Defaults to PNG; pass format/quality/max_width to re-encode (downscaling above 1920 hurts text legibility — keep max_width at 1920+ when you need to read text on screen). Oversized captures are auto-compressed to JPEG to stay within the result budget. Pass `describe:true` if you cannot see images — the result then also carries an `ocr_text` layer (and a scene `description` when a vision model is configured) so a text-only model can still read the screen and act.
- ocr: Extract text from screen with bounding boxes. Optional image_base64.
- click: Click at (x, y). Optional button (left/right/middle).
- double_click: Double-click at (x, y). Optional button.
- drag: Drag from (start_x, start_y) to (end_x, end_y). Optional duration_ms for animation.
- hover: Move mouse to (x, y) without clicking.
- cursor_position: Get current mouse cursor position.
- mouse_button: Press/release mouse at (x, y). Requires press_action (press/release/click).
- type_text: Type text at the current keyboard focus. Blind by nature — the keystrokes go wherever focus actually is — so it is pre-flighted against the accessibility layer: it refuses when nothing is focused, or when the focused element reports that it takes no typed value, and tells you what to do instead (click the input, or use set_value). Typing into a secure/password field is always refused. If the app's accessibility tree lies (canvas, terminal, some Electron shells), pass force:true to type anyway — force does not lift the password refusal. Where no accessibility layer is available the pre-flight is skipped entirely.
- key_combo: Press key combination, e.g. keys=["cmd","c"].
- key_button: Hold or release a key/chord without auto-releasing — keys=["cmd"] plus press_action="press" (hold down) then later press_action="release". Distinct from key_combo, which presses and releases atomically. Useful for drag-with-modifier or chorded shortcuts. Send the matching release on the same target you pressed against: a key held in one app is only let up in that app.
- scroll: Scroll by delta_x/delta_y PIXELS (negative = up/left), e.g. delta_y:-300 scrolls up about 300px. Pixels are quantized to whole mouse-wheel clicks (~100px each), so a delta under ~100px still moves one click and the result says how far it actually went. Pass x/y to say WHICH view to scroll: on a background (app/pid/window_id) scroll the point is required, since the cursor never moves; on a plain scroll the point is optional and, when given, the cursor is moved onto it first so the wheel lands there — omit it to scroll at the current cursor. Any point inside the target works (e.g. an element `center`). The result echoes where it scrolled under `at`.
- launch_app: Launch app by bundle_id.
- quit_app: Close app by bundle_id.
- restart_app: Restart app by bundle_id (quit then relaunch).
- window_list: List open windows — each entry carries `id`, `title`, `owner`, `pid` (pass it back to drive that app in the background) and, where the platform reports them, `bounds` {x,y,width,height} in screen points, `layer` (0 = a normal app window) and `on_screen`.
- focus_window: Bring window to front by window_id.
- move_window: Move a window's top-left corner to (x, y) by window_id.
- resize_window: Resize a window to width × height pixels by window_id.
- clipboard_read: Read clipboard text.
- clipboard_write: Write text to clipboard.
- screen_record: Record screen as MP4. Optional duration/fps/with_audio and region {x,y,width,height} (defaults to the full primary display). region honors coord_space:"normalized" like screenshot.
- display_list: List all connected displays with resolution and scale info.
- batch: Execute multiple actions sequentially. Requires actions array. Sub-actions inherit the batch-level `app` / `pid` / `window_id` (name the app you are driving once, at the top) and `coord_space` / `coord_factors` / `observe`, unless they set their own.
- paste: Paste text via clipboard (Cmd+V). Better for multiline text than type_text. It goes through the user's clipboard: plain text is put back afterwards, but an image or file on the clipboard cannot be (the result says so) — use type_text when the clipboard must be preserved.
- wait_visual: Wait until the screen settles. Polls screenshots and returns when two consecutive captures match, or after `timeout_ms` (default 5000, max 60000). Use after navigation or clicks that trigger animation. Returns `{stable: bool, polls, elapsed_ms}` — `stable=false` means timeout, not failure.
- verify_state: Confirm a postcondition over the target app's accessibility tree, instead of eyeballing a screenshot. Pass `expect`: a list of 1–8 predicates that are ANDed and polled until they hold stably or `timeout_ms` (default 5000, max 10000; 0 = sample once) elapses. Each predicate is `{assert, role?, title?, title_contains?, value?}` where `assert` is one of `exists` (an element matching the selector is present), `focused` (the app's focused element matches), `value_equals` / `value_contains` (a single matched element's value). `stable_samples` (default 2, max 5) is how many consecutive satisfied samples count as stable. Scope with app/pid/window_id. Returns `{status: "satisfied"|"unsatisfied"|"unknown", stable, elapsed_ms, samples, predicates[]}`. `unknown` means the observation could NOT prove either answer (element not found within the node budget — absence is never provable — an ambiguous match, or a secure value) and is NEVER a pass. There is deliberately no "absent"/"not focused" assertion for the same reason.
- set_value: Set a text field's value directly via the accessibility API and VERIFY the write by reading it back — the reliable way to fill forms (multiline, non-ASCII, replacing existing content). Locate the element with role ("AXTextField") and/or element_title, optionally x/y as a nearest-center hint (honors coord_space). Requires text. Result carries verification.state = "verified" | "unverified". Prefer this over click + type_text; type_text is a blind synthetic fallback.
- ax_action: Trigger a native accessibility action (ax_action_name, e.g. "AXPress", "AXShowMenu") on an element located the same way. More reliable than a synthetic click for buttons/menus when the app exposes AX actions. Do not guess the name: desktop_ax_snapshot / desktop_som / desktop_gui_locate return each element's own `actions` list — pass one of those verbatim. An element reporting `enabled:false` is greyed out; acting on it does nothing, so fix the precondition instead. Available on macOS (AX), Windows (UI Automation: AXPress→Invoke/Toggle/Select, AXShowMenu→Expand) and Linux (AT-SPI2: pass the element's own action name, e.g. "click"; the AX* spellings resolve too). Where no accessibility layer is available, fall back to click.

Held inputs — a `press` on key_button / mouse_button stays physically down on the user's keyboard and mouse until you release it, so always send the matching `release` for every key and button you pressed before you finish a step.

Coordinate space — by default, `x` / `y` / `start_x` / `end_x` / `region` are pixels (top-left origin). Two other spaces exist:
- `coord_space:"normalized"` — UI-TARS-style [0, 1000] × [0, 1000] coordinates that scale to any display (override the 1000×1000 default with `coord_factors:[w,h]`). Resolved against the primary display (or `display_id`).
- `coord_space:"window"` — pixels of a WINDOW capture. Requires `window_id`. Their origin is the window's top-left, not the screen's, so the runtime maps them back through that window's frame. Use this for every point you read off a `screenshot {window_id}` — resolving them against the display instead would miss by the window's offset.

IMPORTANT — a `screenshot` is usually downscaled to fit the result budget, so the image you see is smaller than the real capture. Its result carries a `coordinate_space` block with the served `image_width`/`image_height`; to act on a point you saw at image pixel (px, py), pass those as `coord_factors:[image_width, image_height]` with `x=px, y=py` and the matching `coord_space` ("normalized" for a display capture, "window" for a window capture). Replaying raw image pixels as `pixel`-space coords will miss on Retina/downscaled captures.

{"action":"click","coord_space":"normalized","x":500,"y":500}
{"action":"batch","coord_space":"normalized","actions":[{"action":"click","x":300,"y":400},{"action":"type_text","text":"hi"}]}

Drive one app in the background (nothing else on the user's screen is disturbed):
{"action":"screenshot","window_id":8412}
{"action":"click","window_id":8412,"coord_space":"window","coord_factors":[600,400],"x":310,"y":128}
{"action":"type_text","app":"Notes","text":"meeting notes\n"}
{"action":"scroll","pid":733,"x":700,"y":460,"delta_y":-300}

Act→observe in one call — mutating actions accept `observe:"state"` (result gains `post_state`: frontmost app + focused element after a 300ms settle) or `observe:"screenshot"` (additionally a fresh bounded screenshot as `post_screenshot`). Use it on the last action of a step instead of a separate screenshot round-trip. In a batch, sub-actions inherit the batch-level `observe`.

{"action":"click","x":500,"y":300,"observe":"state"}

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

        if !self.clipboard_enabled
            && matches!(
                args.action.as_str(),
                "clipboard_read" | "clipboard_write" | "paste"
            )
        {
            return Ok(DesktopOutput {
                success: false,
                data: None,
                message: Some(
                    "Clipboard actions (clipboard_read / clipboard_write / paste) are \
                     disabled by configuration (UnifiedToolsConfig.clipboard.enabled = \
                     false). Set [unified_tools.native.clipboard] enabled = true (or omit \
                     the section) to allow them."
                        .to_string(),
                ),
            });
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

        // 1.6 Blocked-app guard: never drive a credential vault. Leaf mutating
        //     actions only — batch sub-actions re-enter call() and get checked
        //     individually against the then-current frontmost app.
        if args.action != "batch" && classify_approval(&args).is_some() {
            if let Some(out) = self.check_blocked_app(&args).await {
                return Ok(out);
            }
        }

        // 2. Normalize coordinate space for leaf actions. Batches defer per
        //    sub-action so each one inherits the batch-level `coord_space`
        //    via `execute_batch` and rescales against its own display.
        //
        //    A window-space point that cannot be mapped back (no window_id, a
        //    stale id, a platform with no window bounds) refuses here rather
        //    than dispatching raw pixels, which would click at an arbitrary
        //    place on the screen.
        if args.action != "batch" {
            if let Some(ref platform) = self.platform {
                if let Some(refusal) = coord_resolve::maybe_normalize(&mut args, platform).await? {
                    return Ok(refusal);
                }
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
            if let Err(out) = self.check_escape().await {
                return Ok(out);
            }
        }

        // 6. Handle batch at this level (needs recursive self.call())
        if args.action == "batch" {
            return self.execute_batch(&args).await;
        }

        // 7. Execute via platform
        if let Some(ref platform) = self.platform {
            if let Some(mut output) = self.call_via_platform(platform, &args).await? {
                // 7.5 act→observe fusion: on success, a mutating leaf action may
                //     carry its own post-state so the model saves a round-trip.
                //     Never turns a succeeded action into a failure —
                //     observation is strictly additive to `data`.
                let wants_observe = matches!(args.observe.as_deref(), Some("state" | "screenshot"));
                if wants_observe && output.success && classify_approval(&args).is_some() {
                    let post = observe::gather_post_state(platform).await;
                    let mut data = output.data.take().unwrap_or_else(|| serde_json::json!({}));
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert("post_state".into(), post);
                    }
                    output.data = Some(data);

                    if args.observe.as_deref() == Some("screenshot") {
                        let mut shot_args =
                            DesktopArgs::from(&types::DesktopBatchAction::empty("screenshot"));
                        shot_args.max_width = Some(1568);
                        if let Ok(Some(shot)) = self.call_via_platform(platform, &shot_args).await {
                            if let (Some(obj), Some(shot_data)) = (
                                output.data.as_mut().and_then(|d| d.as_object_mut()),
                                shot.data,
                            ) {
                                obj.insert("post_screenshot".into(), shot_data);
                            }
                        }
                    }
                }
                return Ok(output);
            }
        }

        if self.platform.is_none() {
            return Ok(self.no_capability_output());
        }

        Ok(self.unsupported_action_output(&args))
    }
}

/// Run-scoping of the Escape abort flag ([`DesktopTool::run_boundary_crossed`]).
/// The platform flag itself needs a live desktop, so the boundary logic that
/// decides *when* it is reset is tested here on its own.
#[cfg(test)]
mod escape_scope_tests {
    use super::DesktopTool;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn turn(run_id: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("desktop-escape-scope"),
            run_id: run_id.to_string(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        }
    }

    #[tokio::test]
    async fn boundary_crosses_once_per_run() {
        let tool = DesktopTool::new();

        let first = TURN_CONTEXT
            .scope(turn("run-1"), async { tool.run_boundary_crossed() })
            .await;
        assert!(
            first,
            "the first action of a new run resets the abort scope"
        );

        let same_run = TURN_CONTEXT
            .scope(turn("run-1"), async { tool.run_boundary_crossed() })
            .await;
        assert!(
            !same_run,
            "later actions of the same run must not clear the flag — within one \
             run, once aborted stays aborted"
        );

        let next_run = TURN_CONTEXT
            .scope(turn("run-2"), async { tool.run_boundary_crossed() })
            .await;
        assert!(next_run, "the next run gets a fresh abort scope");
    }

    #[tokio::test]
    async fn without_a_run_identity_the_flag_stays_sticky() {
        let tool = DesktopTool::new();

        assert!(
            !tool.run_boundary_crossed(),
            "outside a turn there is no run to scope the abort to"
        );

        let cron = TURN_CONTEXT
            .scope(turn(""), async { tool.run_boundary_crossed() })
            .await;
        assert!(
            !cron,
            "an empty run_id (cron/internal) must not reset on every call — that \
             would disarm Escape for unattended runs"
        );
    }
}
