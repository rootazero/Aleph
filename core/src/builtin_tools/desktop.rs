//! Desktop Bridge tool — sees and controls the desktop via the Desktop Bridge.
//!
//! Requires the Aleph Desktop Bridge to be connected. When the bridge is absent,
//! all operations return a friendly message instead of an error.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::desktop::{DesktopBridgeClient, DesktopRequest};
use crate::desktop::types::{CanvasPosition, MouseButton, ScreenRegion};
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
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
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
    const DESCRIPTION: &'static str = r#"Control the desktop: screenshots, OCR, UI automation, keyboard/mouse, app launch, canvas overlays.

Requires the Aleph Desktop Bridge (starts automatically with the server).

Actions:
- screenshot: Capture screen as base64 PNG. Optional region: {x,y,width,height}
- ocr: Extract text from screen (or provided image_base64). Returns {text, lines[]}
- ax_tree: Accessibility tree. Optional app_bundle_id (default: frontmost app)
- click: Click at {x,y}. Optional button: "left"/"right"/"middle"
- type_text: Type text string using keyboard
- key_combo: Press key combo, e.g. keys=["cmd","c"] for Copy
- launch_app: Launch by bundle_id, e.g. "com.apple.Safari"
- window_list: List open windows with IDs, titles, owners
- focus_window: Bring window_id to front
- canvas_show: Render HTML panel at position {x,y,width,height}
- canvas_hide: Hide canvas panel
- canvas_update: Apply A2UI patch to canvas

Examples:
{"action":"screenshot"}
{"action":"ocr"}
{"action":"screenshot","region":{"x":0,"y":0,"width":1920,"height":1080}}
{"action":"click","x":500,"y":300}
{"action":"click","x":500,"y":300,"button":"right"}
{"action":"type_text","text":"Hello, world!"}
{"action":"key_combo","keys":["cmd","c"]}
{"action":"launch_app","bundle_id":"com.apple.Safari"}
{"action":"window_list"}
{"action":"focus_window","window_id":123}
{"action":"canvas_show","html":"<h1>Hello</h1>","position":{"x":100,"y":100,"width":800,"height":600}}
{"action":"canvas_hide"}
{"action":"canvas_update","patch":[{"type":"surfaceUpdate","content":"<p>Updated</p>"}]}"#;

    type Args = DesktopArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
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
            let x = args.x.ok_or_else(|| "click requires 'x' coordinate".to_string())?;
            let y = args.y.ok_or_else(|| "click requires 'y' coordinate".to_string())?;
            DesktopRequest::Click {
                x,
                y,
                button: args.button.clone().unwrap_or(MouseButton::Left),
            }
        }
        "type_text" => DesktopRequest::TypeText {
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
                "Unknown desktop action: '{}'. Valid: screenshot, ocr, ax_tree, \
                 click, type_text, key_combo, launch_app, window_list, focus_window, \
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
        assert!(matches!(req, DesktopRequest::Click { x: _, y: _, button: MouseButton::Right }));
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
}
