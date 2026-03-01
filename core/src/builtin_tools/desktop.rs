//! Desktop Bridge tool — sees and controls the desktop via the Desktop Bridge.
//!
//! Requires the Aleph Desktop Bridge to be connected. When the bridge is absent,
//! all operations return a friendly message instead of an error.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::desktop::types::{CanvasPosition, MouseButton, ScreenRegion};
use crate::desktop::{DesktopBridgeClient, DesktopRequest};
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for the desktop tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DesktopArgs {
    /// The desktop operation to perform.
    ///
    /// Perception: "screenshot", "ocr", "ax_tree"
    /// Action:     "click", "type_text", "key_combo", "launch_app", "window_list", "focus_window"
    /// Canvas:     "canvas_show", "canvas_hide", "canvas_update"
    pub action: String,

    /// Screen region for screenshot {"x":0,"y":0,"width":1920,"height":1080}. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<ScreenRegion>,

    /// Base64 image for OCR. If absent, captures current screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,

    /// App bundle ID for ax_tree. Example: "com.apple.Safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_bundle_id: Option<String>,

    /// X coordinate for click (pixels).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,

    /// Y coordinate for click (pixels).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,

    /// Mouse button: "left", "right", "middle". Default: "left".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButton>,

    /// Text to type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Key combination. Example: ["cmd","c"] for Copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,

    /// App bundle ID to launch. Example: "com.apple.safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,

    /// Window ID to focus (from window_list results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,

    /// HTML content for canvas_show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,

    /// Canvas overlay position {"x":100,"y":100,"width":800,"height":600}.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<CanvasPosition>,

    /// A2UI patch for canvas_update (JSON array of patch operations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,

    /// Element ref from snapshot (alternative to x/y coordinates).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub ref_id: Option<String>,

    /// Start element ref for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ref: Option<String>,

    /// Start X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_x: Option<f64>,

    /// Start Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_y: Option<f64>,

    /// End element ref for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ref: Option<String>,

    /// End X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_x: Option<f64>,

    /// End Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_y: Option<f64>,

    /// Horizontal scroll amount in pixels (negative=left).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,

    /// Vertical scroll amount in pixels (negative=up).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,

    /// Drag duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Max AX tree depth for snapshot (default: 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,

    /// Include non-interactive elements in snapshot refs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_non_interactive: Option<bool>,
}

/// Output from desktop operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Desktop Bridge tool — gives the AI agent eyes and hands on the desktop.
#[derive(Clone)]
pub struct DesktopTool {
    client: DesktopBridgeClient,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    #[cfg(feature = "desktop-native")]
    native: Option<Arc<dyn aleph_desktop::DesktopCapability>>,
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
            approval_policy: None,
            #[cfg(feature = "desktop-native")]
            native: None,
        }
    }

    /// Attach a native desktop capability for in-process execution.
    ///
    /// When set, supported actions (screenshot, ocr, click, type_text,
    /// key_combo, scroll, window_list, focus_window, launch_app) use the
    /// native in-process path instead of IPC to the Desktop Bridge.
    /// Unsupported actions (canvas_*, snapshot, PIM, etc.) fall through
    /// to the IPC bridge automatically.
    #[cfg(feature = "desktop-native")]
    pub fn with_native(mut self, native: Arc<dyn aleph_desktop::DesktopCapability>) -> Self {
        self.native = Some(native);
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

// ── Native in-process execution path ───────────────────────────────
#[cfg(feature = "desktop-native")]
impl DesktopTool {
    /// Execute a desktop action via the native in-process path.
    ///
    /// Returns `Ok(Some(output))` if the action was handled natively,
    /// or `Ok(None)` to signal that the caller should fall through to IPC.
    async fn call_native(
        &self,
        native: &Arc<dyn aleph_desktop::DesktopCapability>,
        args: &DesktopArgs,
    ) -> Result<Option<DesktopOutput>> {
        match args.action.as_str() {
            "screenshot" => {
                let region = match args.region.as_ref() {
                    Some(r) => {
                        // Validate f64 → u32 conversion: reject negative values and
                        // values that exceed u32::MAX to avoid silent truncation.
                        if r.x < 0.0 || r.y < 0.0 || r.width < 0.0 || r.height < 0.0 {
                            return Ok(Some(DesktopOutput {
                                success: false,
                                data: None,
                                message: Some(
                                    "screenshot region coordinates must be non-negative"
                                        .to_string(),
                                ),
                            }));
                        }
                        if r.x > u32::MAX as f64
                            || r.y > u32::MAX as f64
                            || r.width > u32::MAX as f64
                            || r.height > u32::MAX as f64
                        {
                            return Ok(Some(DesktopOutput {
                                success: false,
                                data: None,
                                message: Some(
                                    "screenshot region coordinates exceed maximum value"
                                        .to_string(),
                                ),
                            }));
                        }
                        Some(aleph_desktop::ScreenRegion {
                            x: r.x as u32,
                            y: r.y as u32,
                            width: r.width as u32,
                            height: r.height as u32,
                        })
                    }
                    None => None,
                };
                match native.screenshot(region).await {
                    Ok(s) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "image_base64": s.image_base64,
                            "width": s.width,
                            "height": s.height,
                            "format": s.format,
                        })),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "ocr" => {
                // If image_base64 is provided, decode it to raw bytes for the native API.
                // Otherwise pass None to capture a fresh screenshot.
                let png_bytes = match &args.image_base64 {
                    Some(b64) => {
                        use base64::Engine;
                        match base64::engine::general_purpose::STANDARD.decode(b64) {
                            Ok(bytes) => Some(bytes),
                            Err(e) => {
                                return Ok(Some(DesktopOutput {
                                    success: false,
                                    data: None,
                                    message: Some(format!("Invalid base64 image: {e}")),
                                }));
                            }
                        }
                    }
                    None => None,
                };
                let result = native
                    .ocr(png_bytes.as_deref())
                    .await;
                match result {
                    Ok(ocr) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "text": ocr.full_text,
                            "lines": ocr.lines.iter().map(|l| {
                                serde_json::json!({
                                    "text": l.text,
                                    "bounding_box": l.bounding_box.as_ref().map(|b| {
                                        serde_json::json!({"x": b.x, "y": b.y, "w": b.w, "h": b.h})
                                    }),
                                    "confidence": l.confidence,
                                })
                            }).collect::<Vec<_>>(),
                        })),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "click" => {
                // Native click requires x/y coordinates (ref-based targeting
                // is only available via the IPC bridge which has snapshot context).
                if args.ref_id.is_some() {
                    return Ok(None); // Fall through to IPC for ref-based clicks
                }
                let x = match args.x {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let y = match args.y {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let button = match args.button.as_ref().unwrap_or(&MouseButton::Left) {
                    MouseButton::Left => aleph_desktop::MouseButton::Left,
                    MouseButton::Right => aleph_desktop::MouseButton::Right,
                    MouseButton::Middle => aleph_desktop::MouseButton::Middle,
                };
                match native.click(x, y, button).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"clicked": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "type_text" => {
                // If ref_id is provided, fall through to IPC (needs snapshot context to focus)
                if args.ref_id.is_some() {
                    return Ok(None);
                }
                let text = args.text.as_deref().unwrap_or("");
                match native.type_text(text).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"typed": true, "chars": text.chars().count()})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "key_combo" => {
                let keys = args.keys.as_deref().unwrap_or(&[]);
                if keys.is_empty() {
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some("key_combo requires 'keys' array".to_string()),
                    }));
                }
                // Last element is the main key, everything before is modifiers
                let (modifiers, main_key) = keys.split_at(keys.len() - 1);
                let modifiers: Vec<String> = modifiers.to_vec();
                let key = &main_key[0];
                match native.key_combo(&modifiers, key).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"combo": keys})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "scroll" => {
                // Native scroll requires x/y coordinates
                if args.ref_id.is_some() {
                    return Ok(None); // Fall through to IPC for ref-based scrolling
                }
                // Convert delta_x/delta_y to direction + amount for native API
                let delta_y = args.delta_y.unwrap_or(0.0);
                let delta_x = args.delta_x.unwrap_or(0.0);
                if delta_x == 0.0 && delta_y == 0.0 {
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(
                            "scroll requires non-zero delta_x or delta_y".to_string(),
                        ),
                    }));
                }
                // Pick the dominant axis
                let (direction, amount) = if delta_y.abs() >= delta_x.abs() {
                    if delta_y < 0.0 {
                        ("up", (-delta_y) as i32)
                    } else {
                        ("down", delta_y as i32)
                    }
                } else if delta_x < 0.0 {
                    ("left", (-delta_x) as i32)
                } else {
                    ("right", delta_x as i32)
                };
                match native.scroll(direction, amount).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "scrolled": true,
                            "direction": direction,
                            "amount": amount,
                        })),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "window_list" => {
                match native.window_list().await {
                    Ok(windows) => {
                        let data: Vec<serde_json::Value> = windows
                            .iter()
                            .map(|w| {
                                serde_json::json!({
                                    "id": w.id,
                                    "title": w.title,
                                    "owner": w.owner,
                                    "pid": w.pid,
                                })
                            })
                            .collect();
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::json!({"windows": data})),
                            message: None,
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "focus_window" => {
                let window_id = match args.window_id {
                    Some(id) => id as u64,
                    None => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "focus_window requires 'window_id' (get it from window_list)"
                                    .to_string(),
                            ),
                        }));
                    }
                };
                match native.focus_window(window_id).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"focused": true, "window_id": window_id})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            "launch_app" => {
                let bundle_id = match args.bundle_id.as_deref() {
                    Some(id) if !id.is_empty() => id,
                    _ => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some("launch_app requires 'bundle_id'".to_string()),
                        }));
                    }
                };
                match native.launch_app(bundle_id).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"launched": true, "bundle_id": bundle_id})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Native desktop error: {e}")),
                    })),
                }
            }
            // Actions not supported natively: snapshot, ax_tree, double_click, drag, hover,
            // paste, canvas_*, PIM_*, etc. — fall through to IPC.
            _ => Ok(None),
        }
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

        // When a native desktop capability is available, try the in-process path
        // first. If the action is not supported natively (call_native returns
        // Ok(None)), fall through to the IPC bridge below.
        #[cfg(feature = "desktop-native")]
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
                     with aleph-server, or can be run standalone via aleph-tauri."
                        .to_string(),
                ),
            });
        }

        let request = match build_request(&args) {
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

/// Build a `DesktopRequest` from tool args, returning an error message string if invalid.
fn build_request(args: &DesktopArgs) -> std::result::Result<DesktopRequest, String> {
    let req = match args.action.as_str() {
        "screenshot" => DesktopRequest::Screenshot {
            region: args.region.clone(),
        },
        "ocr" => DesktopRequest::Ocr {
            image_base64: args.image_base64.clone(),
        },
        "ax_tree" => DesktopRequest::AxTree {
            app_bundle_id: args.app_bundle_id.clone(),
        },
        "click" => {
            let ref_id = args.ref_id.clone();
            let x = args.x;
            let y = args.y;
            if ref_id.is_none() && (x.is_none() || y.is_none()) {
                return Err("click requires 'ref' or both 'x' and 'y' coordinates".to_string());
            }
            DesktopRequest::Click {
                ref_id,
                x,
                y,
                button: args.button.clone().unwrap_or(MouseButton::Left),
            }
        }
        "type_text" => DesktopRequest::TypeText {
            ref_id: args.ref_id.clone(),
            text: args.text.clone().unwrap_or_default(),
        },
        "key_combo" => DesktopRequest::KeyCombo {
            keys: args.keys.clone().unwrap_or_default(),
        },
        "launch_app" => DesktopRequest::LaunchApp {
            bundle_id: args.bundle_id.clone().unwrap_or_default(),
        },
        "window_list" => DesktopRequest::WindowList,
        "focus_window" => {
            let window_id = args
                .window_id
                .ok_or_else(|| "focus_window requires 'window_id' (get it from window_list)".to_string())?;
            DesktopRequest::FocusWindow { window_id }
        }
        "snapshot" => DesktopRequest::Snapshot {
            app_bundle_id: args.app_bundle_id.clone(),
            max_depth: args.max_depth,
            include_non_interactive: args.include_non_interactive,
        },
        "scroll" => {
            let ref_id = args.ref_id.clone();
            let x = args.x;
            let y = args.y;
            if ref_id.is_none() && (x.is_none() || y.is_none()) {
                return Err("scroll requires 'ref' or both 'x' and 'y' coordinates".to_string());
            }
            DesktopRequest::Scroll {
                ref_id, x, y,
                delta_x: args.delta_x.unwrap_or(0.0),
                delta_y: args.delta_y.unwrap_or(0.0),
            }
        }
        "double_click" => {
            let ref_id = args.ref_id.clone();
            let x = args.x;
            let y = args.y;
            if ref_id.is_none() && (x.is_none() || y.is_none()) {
                return Err("double_click requires 'ref' or both 'x' and 'y' coordinates".to_string());
            }
            DesktopRequest::DoubleClick {
                ref_id, x, y,
                button: args.button.clone().unwrap_or(MouseButton::Left),
            }
        }
        "drag" => {
            let has_start = args.start_ref.is_some() || (args.start_x.is_some() && args.start_y.is_some());
            let has_end = args.end_ref.is_some() || (args.end_x.is_some() && args.end_y.is_some());
            if !has_start || !has_end {
                return Err("drag requires start (start_ref or start_x+start_y) and end (end_ref or end_x+end_y)".to_string());
            }
            DesktopRequest::Drag {
                start_ref: args.start_ref.clone(),
                start_x: args.start_x,
                start_y: args.start_y,
                end_ref: args.end_ref.clone(),
                end_x: args.end_x,
                end_y: args.end_y,
                duration_ms: args.duration_ms,
            }
        }
        "hover" => {
            let ref_id = args.ref_id.clone();
            let x = args.x;
            let y = args.y;
            if ref_id.is_none() && (x.is_none() || y.is_none()) {
                return Err("hover requires 'ref' or both 'x' and 'y' coordinates".to_string());
            }
            DesktopRequest::Hover { ref_id, x, y }
        }
        "paste" => DesktopRequest::Paste {
            text: args.text.clone().unwrap_or_default(),
        },
        "canvas_show" => DesktopRequest::CanvasShow {
            html: args.html.clone().unwrap_or_default(),
            position: args.position.clone().unwrap_or(CanvasPosition {
                x: 100.0,
                y: 100.0,
                width: 800.0,
                height: 600.0,
            }),
        },
        "canvas_hide" => DesktopRequest::CanvasHide,
        "canvas_update" => DesktopRequest::CanvasUpdate {
            patch: args.patch.clone().unwrap_or(serde_json::json!([])),
        },
        other => {
            return Err(format!(
                "Unknown desktop action: '{}'. Valid: snapshot, screenshot, ocr, ax_tree, \
                 click, double_click, scroll, drag, hover, type_text, key_combo, paste, \
                 launch_app, window_list, focus_window, \
                 canvas_show, canvas_hide, canvas_update",
                other
            ));
        }
    };
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(action: &str) -> DesktopArgs {
        DesktopArgs {
            action: action.into(),
            region: None,
            image_base64: None,
            app_bundle_id: None,
            x: None,
            y: None,
            button: None,
            text: None,
            keys: None,
            bundle_id: None,
            window_id: None,
            html: None,
            position: None,
            patch: None,
            ref_id: None,
            start_ref: None,
            start_x: None,
            start_y: None,
            end_ref: None,
            end_x: None,
            end_y: None,
            delta_x: None,
            delta_y: None,
            duration_ms: None,
            max_depth: None,
            include_non_interactive: None,
        }
    }

    #[test]
    fn test_build_request_screenshot() {
        let args = make_args("screenshot");
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Screenshot { region: None }));
    }

    #[test]
    fn test_build_request_screenshot_with_region() {
        let mut args = make_args("screenshot");
        args.region = Some(ScreenRegion { x: 10.0, y: 20.0, width: 100.0, height: 200.0 });
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Screenshot { region: Some(_) }));
    }

    #[test]
    fn test_build_request_ocr() {
        let args = make_args("ocr");
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Ocr { image_base64: None }));
    }

    #[test]
    fn test_build_request_click() {
        let mut args = make_args("click");
        args.x = Some(100.0);
        args.y = Some(200.0);
        args.button = Some(MouseButton::Right);
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Click { ref_id: None, button: MouseButton::Right, .. }));
    }

    #[test]
    fn test_build_request_window_list() {
        let args = make_args("window_list");
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::WindowList));
    }

    #[test]
    fn test_build_request_canvas_hide() {
        let args = make_args("canvas_hide");
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::CanvasHide));
    }

    #[test]
    fn test_build_request_key_combo() {
        let mut args = make_args("key_combo");
        args.keys = Some(vec!["cmd".into(), "c".into()]);
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::KeyCombo { .. }));
    }

    #[test]
    fn test_build_request_canvas_show_default_position() {
        let mut args = make_args("canvas_show");
        args.html = Some("<h1>Hello</h1>".into());
        // No position supplied — should use the default 100/100/800/600
        let req = build_request(&args).unwrap();
        if let DesktopRequest::CanvasShow { position, .. } = req {
            assert_eq!(position.x, 100.0);
            assert_eq!(position.width, 800.0);
        } else {
            panic!("expected CanvasShow");
        }
    }

    #[test]
    fn test_build_request_unknown_action() {
        let args = make_args("unknown");
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn test_build_request_unknown_action_message() {
        let args = make_args("fly");
        let err = build_request(&args).unwrap_err();
        assert!(err.contains("fly"), "error should mention the unknown action");
    }

    #[test]
    fn test_build_request_snapshot() {
        let mut args = make_args("snapshot");
        args.max_depth = Some(3);
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Snapshot { max_depth: Some(3), .. }));
    }

    #[test]
    fn test_build_request_click_with_ref() {
        let mut args = make_args("click");
        args.ref_id = Some("e3".into());
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Click { ref_id: Some(_), .. }));
    }

    #[test]
    fn test_build_request_click_no_target() {
        let args = make_args("click");
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn test_build_request_scroll() {
        let mut args = make_args("scroll");
        args.ref_id = Some("e7".into());
        args.delta_y = Some(-300.0);
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Scroll { .. }));
    }

    #[test]
    fn test_build_request_double_click() {
        let mut args = make_args("double_click");
        args.ref_id = Some("e1".into());
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::DoubleClick { .. }));
    }

    #[test]
    fn test_build_request_drag() {
        let mut args = make_args("drag");
        args.start_ref = Some("e1".into());
        args.end_ref = Some("e5".into());
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Drag { .. }));
    }

    #[test]
    fn test_build_request_drag_missing_end() {
        let mut args = make_args("drag");
        args.start_ref = Some("e1".into());
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn test_build_request_hover() {
        let mut args = make_args("hover");
        args.x = Some(100.0);
        args.y = Some(200.0);
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Hover { .. }));
    }

    #[test]
    fn test_build_request_paste() {
        let mut args = make_args("paste");
        args.text = Some("hello".into());
        let req = build_request(&args).unwrap();
        assert!(matches!(req, DesktopRequest::Paste { text } if text == "hello"));
    }

    // ── Approval policy tests ──────────────────────────────────────────

    use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};

    /// A mock policy that returns a fixed decision for all checks.
    struct MockPolicy {
        decision: ApprovalDecision,
    }

    #[async_trait]
    impl ApprovalPolicy for MockPolicy {
        async fn check(&self, _request: &ActionRequest) -> ApprovalDecision {
            self.decision.clone()
        }
        async fn record(&self, _request: &ActionRequest, _decision: &ApprovalDecision) {}
    }

    #[tokio::test]
    async fn test_desktop_approval_deny_blocks_click() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "click blocked".to_string(),
            },
        });
        let tool = DesktopTool::new().with_approval_policy(policy);

        let mut args = make_args("click");
        args.x = Some(100.0);
        args.y = Some(200.0);
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("Action denied"));
    }

    #[tokio::test]
    async fn test_desktop_approval_deny_blocks_type_text() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "type blocked".to_string(),
            },
        });
        let tool = DesktopTool::new().with_approval_policy(policy);

        let mut args = make_args("type_text");
        args.text = Some("secret password".to_string());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("Action denied"));
    }

    #[tokio::test]
    async fn test_desktop_approval_deny_blocks_key_combo() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "key combo blocked".to_string(),
            },
        });
        let tool = DesktopTool::new().with_approval_policy(policy);

        let mut args = make_args("key_combo");
        args.keys = Some(vec!["cmd".into(), "q".into()]);
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("Action denied"));
    }

    #[tokio::test]
    async fn test_desktop_approval_deny_blocks_launch_app() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "launch blocked".to_string(),
            },
        });
        let tool = DesktopTool::new().with_approval_policy(policy);

        let mut args = make_args("launch_app");
        args.bundle_id = Some("com.evil.malware".to_string());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("Action denied"));
    }

    #[tokio::test]
    async fn test_desktop_approval_ask_returns_prompt() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Ask {
                prompt: "Confirm click action".to_string(),
            },
        });
        let tool = DesktopTool::new().with_approval_policy(policy);

        let mut args = make_args("click");
        args.x = Some(500.0);
        args.y = Some(300.0);
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output
            .message
            .as_deref()
            .unwrap()
            .contains("Approval required"));
        let data = output.data.unwrap();
        assert_eq!(data["approval_required"], true);
    }

    #[tokio::test]
    async fn test_desktop_approval_allows_screenshot() {
        // Screenshot is read-only — should never be blocked even with a
        // deny-all policy. The approval gate is not applied.
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "everything denied".to_string(),
            },
        });
        let tool = DesktopTool::new().with_approval_policy(policy);

        let args = make_args("screenshot");
        let output = AlephTool::call(&tool, args).await.unwrap();
        // Should NOT be "Action denied". It will fail on bridge/app not available,
        // which is the expected behavior (approval gate was not triggered).
        assert!(!output.success);
        let msg = output.message.as_deref().unwrap();
        assert!(
            !msg.contains("Action denied"),
            "Read-only action should bypass approval gate, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_desktop_approval_allows_ocr() {
        let policy = Arc::new(MockPolicy {
            decision: ApprovalDecision::Deny {
                reason: "everything denied".to_string(),
            },
        });
        let tool = DesktopTool::new().with_approval_policy(policy);

        let args = make_args("ocr");
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        let msg = output.message.as_deref().unwrap();
        assert!(
            !msg.contains("Action denied"),
            "Read-only action should bypass approval gate, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_desktop_no_policy_allows_all() {
        // Without a policy, mutating actions should proceed as before.
        let tool = DesktopTool::new();

        let mut args = make_args("click");
        args.x = Some(100.0);
        args.y = Some(200.0);
        let output = AlephTool::call(&tool, args).await.unwrap();
        // Should fail on bridge/app not available, NOT on approval
        assert!(!output.success);
        let msg = output.message.as_deref().unwrap();
        assert!(
            !msg.contains("Action denied") && !msg.contains("Approval required"),
            "Without policy, should not hit approval gate, got: {msg}"
        );
    }
}
