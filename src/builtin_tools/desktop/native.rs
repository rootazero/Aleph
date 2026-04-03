//! Platform execution path for desktop actions.

use crate::sync_primitives::Arc;

use super::types::{DesktopArgs, DesktopOutput, MouseButton};
use crate::error::Result;

/// Convert tool-level `MouseButton` to desktop-level `MouseButton`.
fn to_desktop_button(button: Option<&MouseButton>) -> aleph_desktop::MouseButton {
    match button.unwrap_or(&MouseButton::Left) {
        MouseButton::Left => aleph_desktop::MouseButton::Left,
        MouseButton::Right => aleph_desktop::MouseButton::Right,
        MouseButton::Middle => aleph_desktop::MouseButton::Middle,
    }
}

/// Platform execution methods for [`super::DesktopTool`].
impl super::DesktopTool {
    /// Execute a desktop action via `DesktopPlatform.screen()`.
    ///
    /// Returns `Ok(Some(output))` if the action was handled,
    /// or `Ok(None)` to signal that the caller should fall through to the
    /// legacy `call_native` path or report the action as unsupported.
    pub(super) async fn call_via_platform(
        &self,
        platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
        args: &DesktopArgs,
    ) -> Result<Option<DesktopOutput>> {
        let screen = match platform.screen() {
            Some(s) => s,
            None => return Ok(None),
        };

        match args.action.as_str() {
            "screenshot" => {
                let region = match args.region.as_ref() {
                    Some(r) => {
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
                match screen.screenshot(region).await {
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
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "ocr" => {
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
                match screen.ocr(png_bytes.as_deref()).await {
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
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "click" => {
                let x = match args.x {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let y = match args.y {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let button = to_desktop_button(args.button.as_ref());
                match screen.click(x, y, button).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"clicked": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "type_text" => {
                let text = args.text.as_deref().unwrap_or("");
                match screen.type_text(text).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(
                            serde_json::json!({"typed": true, "chars": text.chars().count()}),
                        ),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
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
                let (modifiers, main_key) = keys.split_at(keys.len() - 1);
                let modifiers: Vec<String> = modifiers.to_vec();
                let key = &main_key[0];
                match screen.key_combo(&modifiers, key).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"combo": keys})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "scroll" => {
                let delta_y = args.delta_y.unwrap_or(0.0);
                let delta_x = args.delta_x.unwrap_or(0.0);
                if delta_x == 0.0 && delta_y == 0.0 {
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some("scroll requires non-zero delta_x or delta_y".to_string()),
                    }));
                }
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
                match screen.scroll(direction, amount).await {
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
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "window_list" => match screen.window_list().await {
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
                    message: Some(format!("Screen capability error: {e}")),
                })),
            },
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
                match screen.focus_window(window_id).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"focused": true, "window_id": window_id})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
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
                match screen.launch_app(bundle_id).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"launched": true, "bundle_id": bundle_id})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "screen_record" => {
                let config = aleph_desktop::screen_types::ScreenRecordConfig {
                    duration_secs: args.duration.unwrap_or(5.0),
                    fps: args.fps.unwrap_or(30),
                    with_audio: args.with_audio.unwrap_or(false),
                    region: None,
                };
                match screen.screen_record(config).await {
                    Ok(result) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::to_value(&result).unwrap_or_default()),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen recording error: {e}")),
                    })),
                }
            }
            "double_click" => {
                let x = args.x.unwrap_or(0.0);
                let y = args.y.unwrap_or(0.0);
                let button = to_desktop_button(args.button.as_ref());
                match screen.double_click(x, y, button).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"double_clicked": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "drag" => {
                let sx = args.start_x.unwrap_or(0.0);
                let sy = args.start_y.unwrap_or(0.0);
                let ex = args.end_x.unwrap_or(0.0);
                let ey = args.end_y.unwrap_or(0.0);
                match screen.drag(sx, sy, ex, ey, args.duration_ms).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"dragged": true})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "hover" => {
                let x = args.x.unwrap_or(0.0);
                let y = args.y.unwrap_or(0.0);
                match screen.hover(x, y).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"hovered": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "cursor_position" => {
                match screen.cursor_position().await {
                    Ok((x, y)) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "mouse_button" => {
                let x = args.x.unwrap_or(0.0);
                let y = args.y.unwrap_or(0.0);
                let button = to_desktop_button(args.button.as_ref());
                let press_action = match args.press_action.as_deref() {
                    Some("press") => aleph_desktop::PressAction::Press,
                    Some("release") => aleph_desktop::PressAction::Release,
                    Some("click") | None => aleph_desktop::PressAction::Click,
                    Some(other) => return Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!(
                            "Invalid press_action '{other}'. Use 'press', 'release', or 'click'."
                        )),
                    })),
                };
                match screen.mouse_button(x, y, button, press_action).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "quit_app" => {
                let bundle_id = match args.bundle_id.as_deref() {
                    Some(id) if !id.is_empty() => id,
                    _ => return Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some("quit_app requires 'bundle_id'".to_string()),
                    })),
                };
                match screen.quit_app(bundle_id).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"quit": true, "bundle_id": bundle_id})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "clipboard_read" => {
                match screen.clipboard_read().await {
                    Ok(text) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"text": text})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "clipboard_write" => {
                let text = args.text.as_deref().unwrap_or("");
                match screen.clipboard_write(text).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"written": true})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            _ => Ok(None),
        }
    }
}
