//! Platform execution path for desktop actions.

use crate::sync_primitives::Arc;

use super::types::{DesktopArgs, DesktopOutput, MouseButton};
use crate::error::Result;
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxLocator, PerformActionParams, SetValueParams,
};

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

/// Validate and convert the tool-level `region` (f64; possibly already
/// normalized-then-rescaled by [`super::coord_resolve`]) into the limb-level
/// [`aleph_desktop::ScreenRegion`] (u32).
///
/// Shared by `screenshot` and `screen_record` so a region is honored
/// identically by both — capture and recording validate and clamp the same
/// way, and `coord_space:"normalized"` regions resolve through the one rescale
/// path. Returns `Ok(None)` when no region was supplied (capture the whole
/// display), or a structured validation failure for negative / oversized
/// coordinates.
fn screen_region_from_args(
    args: &DesktopArgs,
    action: &str,
) -> std::result::Result<Option<aleph_desktop::ScreenRegion>, DesktopOutput> {
    let r = match args.region.as_ref() {
        Some(r) => r,
        None => return Ok(None),
    };
    if r.x < 0.0 || r.y < 0.0 || r.width < 0.0 || r.height < 0.0 {
        return Err(invalid_args(format!(
            "{action} region coordinates must be non-negative"
        )));
    }
    if r.x > f64::from(u32::MAX)
        || r.y > f64::from(u32::MAX)
        || r.width > f64::from(u32::MAX)
        || r.height > f64::from(u32::MAX)
    {
        return Err(invalid_args(format!(
            "{action} region coordinates exceed maximum value"
        )));
    }
    Ok(Some(aleph_desktop::ScreenRegion {
        x: r.x as u32,
        y: r.y as u32,
        width: r.width as u32,
        height: r.height as u32,
    }))
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
    /// Build the `screenshot` action output, optionally augmenting it with a
    /// text layer for text-only models.
    ///
    /// When `want_describe` is set and a [`VisionBridge`](super::VisionBridge)
    /// is wired, the captured image is run through the bridge: an `ocr_text`
    /// field (offline, platform-native) and — when a multimodal provider is
    /// registered — a `description` field are attached alongside the raw
    /// `image_base64`. Both are best-effort; an unavailable layer is simply
    /// omitted (P7). With no bridge or `want_describe == false`, the output is
    /// byte-identical to the legacy `{image_base64,width,height,format}` shape.
    ///
    /// When `full_screen` is set (a whole-display capture, not a region crop) a
    /// `coordinate_space` self-description is attached. A full-resolution Retina
    /// capture almost always exceeds the result-size budget and is silently
    /// downscaled before the model sees it, so a model that reads a pixel off
    /// the *served* image and replays it as a `pixel`-space click would land in
    /// the wrong place. The guide tells the model to address points in the
    /// served image's own pixel space via `coord_space:"normalized"` +
    /// `coord_factors:[width,height]`; [`super::coord_resolve`] then maps those
    /// back onto the real display at dispatch, staying correct under any
    /// downscale or DPI scale factor. Region crops are excluded because their
    /// pixels do not map linearly onto the full display.
    async fn screenshot_output(
        &self,
        want_describe: bool,
        full_screen: bool,
        image_base64: String,
        width: u32,
        height: u32,
        format: String,
    ) -> DesktopOutput {
        let mut obj = serde_json::Map::new();

        if want_describe {
            if let Some(ref bridge) = self.vision_bridge {
                let img_fmt = match format.as_str() {
                    "jpeg" | "jpg" => crate::vision::types::ImageFormat::Jpeg,
                    _ => crate::vision::types::ImageFormat::Png,
                };
                let aug = bridge.augment(&image_base64, img_fmt, true).await;
                if let Some(text) = aug.ocr_text {
                    obj.insert("ocr_text".into(), serde_json::json!(text));
                }
                if let Some(desc) = aug.description {
                    obj.insert("description".into(), serde_json::json!(desc));
                }
            }
        }

        obj.insert("image_base64".into(), serde_json::json!(image_base64));
        obj.insert("width".into(), serde_json::json!(width));
        obj.insert("height".into(), serde_json::json!(height));
        obj.insert("format".into(), serde_json::json!(format));

        if full_screen {
            obj.insert(
                "coordinate_space".into(),
                serde_json::json!({
                    "image_width": width,
                    "image_height": height,
                    "note": format!(
                        "This image is {width}x{height}px and may be downscaled from the \
                         real display. To click/drag a point you see here at image pixel \
                         (px, py), send coord_space=\"normalized\" with \
                         coord_factors=[{width}, {height}] and x=px, y=py — the runtime \
                         maps it onto the real display (correct under downscale and Retina \
                         scaling). Do NOT replay raw image pixels as pixel-space coords."
                    ),
                }),
            );
        }

        DesktopOutput {
            success: true,
            data: Some(serde_json::Value::Object(obj)),
            message: None,
        }
    }

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
                let region = match screen_region_from_args(args, "screenshot") {
                    Ok(region) => region,
                    Err(out) => return Ok(Some(out)),
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
                                Ok(processed) => Ok(Some(
                                    self.screenshot_output(
                                        args.describe == Some(true),
                                        args.region.is_none(),
                                        processed.image_base64,
                                        processed.width,
                                        processed.height,
                                        processed.format,
                                    )
                                    .await,
                                )),
                                Err(e) => Ok(Some(DesktopOutput {
                                    success: false,
                                    data: None,
                                    message: Some(format!("Screenshot processing error: {e}")),
                                })),
                            }
                        } else {
                            Ok(Some(
                                self.screenshot_output(
                                    args.describe == Some(true),
                                    args.region.is_none(),
                                    s.image_base64,
                                    s.width,
                                    s.height,
                                    s.format,
                                )
                                .await,
                            ))
                        }
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                                    message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
                    })),
                }
            }
            "key_button" => {
                let keys = args.keys.as_deref().unwrap_or(&[]);
                if keys.is_empty() {
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some("key_button requires 'keys' array".to_string()),
                    }));
                }
                let action = match args.press_action.as_deref() {
                    Some("press") | Some("down") => aleph_desktop::PressAction::Press,
                    Some("release") | Some("up") => aleph_desktop::PressAction::Release,
                    Some("click") | None => aleph_desktop::PressAction::Click,
                    Some(other) => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(format!(
                                "Invalid press_action '{other}'. Use 'press'/'down', 'release'/'up', or 'click'."
                            )),
                        }))
                    }
                };
                match screen.key_button(keys, action).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"keys": keys, "action": args.press_action})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                // Round (not truncate): a non-zero sub-unit delta in (-1, 1) must
                // not silently become a no-op scroll reported as success.
                let (direction, amount) = if delta_y.abs() >= delta_x.abs() {
                    if delta_y < 0.0 {
                        ("up", delta_y.abs().round() as i32)
                    } else {
                        ("down", delta_y.round() as i32)
                    }
                } else if delta_x < 0.0 {
                    ("left", delta_x.abs().round() as i32)
                } else {
                    ("right", delta_x.round() as i32)
                };
                if amount == 0 {
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(
                            "scroll delta too small to move (rounded to 0); use a larger \
                             delta_x/delta_y (e.g. >= 1)"
                                .to_string(),
                        ),
                    }));
                }
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                    message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
                })),
            },
            "focus_window" => {
                let window_id = match args.window_id {
                    Some(id) => u64::from(id),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
                    })),
                }
            }
            "move_window" => {
                let window_id = match args.window_id {
                    Some(id) => u64::from(id),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
                    })),
                }
            }
            "resize_window" => {
                let window_id = match args.window_id {
                    Some(id) => u64::from(id),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
                    })),
                }
            }
            "screen_record" => {
                // Honor `region` like `screenshot` does: a normalized region is
                // already rescaled to pixels by `coord_resolve::maybe_normalize`
                // (it runs for every non-batch action), so recording a sub-rect
                // of the display now works end-to-end instead of being silently
                // widened to the full display.
                let region = match screen_region_from_args(args, "screen_record") {
                    Ok(region) => region,
                    Err(out) => return Ok(Some(out)),
                };
                let config = aleph_desktop::screen_types::ScreenRecordConfig {
                    duration_secs: args.duration.unwrap_or(5.0),
                    fps: args.fps.unwrap_or(30),
                    with_audio: args.with_audio.unwrap_or(false),
                    region,
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
                        message: Some(super::recovery::with_hint(format!("Screen recording error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                    message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
                    })),
                }
            }
            "restart_app" => {
                let bundle_id = match args.bundle_id.as_deref() {
                    Some(id) if !id.is_empty() => id,
                    _ => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some("restart_app requires 'bundle_id'".to_string()),
                        }))
                    }
                };
                // Prefer SystemCapability::restart_app: it encapsulates the
                // quit -> settle -> launch sequence in one place (and lets a
                // platform supply its own restart override), avoiding the
                // duplicated 500ms magic constant. Mirrors the system-preferred
                // clipboard_read pattern below; fall back to the hand-rolled
                // screen sequence only when no system capability is wired.
                if let Some(system) = platform.system() {
                    return Ok(Some(match system.restart_app(bundle_id).await {
                        Ok(()) => DesktopOutput {
                            success: true,
                            data: Some(
                                serde_json::json!({"restarted": true, "bundle_id": bundle_id}),
                            ),
                            message: None,
                        },
                        Err(e) => DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(format!("System capability error: {e}"))),
                        },
                    }));
                }
                match screen.quit_app(bundle_id).await {
                    Ok(()) => {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        match screen.launch_app(bundle_id).await {
                            Ok(()) => Ok(Some(DesktopOutput {
                                success: true,
                                data: Some(
                                    serde_json::json!({"restarted": true, "bundle_id": bundle_id}),
                                ),
                                message: None,
                            })),
                            Err(e) => Ok(Some(DesktopOutput {
                                success: false,
                                data: None,
                                message: Some(super::recovery::with_hint(format!("Launch failed after quit: {e}"))),
                            })),
                        }
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!("Quit failed: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("System capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                    message: Some(super::recovery::with_hint(format!("Screen capability error: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Failed to write to clipboard: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("Failed to paste: {e}"))),
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
            "set_value" => {
                let ax = match platform.ax() {
                    Some(a) => a,
                    None => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(
                                "AX capability not available on this platform — \
                                 fall back to click + type_text."
                                    .into(),
                            )),
                        }))
                    }
                };
                let value = match args.text.as_deref() {
                    Some(t) => t.to_string(),
                    None => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some("set_value requires 'text'".into()),
                        }))
                    }
                };
                let params = SetValueParams {
                    locator: locator_from_args(args),
                    value,
                };
                match ax.set_value(params).await {
                    Ok(r) => Ok(Some(ax_action_output(r))),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "set_value failed: {e}"
                        ))),
                    })),
                }
            }
            "ax_action" => {
                let ax = match platform.ax() {
                    Some(a) => a,
                    None => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(
                                "AX capability not available on this platform — \
                                 fall back to click."
                                    .into(),
                            )),
                        }))
                    }
                };
                let action = match args.ax_action_name.as_deref() {
                    Some(a) => a.to_string(),
                    None => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some("ax_action requires 'ax_action_name'".into()),
                        }))
                    }
                };
                let params = PerformActionParams {
                    locator: locator_from_args(args),
                    action,
                };
                match ax.perform_action(params).await {
                    Ok(r) => Ok(Some(ax_action_output(r))),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "ax_action failed: {e}"
                        ))),
                    })),
                }
            }
            _ => Ok(None),
        }
    }
}

/// Build an [`AxLocator`] from the flat `DesktopArgs` fields used by
/// `set_value` / `ax_action`. `x`/`y` are already coordinate-space-normalized
/// by [`super::coord_resolve::maybe_normalize`] before dispatch, so they can
/// be passed straight through as the locator's nearest-center pixel hint.
fn locator_from_args(args: &DesktopArgs) -> AxLocator {
    AxLocator {
        pid: args.pid,
        role: args.role.clone(),
        title: args.element_title.clone(),
        center: match (args.x, args.y) {
            (Some(x), Some(y)) => Some([x, y]),
            _ => None,
        },
    }
}

/// Convert an [`AxActionResult`] from `ax.set_value` / `ax.perform_action`
/// into a [`DesktopOutput`], surfacing write-verification state in `message`
/// so an unverified write is not silently reported as plain success.
fn ax_action_output(r: AxActionResult) -> DesktopOutput {
    let verified = r
        .verification
        .as_ref()
        .is_some_and(|v| v.state == "verified");
    let message = r.verification.as_ref().and_then(|v| {
        (v.state == "unverified").then(|| {
            format!(
                "Value written but read-back did not match ({}). Re-observe before proceeding.",
                v.reason.as_deref().unwrap_or("unknown")
            )
        })
    });
    DesktopOutput {
        success: r.performed,
        data: serde_json::to_value(&r).ok(),
        message: message.or_else(|| verified.then(|| "Value set and verified.".into())),
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
    fn screen_region_none_when_absent() {
        // No region supplied → capture the whole display (Ok(None)), shared by
        // screenshot and screen_record alike.
        let a = args(serde_json::json!({"action": "screen_record"}));
        assert!(screen_region_from_args(&a, "screen_record")
            .unwrap()
            .is_none());
    }

    #[test]
    fn screen_region_converts_valid_rect_to_u32() {
        let a = args(serde_json::json!({
            "action": "screen_record",
            "region": {"x": 10.0, "y": 20.0, "width": 640.0, "height": 480.0}
        }));
        let region = screen_region_from_args(&a, "screen_record")
            .unwrap()
            .expect("region present");
        assert_eq!(
            (region.x, region.y, region.width, region.height),
            (10, 20, 640, 480)
        );
    }

    #[test]
    fn screen_region_rejects_negative() {
        let a = args(serde_json::json!({
            "action": "screenshot",
            "region": {"x": -1.0, "y": 0.0, "width": 100.0, "height": 100.0}
        }));
        let err = screen_region_from_args(&a, "screenshot").expect_err("negative must reject");
        assert!(!err.success);
        assert!(err.message.unwrap().contains("non-negative"));
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

    // ── Vision bridge wiring (screenshot_output) ──────────────────────

    use crate::builtin_tools::desktop::{DesktopTool, VisionBridge};
    use crate::sync_primitives::Arc;
    use crate::vision::provider::VisionProvider;
    use crate::vision::types::{ImageInput, OcrResult, VisionCapabilities, VisionResult};
    use crate::vision::{VisionError, VisionPipeline};

    /// Minimal provider returning fixed OCR text for wiring assertions.
    struct FixedOcrProvider;

    #[async_trait::async_trait]
    impl VisionProvider for FixedOcrProvider {
        async fn understand_image(
            &self,
            _image: &ImageInput,
            _prompt: &str,
        ) -> std::result::Result<VisionResult, VisionError> {
            Err(VisionError::UnsupportedCapability(
                "image_understanding".into(),
            ))
        }
        async fn ocr(&self, _image: &ImageInput) -> std::result::Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: "Login Submit".into(),
                lines: vec![],
            })
        }
        fn capabilities(&self) -> VisionCapabilities {
            VisionCapabilities {
                image_understanding: false,
                ocr: true,
                object_detection: false,
            }
        }
        fn name(&self) -> &str {
            "fixed-ocr"
        }
    }

    #[tokio::test]
    async fn screenshot_output_without_bridge_is_passthrough() {
        let tool = DesktopTool::new();
        let out = tool
            .screenshot_output(true, true, "Ymd4".into(), 100, 50, "png".into())
            .await;
        assert!(out.success);
        let data = out.data.unwrap();
        assert_eq!(data["image_base64"], "Ymd4");
        assert_eq!(data["width"], 100);
        assert_eq!(data["format"], "png");
        // No bridge → no augmentation fields, byte-identical legacy shape.
        assert!(data.get("ocr_text").is_none());
        assert!(data.get("description").is_none());
        // Full-screen capture carries the coordinate self-description so the
        // model addresses clicks in the served image's pixel space.
        let cs = &data["coordinate_space"];
        assert_eq!(cs["image_width"], 100);
        assert_eq!(cs["image_height"], 50);
        assert!(cs["note"].as_str().unwrap().contains("normalized"));
    }

    #[tokio::test]
    async fn screenshot_output_region_crop_omits_coordinate_space() {
        let tool = DesktopTool::new();
        // A region crop (full_screen=false): normalized coords would map onto
        // the whole display, not the crop, so the guide must be absent.
        let out = tool
            .screenshot_output(false, false, "Ymd4".into(), 100, 50, "png".into())
            .await;
        let data = out.data.unwrap();
        assert!(data.get("coordinate_space").is_none());
    }

    #[tokio::test]
    async fn screenshot_output_attaches_ocr_when_described() {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(FixedOcrProvider));
        let bridge = Arc::new(VisionBridge::new(Arc::new(pipeline)));
        let tool = DesktopTool::new().with_vision_bridge(bridge);

        let out = tool
            .screenshot_output(true, true, "aW1n".into(), 10, 10, "png".into())
            .await;
        let data = out.data.unwrap();
        assert_eq!(data["image_base64"], "aW1n");
        assert_eq!(data["ocr_text"], "Login Submit");
        // OCR-only provider → description degrades to absent (P7).
        assert!(data.get("description").is_none());
    }

    #[tokio::test]
    async fn screenshot_output_skips_bridge_when_not_described() {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(FixedOcrProvider));
        let bridge = Arc::new(VisionBridge::new(Arc::new(pipeline)));
        let tool = DesktopTool::new().with_vision_bridge(bridge);

        // want_describe=false → no augmentation even though a bridge is wired.
        let out = tool
            .screenshot_output(false, true, "aW1n".into(), 10, 10, "png".into())
            .await;
        let data = out.data.unwrap();
        assert_eq!(data["image_base64"], "aW1n");
        assert!(data.get("ocr_text").is_none());
    }
}
