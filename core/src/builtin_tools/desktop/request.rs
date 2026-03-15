//! Build a `DesktopRequest` from tool arguments.

use crate::desktop::types::{CanvasPosition, MouseButton};
use crate::desktop::DesktopRequest;
use super::types::DesktopArgs;

/// Build a `DesktopRequest` from tool args, returning an error message string if invalid.
pub(super) fn build_request(args: &DesktopArgs) -> std::result::Result<DesktopRequest, String> {
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
