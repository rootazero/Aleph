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

/// Build a structured validation-failure output for a known action whose
/// required arguments are missing or malformed.
fn invalid_args(message: impl Into<String>) -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(message.into()),
    }
}

/// Extract the `x`/`y` pair required by point actions (click, hover, …).
///
/// Returns a clear validation error rather than silently defaulting to
/// `(0.0, 0.0)` — a click at the screen's top-left corner can hit the
/// system menu or a window's close button.
fn require_xy(args: &DesktopArgs, action: &str) -> std::result::Result<(f64, f64), DesktopOutput> {
    match (args.x, args.y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(invalid_args(format!(
            "{action} requires numeric 'x' and 'y' coordinates"
        ))),
    }
}

/// Fit a clipboard image (base64 PNG from the limb) within the tool-result
/// budget. A pasted screenshot easily exceeds it, and the generic result
/// budget would then truncate the base64 into an undecodable image — the same
/// footgun the `screenshot` action guards against. Small images pass through
/// untouched; oversized ones are re-encoded to budget-fitting JPEG via the
/// shared screenshot pipeline (reused, not duplicated).
async fn fit_clipboard_image(png_base64: String) -> Result<String> {
    use base64::Engine;

    let est_raw_bytes = png_base64.len() / 4 * 3;
    if est_raw_bytes <= aleph_desktop::perception::DEFAULT_SCREENSHOT_MAX_BYTES {
        return Ok(png_base64);
    }

    let raw = base64::engine::general_purpose::STANDARD
        .decode(&png_base64)
        .map_err(|e| {
            crate::error::AlephError::other(format!("clipboard image base64 decode: {e}"))
        })?;

    let processed = tokio::task::spawn_blocking(move || {
        aleph_desktop::perception::process_screenshot(
            &raw,
            None,
            None,
            "jpeg",
            90,
            Some(aleph_desktop::perception::DEFAULT_SCREENSHOT_MAX_BYTES),
        )
    })
    .await
    .map_err(|e| crate::error::AlephError::other(format!("task join: {e}")))?
    .map_err(|e| crate::error::AlephError::other(format!("clipboard image processing: {e}")))?;

    Ok(processed.image_base64)
}

/// Split a single trailing newline off `text` for UI-TARS
/// `type(content='…\n')` submit semantics.
///
/// Returns the text to type and whether a trailing newline was present (in
/// which case the caller emits an explicit Return keypress). Only one trailing
/// `\n` is stripped — interior newlines are left untouched.
fn split_trailing_newline(text: &str) -> (&str, bool) {
    match text.strip_suffix('\n') {
        Some(stripped) => (stripped, true),
        None => (text, false),
    }
}

/// Platform execution methods for [`super::DesktopTool`].
impl super::DesktopTool {
    /// Execute a desktop action via `DesktopPlatform.screen()`.
    ///
    /// Returns `Ok(Some(output))` when the action was recognized and handled
    /// (the output itself may report success or a structured failure), or
    /// `Ok(None)` when the action name is not handled here, so the caller
    /// reports it as unsupported on this platform.
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

                // Extract post-processing params before moving region
                let fmt = args.format.clone();
                let quality = args.quality;
                let max_w = args.max_width;
                let max_h = args.max_height;
                let display_id = args.display_id;
                let needs_processing = fmt.is_some() || max_w.is_some() || max_h.is_some();

                // Capture: specific display or primary
                let screenshot_result = if let Some(did) = display_id {
                    let region_clone = region;
                    tokio::task::spawn_blocking(move || {
                        aleph_desktop::perception::take_screenshot_display(
                            did,
                            region_clone.as_ref(),
                        )
                    })
                    .await
                    .map_err(|e| crate::error::AlephError::other(format!("task join: {e}")))?
                } else {
                    screen.screenshot(region).await
                };

                match screenshot_result {
                    Ok(s) => {
                        // A full-resolution screenshot can be tens of MB once
                        // base64-encoded; the generic tool-result budget would
                        // then truncate the base64 string into an undecodable
                        // image. Estimate the decoded size (base64 is ~4/3 of
                        // raw) and route oversized captures through the
                        // budget-enforcing re-encoder even when the caller
                        // passed no processing parameters.
                        let est_raw_bytes = s.image_base64.len() / 4 * 3;
                        let over_budget =
                            est_raw_bytes > aleph_desktop::perception::DEFAULT_SCREENSHOT_MAX_BYTES;

                        if needs_processing || over_budget {
                            use base64::Engine;
                            let raw_bytes = base64::engine::general_purpose::STANDARD
                                .decode(&s.image_base64)
                                .map_err(|e| {
                                    crate::error::AlephError::other(format!("base64 decode: {e}"))
                                })?;
                            // An explicit request honours the caller's format;
                            // a budget-only re-encode goes straight to JPEG to
                            // skip a wasteful full-resolution PNG round-trip.
                            let out_fmt = match &fmt {
                                Some(f) => f.clone(),
                                None if over_budget => "jpeg".to_string(),
                                None => "png".to_string(),
                            };
                            // Default JPEG quality 0.9 (was 0.75): screenshots
                            // routinely contain small UI text and 0.75 caused
                            // legibility complaints from the LLM consumer. PNG
                            // is unaffected (lossless regardless of quality).
                            let quality_u8 = (quality.unwrap_or(0.9).clamp(0.0, 1.0) * 100.0) as u8;
                            match tokio::task::spawn_blocking(move || {
                                aleph_desktop::perception::process_screenshot(
                                    &raw_bytes,
                                    max_w,
                                    max_h,
                                    &out_fmt,
                                    quality_u8,
                                    Some(aleph_desktop::perception::DEFAULT_SCREENSHOT_MAX_BYTES),
                                )
                            })
                            .await
                            .map_err(|e| {
                                crate::error::AlephError::other(format!("task join: {e}"))
                            })? {
                                Ok(processed) => Ok(Some(DesktopOutput {
                                    success: true,
                                    data: Some(serde_json::json!({
                                        "image_base64": processed.image_base64,
                                        "width": processed.width,
                                        "height": processed.height,
                                        "format": processed.format,
                                    })),
                                    message: None,
                                })),
                                Err(e) => Ok(Some(DesktopOutput {
                                    success: false,
                                    data: None,
                                    message: Some(format!("Screenshot processing error: {e}")),
                                })),
                            }
                        } else {
                            Ok(Some(DesktopOutput {
                                success: true,
                                data: Some(serde_json::json!({
                                    "image_base64": s.image_base64,
                                    "width": s.width,
                                    "height": s.height,
                                    "format": s.format,
                                })),
                                message: None,
                            }))
                        }
                    }
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
                let (x, y) = match require_xy(args, "click") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
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
                // UI-TARS `type(content='…\n')` parity: a single trailing
                // newline means "type the text, then submit". We strip it and
                // emit an explicit Return keypress, which is reliable across
                // platforms — passing a literal `\n` to the text injector
                // behaves inconsistently in single-line fields.
                let raw = args.text.as_deref().unwrap_or("");
                let (text, submit) = split_trailing_newline(raw);
                match screen.type_text(text).await {
                    Ok(()) => {
                        if submit {
                            if let Err(e) = screen.key_combo(&[], "return").await {
                                return Ok(Some(DesktopOutput {
                                    success: false,
                                    data: None,
                                    message: Some(format!("Screen capability error: {e}")),
                                }));
                            }
                        }
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::json!({
                                "typed": true,
                                "chars": text.chars().count(),
                                "submitted": submit,
                            })),
                            message: None,
                        }))
                    }
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
                        ("up", delta_y.abs() as i32)
                    } else {
                        ("down", delta_y as i32)
                    }
                } else if delta_x < 0.0 {
                    ("left", delta_x.abs() as i32)
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
            "move_window" => {
                let window_id = match args.window_id {
                    Some(id) => id as u64,
                    None => {
                        return Ok(Some(invalid_args(
                            "move_window requires 'window_id' (get it from window_list)",
                        )));
                    }
                };
                let (x, y) = match require_xy(args, "move_window") {
                    Ok((x, y)) => (x as i32, y as i32),
                    Err(out) => return Ok(Some(out)),
                };
                match screen.move_window(window_id, x, y).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "moved": true, "window_id": window_id, "x": x, "y": y,
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
            "resize_window" => {
                let window_id = match args.window_id {
                    Some(id) => id as u64,
                    None => {
                        return Ok(Some(invalid_args(
                            "resize_window requires 'window_id' (get it from window_list)",
                        )));
                    }
                };
                let (width, height) = match (args.width, args.height) {
                    (Some(w), Some(h)) => (w, h),
                    _ => {
                        return Ok(Some(invalid_args(
                            "resize_window requires numeric 'width' and 'height'",
                        )));
                    }
                };
                match screen.resize_window(window_id, width, height).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "resized": true, "window_id": window_id,
                            "width": width, "height": height,
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
                let (x, y) = match require_xy(args, "double_click") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
                };
                let button = to_desktop_button(args.button.as_ref());
                match screen.double_click(x, y, button).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"double_clicked": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "drag" => {
                let (sx, sy, ex, ey) = match (args.start_x, args.start_y, args.end_x, args.end_y) {
                    (Some(sx), Some(sy), Some(ex), Some(ey)) => (sx, sy, ex, ey),
                    _ => {
                        return Ok(Some(invalid_args(
                            "drag requires numeric 'start_x', 'start_y', 'end_x' and 'end_y'",
                        )));
                    }
                };
                match screen.drag(sx, sy, ex, ey, args.duration_ms).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"dragged": true})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "hover" => {
                let (x, y) = match require_xy(args, "hover") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
                };
                match screen.hover(x, y).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"hovered": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "cursor_position" => match screen.cursor_position().await {
                Ok((x, y)) => Ok(Some(DesktopOutput {
                    success: true,
                    data: Some(serde_json::json!({"x": x, "y": y})),
                    message: None,
                })),
                Err(e) => Ok(Some(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Screen capability error: {e}")),
                })),
            },
            "mouse_button" => {
                let (x, y) = match require_xy(args, "mouse_button") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
                };
                let button = to_desktop_button(args.button.as_ref());
                let press_action = match args.press_action.as_deref() {
                    Some("press") => aleph_desktop::PressAction::Press,
                    Some("release") => aleph_desktop::PressAction::Release,
                    Some("click") | None => aleph_desktop::PressAction::Click,
                    Some(other) => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(format!(
                            "Invalid press_action '{other}'. Use 'press', 'release', or 'click'."
                        )),
                        }))
                    }
                };
                match screen.mouse_button(x, y, button, press_action).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "quit_app" => {
                let bundle_id = match args.bundle_id.as_deref() {
                    Some(id) if !id.is_empty() => id,
                    _ => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some("quit_app requires 'bundle_id'".to_string()),
                        }))
                    }
                };
                match screen.quit_app(bundle_id).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"quit": true, "bundle_id": bundle_id})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "clipboard_read" => match platform.system() {
                // Prefer the system capability: macOS (NSPasteboard PNG/TIFF)
                // and Linux (wl-paste/xclip image/*) surface clipboard images as
                // base64 PNG decoded natively in the limb, which the text-only
                // screen path silently drops. Windows still returns text only
                // (has_image=false), so this stays backward compatible while
                // letting vision-capable agents see a copied image.
                Some(system) => match system.clipboard_read().await {
                    Ok(content) => {
                        let mut obj = serde_json::Map::new();
                        obj.insert("text".into(), serde_json::json!(content.text));
                        obj.insert("has_image".into(), serde_json::json!(content.has_image));
                        if let Some(img) = content.image_base64 {
                            let fitted = fit_clipboard_image(img).await?;
                            obj.insert("image_base64".into(), serde_json::json!(fitted));
                        }
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::Value::Object(obj)),
                            message: None,
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("System capability error: {e}")),
                    })),
                },
                // No system capability wired: fall back to the text-only screen
                // path (unchanged behavior).
                None => match screen.clipboard_read().await {
                    Ok(text) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"text": text, "has_image": false})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                },
            },
            "clipboard_write" => {
                let text = args.text.as_deref().unwrap_or("");
                match screen.clipboard_write(text).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"written": true})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "display_list" => match screen.display_list().await {
                Ok(displays) => {
                    let data: Vec<serde_json::Value> = displays
                        .iter()
                        .map(|d| {
                            serde_json::json!({
                                "id": d.id,
                                "name": d.name,
                                "width": d.width,
                                "height": d.height,
                                "scale_factor": d.scale_factor,
                                "is_primary": d.is_primary,
                                "origin_x": d.origin_x,
                                "origin_y": d.origin_y,
                            })
                        })
                        .collect();
                    Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"displays": data})),
                        message: None,
                    }))
                }
                Err(e) => Ok(Some(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Screen capability error: {e}")),
                })),
            },
            "paste" => {
                let text = args.text.as_deref().unwrap_or("");

                // Save current clipboard (best effort)
                let saved = screen.clipboard_read().await.ok();

                // Write target text to clipboard
                if let Err(e) = screen.clipboard_write(text).await {
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Failed to write to clipboard: {e}")),
                    }));
                }

                // Paste shortcut: Cmd+V on macOS, Ctrl+V on Linux/Windows.
                #[cfg(target_os = "macos")]
                let paste_modifier = "meta";
                #[cfg(not(target_os = "macos"))]
                let paste_modifier = "ctrl";

                if let Err(e) = screen.key_combo(&[paste_modifier.into()], "v").await {
                    if let Some(ref original) = saved {
                        if let Err(restore_err) = screen.clipboard_write(original).await {
                            tracing::warn!(
                                error = %restore_err,
                                "Failed to restore original clipboard after paste"
                            );
                        }
                    }
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("Failed to paste: {e}")),
                    }));
                }

                // Wait for paste to take effect
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // Restore original clipboard (best effort)
                if let Some(original) = saved {
                    if let Err(restore_err) = screen.clipboard_write(&original).await {
                        tracing::warn!(
                            error = %restore_err,
                            "Failed to restore original clipboard after paste"
                        );
                    }
                }

                Ok(Some(DesktopOutput {
                    success: true,
                    data: Some(serde_json::json!({"pasted": true, "chars": text.chars().count()})),
                    message: None,
                }))
            }
            "wait_visual" => {
                let region = args.region.clone();
                let output =
                    super::wait_visual::run_wait_visual(screen, args.timeout_ms, region).await;
                Ok(Some(output))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: serde_json::Value) -> DesktopArgs {
        serde_json::from_value(value).expect("valid DesktopArgs")
    }

    #[test]
    fn require_xy_rejects_missing_coordinates() {
        let err = require_xy(&args(serde_json::json!({"action": "click"})), "click")
            .expect_err("missing x/y must be rejected");
        assert!(!err.success);
        assert!(err.message.unwrap().contains("'x' and 'y'"));
    }

    #[test]
    fn require_xy_rejects_partial_coordinates() {
        let partial = args(serde_json::json!({"action": "click", "x": 10.0}));
        assert!(require_xy(&partial, "click").is_err());
    }

    #[test]
    fn require_xy_accepts_full_coordinates() {
        let full = args(serde_json::json!({"action": "click", "x": 12.0, "y": 34.0}));
        assert_eq!(require_xy(&full, "click").unwrap(), (12.0, 34.0));
    }

    #[test]
    fn trailing_newline_signals_submit() {
        assert_eq!(split_trailing_newline("search\n"), ("search", true));
    }

    #[test]
    fn no_trailing_newline_does_not_submit() {
        assert_eq!(split_trailing_newline("search"), ("search", false));
    }

    #[test]
    fn only_one_trailing_newline_is_stripped() {
        // Interior newlines stay; a bare newline means "just submit".
        assert_eq!(split_trailing_newline("a\nb\n"), ("a\nb", true));
        assert_eq!(split_trailing_newline("\n"), ("", true));
    }
}
