//! Platform execution path for desktop actions.
//!
//! # Two input rails
//!
//! Every coordinate-space and keyboard action can be delivered two ways:
//!
//! * **targeted** — posted into one process's event queue. The user's physical
//!   cursor never moves and the target app does not have to be frontmost. This
//!   is the rail the AX write path (`set_value` / `ax_action`) has always had,
//!   now available to clicks and keystrokes too.
//! * **global** — posted to the system-wide HID event tap. It *drags the user's
//!   real cursor across the screen* and only lands where the target app is
//!   already frontmost.
//!
//! Naming a target process (`pid`, `app`, or `window_id`) selects the targeted
//! rail. Naming none selects the global one — which, on a platform that *can*
//! target ([`ScreenCapability::supports_targeted_input`]), is refused unless
//! `[desktop] allow_global_pointer = true`. That refusal is deliberate and it is
//! fail-closed: there is no "try targeted, silently fall back to global". Which
//! rail an action ran on is a fact the model plans on, so the tool never picks
//! the intrusive one on the model's behalf — it hands back the refusal and the
//! way out, and the model decides (A2).
//!
//! A platform with no targeted rail (Windows, Linux today: the trait defaults to
//! `NotImplemented`) is unaffected — the policy is gated on
//! `supports_targeted_input()`, not on `cfg!`, so its behavior is unchanged.

use crate::sync_primitives::Arc;

use super::types::{DesktopArgs, DesktopOutput, MouseButton};
use crate::error::{AlephError, Result};
use aleph_desktop::system_types::AppInfo;
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxLocator, PerformActionParams, SetValueParams,
};
use aleph_protocol::desktop_bridge::methods::input::{DELIVERY_GLOBAL, DELIVERY_TARGETED};

/// Convert tool-level `MouseButton` to desktop-level `MouseButton`.
fn to_desktop_button(button: Option<&MouseButton>) -> aleph_desktop::MouseButton {
    match button.unwrap_or(&MouseButton::Left) {
        MouseButton::Left => aleph_desktop::MouseButton::Left,
        MouseButton::Right => aleph_desktop::MouseButton::Right,
        MouseButton::Middle => aleph_desktop::MouseButton::Middle,
    }
}

/// Which rail a synthetic input event rides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Rail {
    /// Straight into that process's event queue. Cursor-free, background-safe.
    Targeted(i32),
    /// The global HID tap: moves the user's cursor, needs the app frontmost.
    Global,
}

impl Rail {
    /// The word the model reads out of the result.
    ///
    /// It always names the rail that *actually ran*. Reporting a background
    /// action for an intrusive one would be a lie the model then plans on — and
    /// the whole point of this rail split is that the model can tell.
    const fn delivery(self) -> &'static str {
        match self {
            Self::Targeted(_) => DELIVERY_TARGETED,
            Self::Global => DELIVERY_GLOBAL,
        }
    }
}

/// Actions that synthesize an input event, and therefore pick a rail.
///
/// A static name table, not a judgement about the message: it says which verbs
/// put an event on a wire, nothing about intent (R7/P8).
///
/// `key_button` used to be absent here, on the grounds that a held key had no
/// targeted counterpart. That left a hole rather than an honest gap: with
/// `allow_global_pointer = false`, `key_combo` was refused while
/// `key_button {press_action: "click"}` delivered the *same* keystroke to the
/// user's frontmost window, unrefused and without even reporting which rail it
/// rode. The limb contract now has [`ScreenCapability::key_button_targeted`], so
/// the verb is gated like every other event-synthesizing one.
fn is_input_action(action: &str) -> bool {
    matches!(
        action,
        "click"
            | "double_click"
            | "drag"
            | "hover"
            | "scroll"
            | "mouse_button"
            | "key_button"
            | "type_text"
            | "key_combo"
            | "paste"
    )
}

/// The whole pointer policy, as a pure function of three facts.
///
/// Mechanical: it reads whether the platform *can* target, whether the caller
/// *named* a process, and whether the operator has permitted the intrusive rail.
/// It never looks at what is being clicked or typed.
fn choose_rail(
    supports_targeted: bool,
    pid: Option<i32>,
    allow_global_pointer: bool,
    action: &str,
) -> std::result::Result<Rail, String> {
    if !supports_targeted {
        // No background rail exists here (Windows / Linux today). There is
        // nothing to refuse *in favour of*, so behavior stays exactly as it was.
        return Ok(Rail::Global);
    }
    match pid {
        Some(pid) => Ok(Rail::Targeted(pid)),
        None if allow_global_pointer => Ok(Rail::Global),
        None => Err(format!(
            "{action} refused: with no target process this would run on the global input tap — \
             it drags the user's physical cursor across the screen and only lands if the target \
             app happens to be frontmost. Pass `app` (name or bundle id), `pid`, or `window_id` \
             and the event is delivered into that process in the background, leaving the user's \
             cursor where it is. For text fields and buttons, `set_value` / `ax_action` address \
             the element directly and are more reliable still. To permit the intrusive global \
             path anyway, set [desktop] allow_global_pointer = true."
        )),
    }
}

/// Parse the shared `press_action` argument into a [`PressAction`].
///
/// Single source for both `key_button` and `mouse_button` — they used to each
/// accept a different dialect (keys took `down`/`up` aliases, the mouse did
/// not), so the same word was accepted on one verb and rejected on the other.
fn parse_press_action(
    raw: Option<&str>,
) -> std::result::Result<aleph_desktop::PressAction, String> {
    match raw {
        Some("press") | Some("down") => Ok(aleph_desktop::PressAction::Press),
        Some("release") | Some("up") => Ok(aleph_desktop::PressAction::Release),
        Some("click") | None => Ok(aleph_desktop::PressAction::Click),
        Some(other) => Err(format!(
            "Invalid press_action '{other}'. Use 'press'/'down', 'release'/'up', or 'click'."
        )),
    }
}

/// Resolve an app name / executable / bundle id against the running-app list.
///
/// String matching, not semantics: an exact (case-insensitive) hit on either
/// field wins; otherwise a *unique* substring hit wins; an ambiguous one is
/// handed back with the candidates rather than guessed at — picking which
/// "Chrome" the user meant is the model's call, not the tool's (R7).
fn match_running_app<'a>(
    apps: &'a [AppInfo],
    query: &str,
) -> std::result::Result<&'a AppInfo, String> {
    let q = query.trim().to_lowercase();
    // Only a running app has a pid, and a pid is the entire point here.
    let running: Vec<&AppInfo> = apps.iter().filter(|a| a.pid.is_some()).collect();

    if let Some(exact) = running
        .iter()
        .copied()
        .find(|a| a.name.to_lowercase() == q || a.bundle_id.to_lowercase() == q)
    {
        return Ok(exact);
    }

    let hits: Vec<&AppInfo> = running
        .iter()
        .copied()
        .filter(|a| a.name.to_lowercase().contains(&q) || a.bundle_id.to_lowercase().contains(&q))
        .collect();
    match hits.as_slice() {
        [] => Err(format!(
            "app '{query}' is not running. Launch it first (launch_app with its bundle id), or \
             pass a `pid` from window_list / desktop_som."
        )),
        [only] => Ok(*only),
        many => {
            let names: Vec<&str> = many.iter().map(|a| a.name.as_str()).take(8).collect();
            Err(format!(
                "app '{query}' matches {} running apps ({}). Name one exactly, or pass its `pid`.",
                many.len(),
                names.join(", ")
            ))
        }
    }
}

/// Poll the running-app list until `bundle_id` shows up or the deadline hits.
///
/// `restart_app` is the one place this is used: both dispatch paths report
/// success on *dispatch*, and "the OS accepted the request" is not "the app
/// is back". Three answers, honestly separated: `Some(true)` = observed
/// running, `Some(false)` = deadline hit with no sighting, `None` = the
/// platform cannot list apps (verification impossible, not failed).
async fn verify_app_running(
    system: &dyn aleph_desktop::SystemCapability,
    bundle_id: &str,
) -> Option<bool> {
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
    const POLL: std::time::Duration = std::time::Duration::from_millis(500);
    verify_app_running_within(system, bundle_id, DEADLINE, POLL).await
}

/// The poll loop behind [`verify_app_running`], with the timing as parameters
/// so a test does not have to wait out the real deadline.
async fn verify_app_running_within(
    system: &dyn aleph_desktop::SystemCapability,
    bundle_id: &str,
    deadline: std::time::Duration,
    poll: std::time::Duration,
) -> Option<bool> {
    let start = std::time::Instant::now();
    let mut ever_listed = false;
    loop {
        if let Ok(apps) = system.list_running_apps().await {
            ever_listed = true;
            if match_running_app(&apps, bundle_id).is_ok() {
                return Some(true);
            }
        }
        // A listing failure is not a verdict; the deadline below is.
        if start.elapsed() >= deadline {
            // "never managed to list" is unknown, not absent.
            return if ever_listed { Some(false) } else { None };
        }
        tokio::time::sleep(poll).await;
    }
}

/// The process this action is aimed at, if the caller named one.
///
/// Three ways to say the same thing, in order of directness: an explicit `pid`,
/// an `app` resolved against the running-app list, or the owner of a
/// `window_id`. `Ok(None)` means the caller named no target at all — which is
/// what [`choose_rail`] then rules on.
async fn resolve_target_pid(
    platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
    screen: &dyn aleph_desktop::ScreenCapability,
    args: &DesktopArgs,
) -> std::result::Result<Option<i32>, String> {
    if let Some(pid) = args.pid {
        return Ok(Some(pid));
    }

    if let Some(app) = args.app.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        let system = platform.system().ok_or_else(|| {
            format!(
                "cannot resolve app '{app}' to a process: this platform exposes no running-app \
                 list. Pass `pid` instead."
            )
        })?;
        let apps = system
            .list_running_apps()
            .await
            .map_err(|e| format!("cannot list running apps to resolve '{app}': {e}"))?;
        let hit = match_running_app(&apps, app)?;
        let pid = hit
            .pid
            .ok_or_else(|| format!("app '{app}' reports no pid"))?;
        return i32::try_from(pid)
            .map(Some)
            .map_err(|_| format!("app '{app}' has a pid ({pid}) outside the addressable range"));
    }

    if let Some(window_id) = args.window_id.map(u64::from) {
        let info = super::window_lookup::lookup_window(screen, window_id).await?;
        return super::window_lookup::pid_of(&info).map(Some);
    }

    Ok(None)
}

/// The rail this action will ride, or the refusal that stops it before a single
/// event is synthesized.
///
/// `pub(super)` because the blocked-app guard needs the same answer *before*
/// dispatch: a targeted event never touches the frontmost app, so the guard has
/// to look at the app the event is actually going to.
pub(super) async fn resolve_rail(
    allow_global_pointer: bool,
    platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
    screen: &dyn aleph_desktop::ScreenCapability,
    args: &DesktopArgs,
) -> std::result::Result<Rail, DesktopOutput> {
    let pid = resolve_target_pid(platform, screen, args)
        .await
        .map_err(|message| DesktopOutput {
            success: false,
            data: None,
            message: Some(super::recovery::with_hint(message)),
        })?;

    choose_rail(
        screen.supports_targeted_input(),
        pid,
        allow_global_pointer,
        &args.action,
    )
    .map_err(|message| DesktopOutput {
        success: false,
        data: None,
        message: Some(super::recovery::with_hint(message)),
    })
}

/// Pre-flight `type_text`'s focus, in the rail's own terms.
///
/// The global rail's keystrokes land on whatever the *system* focuses; the
/// targeted rail's land inside one named process, which is usually not the app
/// in front of the user. Both cases are the same question asked of a different
/// subject, so both go through [`super::focus_gate::check`] — the rail just
/// decides who is asked.
///
/// This used to read the system-focused element for the targeted rail too and
/// then bail out whenever it belonged to another process. That was fail-open in
/// the wrong place: the targeted rail is the *default* on macOS, so the branch
/// that ran almost every time was the one that skipped the gate — including its
/// hard refusal to type into a password field. The gate is only as good as the
/// window it is pointed at.
async fn focus_preflight(
    platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
    rail: Rail,
    force: bool,
) -> Option<DesktopOutput> {
    let pid = match rail {
        Rail::Targeted(pid) => Some(pid),
        Rail::Global => None,
    };
    super::focus_gate::check(platform, pid, force).await
}

/// What a served screenshot's pixels are *of* — decides which coordinate guide
/// travels with it.
#[derive(Debug, Clone)]
enum ShotSpace {
    /// A whole display: normalized coordinates map linearly onto it.
    FullScreen,
    /// A crop of a display: its pixels do not map linearly onto the display, so
    /// no guide is attached (unchanged legacy behavior).
    Region,
    /// One window: its pixels are relative to the window's own origin, and only
    /// `coord_space:"window"` maps them back.
    Window {
        window_id: u32,
        bounds: Option<aleph_desktop::BoundingBox>,
    },
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

/// Reject non-finite f64 values (`NaN`, `±Infinity`).
///
/// The standard `< 0.0` and `> MAX` comparisons silently let `NaN` through
/// because `NaN` compares false to every number; `as u32` then casts `NaN` to
/// `0` and `as i32` likewise, so an unfiltered coordinate lands at the screen
/// origin instead of being refused. Centralizing the check here means every
/// coordinate / duration / region field that flows from `DesktopArgs` into
/// the limb goes through the same gate.
fn finite_f64(v: f64, name: &str) -> std::result::Result<f64, DesktopOutput> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(invalid_args(format!(
            "{name} must be a finite number (got {v})"
        )))
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
    // `NaN < 0.0` is false, so the negative check alone would let NaN through;
    // the explicit finite-check closes that hole before the bound checks run.
    if !r.x.is_finite() || !r.y.is_finite() || !r.width.is_finite() || !r.height.is_finite() {
        return Err(invalid_args(format!(
            "{action} region coordinates must be finite numbers"
        )));
    }
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
        (Some(x), Some(y)) => Ok((finite_f64(x, "x")?, finite_f64(y, "y")?)),
        _ => Err(invalid_args(format!(
            "{action} requires numeric 'x' and 'y' coordinates"
        ))),
    }
}

/// Extract the two points a drag is made of. Same discipline as [`require_xy`].
fn require_drag_points(
    args: &DesktopArgs,
) -> std::result::Result<(f64, f64, f64, f64), DesktopOutput> {
    match (args.start_x, args.start_y, args.end_x, args.end_y) {
        (Some(sx), Some(sy), Some(ex), Some(ey)) => Ok((
            finite_f64(sx, "start_x")?,
            finite_f64(sy, "start_y")?,
            finite_f64(ex, "end_x")?,
            finite_f64(ey, "end_y")?,
        )),
        _ => Err(invalid_args(
            "drag requires numeric 'start_x', 'start_y', 'end_x' and 'end_y'",
        )),
    }
}

/// Reject a request that is malformed *as a request*, before ruling on the rail
/// that would have delivered it.
///
/// A coordinate action with no coordinates is broken whichever rail carries it,
/// so the model must be told which argument is wrong — not sent away to find a
/// `pid` only to come back and fail again on the point it never had. Ordering
/// only, not a new refusal: both errors were already true, and the arms below
/// still do their own extraction. This decides which of the two the model sees.
///
/// `scroll` is deliberately absent: its point is required on the targeted rail
/// and optional on the global one (which scrolls at the real cursor), so only
/// the arm that knows the rail can rule on it.
fn reject_malformed_coordinates(args: &DesktopArgs) -> std::result::Result<(), DesktopOutput> {
    match args.action.as_str() {
        "click" | "double_click" | "hover" | "mouse_button" => {
            require_xy(args, &args.action).map(|_| ())
        }
        "drag" => require_drag_points(args).map(|_| ()),
        _ => Ok(()),
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

/// Pixels moved by one wheel detent ("click") of
/// [`aleph_desktop::ScreenCapability::scroll`], whose `amount` is wheel clicks,
/// not pixels.
///
/// The model's only measuring stick is the screenshot it just looked at, so the
/// tool's `delta_x`/`delta_y` are pixels; the limb speaks detents. The
/// conversion lives here, at the one boundary that knows both units.
///
/// 100 is the desktop-wide convention for what one notch of a user's wheel does:
/// a notch is `WHEEL_DELTA` = 120 raw units on Windows and 3 text lines in every
/// major toolkit, which the browsers realize as ~100 px. It cannot be exact —
/// the OS applies its own acceleration and each app picks its own line height —
/// and the contract is deliberately "about what one notch of the wheel does",
/// which is what a model estimating a scroll from a screenshot actually needs.
const PIXELS_PER_SCROLL_CLICK: f64 = 100.0;

/// Convert a positive pixel scroll distance (direction already split off) into
/// wheel clicks.
///
/// Returns the clicks to send and whether the request was quantized *up* to the
/// one-detent floor. A wheel cannot turn less than one notch, so a sub-detent
/// request either moves further than asked or does not move at all — and a
/// no-op reported as success is the worse of the two. The caller says which
/// happened instead of silently rounding to zero.
fn scroll_clicks(pixels: f64) -> (i32, bool) {
    // NaN/Infinity input would otherwise fall through (`NaN.round()` is NaN,
    // `NaN < 1.0` is false, `NaN as i32` is 0 — reported as a successful
    // zero-distance scroll). Saturate to the same extremes finite input would
    // hit, and tell the caller we quantized away the impossible input.
    if !pixels.is_finite() {
        return (i32::MAX, true);
    }
    let rounded = (pixels / PIXELS_PER_SCROLL_CLICK).round();
    if !rounded.is_finite() || rounded < 1.0 {
        return (1, true);
    }
    // Float→int casts saturate in Rust, so an absurd delta lands on i32::MAX
    // rather than wrapping.
    (rounded as i32, false)
}

/// The clipboard as it stood before a `paste` overwrote it, and whether a text
/// write can put it back.
enum ClipboardSnapshot {
    /// Plain text — `clipboard_write` restores it exactly.
    Text(String),
    /// Content no text write can reproduce (an image, a file, a PDF). Writing
    /// text over it is not a restore: every platform's text write clears the
    /// pasteboard first, so "restoring" the empty string a text-only read hands
    /// back would destroy the user's copied image. The phrase describes what was
    /// there, for an honest message.
    Unrestorable(&'static str),
    /// Nothing to put back — the clipboard could not be read.
    Nothing,
}

impl ClipboardSnapshot {
    /// The note a paste owes the model when it could not put the clipboard back.
    fn unrestorable_note(&self) -> Option<String> {
        match self {
            Self::Unrestorable(what) => Some(format!(
                "Note: the clipboard held {what}. A text write cannot reproduce it, so nothing \
                 was written back over it — but the paste itself replaced it, so the clipboard \
                 now holds the pasted text. The original content is gone: tell the user to \
                 re-copy it if they still need it, and prefer type_text over paste when the \
                 clipboard must be preserved."
            )),
            Self::Text(_) | Self::Nothing => None,
        }
    }
}

/// Snapshot the clipboard before `paste` overwrites it.
///
/// Prefers `SystemCapability::clipboard_read`, whose `ClipboardContent` reports
/// the *flavor* on the pasteboard. The text-only `ScreenCapability` path cannot
/// tell "the user copied an image" from "the clipboard is empty" — both come
/// back as `Ok("")` — so on that path an empty string is never treated as
/// restorable text.
async fn snapshot_clipboard(
    platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
    screen: &dyn aleph_desktop::ScreenCapability,
) -> ClipboardSnapshot {
    if let Some(system) = platform.system() {
        return match system.clipboard_read().await {
            Ok(content) if content.has_image => ClipboardSnapshot::Unrestorable("an image"),
            Ok(content) => match content.text {
                Some(text) if !text.is_empty() => ClipboardSnapshot::Text(text),
                // An empty string *flavor* is a genuinely empty clipboard:
                // nothing to put back, nothing to lose.
                Some(_) => ClipboardSnapshot::Nothing,
                // No string flavor at all: something a text write cannot
                // reproduce (a file, a PDF, rich data) — or an empty pasteboard,
                // which macOS reports the same way. Either way, write nothing.
                None => ClipboardSnapshot::Unrestorable(
                    "content that is not plain text (a file, a PDF, or nothing at all)",
                ),
            },
            Err(e) => {
                tracing::warn!(error = %e, "Clipboard snapshot failed; paste will not restore");
                ClipboardSnapshot::Nothing
            }
        };
    }

    match screen.clipboard_read().await {
        Ok(text) if !text.is_empty() => ClipboardSnapshot::Text(text),
        Ok(_) => ClipboardSnapshot::Nothing,
        Err(e) => {
            tracing::warn!(error = %e, "Clipboard snapshot failed; paste will not restore");
            ClipboardSnapshot::Nothing
        }
    }
}

/// Put back what [`snapshot_clipboard`] captured; returns whether the original
/// clipboard is back in place.
///
/// Only `Text` is ever written. A `clipboard_write("")` is not a restore, it is
/// a `clearContents()` — the one move that destroys a clipboard image the tool
/// never owned.
async fn restore_clipboard(
    screen: &dyn aleph_desktop::ScreenCapability,
    saved: &ClipboardSnapshot,
) -> bool {
    let ClipboardSnapshot::Text(original) = saved else {
        return false;
    };
    match screen.clipboard_write(original).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to restore original clipboard after paste");
            false
        }
    }
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
    /// A `coordinate_space` self-description travels with the image whenever its
    /// pixels *can* be mapped back to the screen. A full-resolution Retina
    /// capture almost always exceeds the result-size budget and is silently
    /// downscaled before the model sees it, so a model that reads a pixel off
    /// the *served* image and replays it as a `pixel`-space click would land in
    /// the wrong place. The guide tells the model which space to address the
    /// image in, and [`super::coord_resolve`] maps it back at dispatch:
    ///
    /// * [`ShotSpace::FullScreen`] → `coord_space:"normalized"` +
    ///   `coord_factors:[width,height]`, resolved against the display.
    /// * [`ShotSpace::Window`] → `coord_space:"window"` + `window_id` +
    ///   `coord_factors:[width,height]`, resolved through the window's frame.
    ///   Without this the model would replay window-relative pixels against the
    ///   display and miss by the window's offset.
    /// * [`ShotSpace::Region`] → no guide: a crop's pixels do not map linearly
    ///   onto anything the caller can name (unchanged legacy behavior).
    async fn screenshot_output(
        &self,
        want_describe: bool,
        space: ShotSpace,
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

        match space {
            ShotSpace::FullScreen => {
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
            ShotSpace::Window { window_id, bounds } => {
                let mut cs = serde_json::Map::new();
                cs.insert("image_width".into(), serde_json::json!(width));
                cs.insert("image_height".into(), serde_json::json!(height));
                cs.insert("window_id".into(), serde_json::json!(window_id));
                if let Some(b) = bounds {
                    cs.insert(
                        "window_bounds".into(),
                        serde_json::json!({"x": b.x, "y": b.y, "width": b.w, "height": b.h}),
                    );
                }
                cs.insert(
                    "note".into(),
                    serde_json::json!(format!(
                        "This image is {width}x{height}px of WINDOW {window_id} only — its \
                         pixels are relative to that window's top-left corner, not the \
                         screen's. To act on a point you see here at image pixel (px, py), \
                         send coord_space=\"window\" with window_id={window_id}, \
                         coord_factors=[{width}, {height}] and x=px, y=py; the runtime maps it \
                         back through the window's frame. Replaying these pixels as \
                         pixel-space (or normalized) coords would miss by the window's offset. \
                         Pass window_id on the action too and it is delivered into that \
                         window's process without moving the user's cursor."
                    )),
                );
                obj.insert("coordinate_space".into(), serde_json::Value::Object(cs));
            }
            ShotSpace::Region => {}
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

        // Pick the delivery rail once, before a single event is synthesized, so
        // a refusal costs the user nothing. Non-input actions never consult the
        // policy (and never pay for the pid lookup) — `Global` is inert for them.
        let rail = if is_input_action(&args.action) {
            // Args first, rail second. See `reject_malformed_coordinates`.
            if let Err(malformed) = reject_malformed_coordinates(args) {
                return Ok(Some(malformed));
            }
            match resolve_rail(self.allow_global_pointer, platform, screen, args).await {
                Ok(rail) => rail,
                Err(refusal) => return Ok(Some(refusal)),
            }
        } else {
            Rail::Global
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
                let window_id = args.window_id.map(u64::from);
                let needs_processing = fmt.is_some() || max_w.is_some() || max_h.is_some();

                // Capture: one window, a specific display, or the primary one.
                //
                // A window capture carries its own frame back, which is the only
                // thing that can turn its window-relative pixels into a click —
                // so it is kept and handed to the model, not dropped.
                let mut window_bounds = None;
                let screenshot_result = if let Some(wid) = window_id {
                    if region.is_some() {
                        return Ok(Some(invalid_args(
                            "screenshot: `region` is a rectangle of a display and has no meaning \
                             against `window_id` — pass one or the other. A window capture is \
                             already cropped to the window.",
                        )));
                    }
                    // `show_cursor: false` — the cursor is not UI, and a model
                    // reading pixel coordinates should not be shown one.
                    match screen.screenshot_window(wid, false).await {
                        Ok(shot) => {
                            window_bounds = shot.window_bounds;
                            Ok(shot.image)
                        }
                        Err(e) => Err(e),
                    }
                } else if let Some(did) = display_id {
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

                let space = match (args.window_id, args.region.is_some()) {
                    (Some(window_id), _) => ShotSpace::Window {
                        window_id,
                        bounds: window_bounds,
                    },
                    (None, true) => ShotSpace::Region,
                    (None, false) => ShotSpace::FullScreen,
                };

                match screenshot_result {
                    // A dead capture chain does not always surface as an `Err`:
                    // a locked screen, a revoked screen-recording grant or a
                    // wedged helper hand back a well-formed frame full of
                    // nothing. Such a frame is small, so the budget re-encode
                    // below never decodes it — it would reach the model as real
                    // pixels, carrying a `coordinate_space` that tells the model
                    // to aim at them. Refuse it here, platform-independently.
                    Ok(s) if aleph_desktop::perception::is_degenerate(&s) => {
                        Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(
                                "Screen capture returned a blank frame — no pixels to look at."
                                    .to_string(),
                            )),
                        }))
                    }
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
                            // The re-encode must not drop what `take_screenshot`
                            // reported: the points-to-pixels ratio survives via
                            // the `_with_scale` entry point.
                            let scale_factor = s.scale_factor;
                            match tokio::task::spawn_blocking(move || {
                                aleph_desktop::perception::process_screenshot_with_scale(
                                    &raw_bytes,
                                    max_w,
                                    max_h,
                                    &out_fmt,
                                    quality_u8,
                                    Some(aleph_desktop::perception::DEFAULT_SCREENSHOT_MAX_BYTES),
                                    scale_factor,
                                )
                            })
                            .await
                            .map_err(|e| {
                                crate::error::AlephError::other(format!("task join: {e}"))
                            })? {
                                Ok(processed) => Ok(Some(
                                    self.screenshot_output(
                                        args.describe == Some(true),
                                        space,
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
                                    space,
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
                    })),
                }
            }
            "click" => {
                let (x, y) = match require_xy(args, "click") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
                };
                let button = to_desktop_button(args.button.as_ref());
                let result = match rail {
                    Rail::Targeted(pid) => screen.click_targeted(pid, x, y, button).await,
                    Rail::Global => screen.click(x, y, button).await,
                };
                match result {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "clicked": true, "x": x, "y": y, "delivery": rail.delivery(),
                        })),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
                    })),
                }
            }
            "type_text" => {
                // Pre-flight the focus. On the global rail these keystrokes land
                // on whatever holds focus *now*, which is not necessarily what
                // the model thinks it clicked; on the targeted rail they land
                // inside the named process, so the gate is only asked about that
                // process (see focus_preflight). Fails open when the AX layer
                // cannot say — `force:true` overrides everything but the
                // secure-field hard block.
                if let Some(refusal) =
                    focus_preflight(platform, rail, args.force == Some(true)).await
                {
                    return Ok(Some(refusal));
                }

                // UI-TARS `type(content='…\n')` parity: a single trailing
                // newline means "type the text, then submit". We strip it and
                // emit an explicit Return keypress, which is reliable across
                // platforms — passing a literal `\n` to the text injector
                // behaves inconsistently in single-line fields.
                let raw = args.text.as_deref().unwrap_or("");
                let (text, submit) = split_trailing_newline(raw);
                let typed = match rail {
                    Rail::Targeted(pid) => screen.type_text_targeted(pid, text).await,
                    Rail::Global => screen.type_text(text).await,
                };
                match typed {
                    Ok(()) => {
                        if submit {
                            // The Return must ride the same rail as the text —
                            // a targeted type followed by a global Return would
                            // submit whatever the *user* has focused.
                            let submitted = match rail {
                                Rail::Targeted(pid) => {
                                    screen.key_combo_targeted(pid, &[], "return").await
                                }
                                Rail::Global => screen.key_combo(&[], "return").await,
                            };
                            if let Err(e) = submitted {
                                return Ok(Some(DesktopOutput {
                                    success: false,
                                    data: None,
                                    message: Some(super::recovery::with_hint(format!(
                                        "Screen capability error: {e}"
                                    ))),
                                }));
                            }
                        }
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::json!({
                                "typed": true,
                                "chars": text.chars().count(),
                                "submitted": submit,
                                "delivery": rail.delivery(),
                            })),
                            message: None,
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                let key = main_key
                    .first()
                    .expect("invariant: main_key has exactly one element after split");
                let result = match rail {
                    Rail::Targeted(pid) => screen.key_combo_targeted(pid, &modifiers, key).await,
                    Rail::Global => screen.key_combo(&modifiers, key).await,
                };
                match result {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"combo": keys, "delivery": rail.delivery()})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                let action = match parse_press_action(args.press_action.as_deref()) {
                    Ok(action) => action,
                    Err(message) => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(message),
                        }))
                    }
                };
                let session_id = super::held_inputs::current_session_id();
                let result = match rail {
                    Rail::Targeted(pid) => screen.key_button_targeted(pid, keys, action).await,
                    Rail::Global => screen.key_button(keys, action).await,
                };
                match result {
                    Ok(()) => {
                        // Ledger only what the OS actually took: a failed press
                        // holds nothing, and `Click` releases what it pressed in
                        // the same call. The abort path releases the rest.
                        match action {
                            aleph_desktop::PressAction::Press => {
                                // Record the rail so the release rides the same
                                // one — a targeted press is only matched by a
                                // targeted release.
                                let held_pid = match rail {
                                    Rail::Targeted(pid) => Some(pid),
                                    Rail::Global => None,
                                };
                                super::held_inputs::record_key_press(&session_id, keys, held_pid);
                            }
                            aleph_desktop::PressAction::Release => {
                                super::held_inputs::clear_key_release(&session_id, keys);
                            }
                            aleph_desktop::PressAction::Click => {}
                        }
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::json!({
                                "keys": keys,
                                "action": args.press_action,
                                "delivery": rail.delivery(),
                            })),
                            message: None,
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
                    })),
                }
            }
            "scroll" => {
                let delta_y = args.delta_y.unwrap_or(0.0);
                let delta_x = args.delta_x.unwrap_or(0.0);
                if !delta_x.is_finite() || !delta_y.is_finite() {
                    return Ok(Some(invalid_args(
                        "scroll requires finite numeric delta_x/delta_y (pixels)",
                    )));
                }
                if delta_x == 0.0 && delta_y == 0.0 {
                    return Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some("scroll requires non-zero delta_x or delta_y".to_string()),
                    }));
                }
                // `delta_*` are pixels (what the model can measure off a
                // screenshot); the limb scrolls in wheel clicks. Split the
                // dominant axis into a direction plus a positive pixel distance,
                // then convert.
                let (direction, pixels) = if delta_y.abs() >= delta_x.abs() {
                    if delta_y < 0.0 {
                        ("up", delta_y.abs())
                    } else {
                        ("down", delta_y)
                    }
                } else if delta_x < 0.0 {
                    ("left", delta_x.abs())
                } else {
                    ("right", delta_x)
                };
                let (clicks, quantized) = scroll_clicks(pixels);
                // A point is optional for scroll, and the two rails read a
                // missing one differently. The targeted rail *requires* it: the
                // event carries no cursor, so the point is the only thing that
                // routes the wheel into the right view. The global rail treats a
                // given point as "scroll THIS view" — it lands the real cursor
                // on the point first and then scrolls, so the wheel goes where
                // the model looked instead of wherever the cursor happened to
                // sit. Dropping the point on the global rail (the old behaviour)
                // scrolled an arbitrary view and still reported success; on
                // Windows/Linux, which have no targeted rail, moving-then-
                // scrolling is the ONLY way to scroll a chosen view. No point at
                // all stays valid on the global rail: it scrolls at the real
                // cursor, unchanged.
                let point = require_xy(args, "scroll").ok();
                let result = match rail {
                    Rail::Targeted(pid) => {
                        let Some((x, y)) = point else {
                            return Ok(Some(invalid_args(
                                "a scroll delivered into a specific process needs `x`/`y`: \
                                 the user's cursor never moves, so the point on the event is \
                                 the only thing that tells the app which view to scroll. Pass \
                                 a point inside the target (an element `center` from \
                                 desktop_som / desktop_ax_snapshot works), or drop \
                                 app/pid/window_id to scroll at the real cursor.",
                            )));
                        };
                        screen.scroll_targeted(pid, x, y, direction, clicks).await
                    }
                    Rail::Global => {
                        // Position the cursor on the requested view before
                        // scrolling. If the move itself fails, report it rather
                        // than scrolling blind and claiming the point mattered.
                        if let Some((x, y)) = point {
                            if let Err(e) = screen.hover(x, y).await {
                                return Ok(Some(DesktopOutput {
                                    success: false,
                                    data: None,
                                    message: Some(super::recovery::with_hint(format!(
                                        "Could not move the cursor to ({x:.0},{y:.0}) before \
                                         scrolling there: {e}"
                                    ))),
                                }));
                            }
                        }
                        screen.scroll(direction, clicks).await
                    }
                };
                match result {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "scrolled": true,
                            "direction": direction,
                            "requested_pixels": pixels,
                            "wheel_clicks": clicks,
                            "approx_pixels_moved": f64::from(clicks) * PIXELS_PER_SCROLL_CLICK,
                            "delivery": rail.delivery(),
                            // Where the wheel landed. `[x, y]` when the model
                            // named a point (the cursor was moved there first on
                            // the global rail, or the event was routed there on
                            // the targeted rail); `null` means "at the real
                            // cursor", the global-rail default.
                            "at": point.map(|(x, y)| serde_json::json!([x, y])),
                        })),
                        message: quantized.then(|| {
                            format!(
                                "Scrolled 1 wheel click (~{PIXELS_PER_SCROLL_CLICK:.0}px): the \
                                 requested {pixels:.0}px is below one wheel detent, the smallest \
                                 step a wheel scroll can take, so the screen moved further than \
                                 you asked. Re-observe before acting on coordinates."
                            )
                        }),
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
                    })),
                }
            }
            "window_list" => match screen.window_list().await {
                Ok(windows) => {
                    let data: Vec<serde_json::Value> = windows
                        .iter()
                        .map(|w| {
                            // `bounds` / `layer` / `on_screen` are omitted rather
                            // than defaulted when the platform did not report
                            // them: absent means "unknown", and `layer: 0` would
                            // otherwise read as "a normal app window".
                            let mut obj = serde_json::Map::new();
                            obj.insert("id".into(), serde_json::json!(w.id));
                            obj.insert("title".into(), serde_json::json!(w.title));
                            obj.insert("owner".into(), serde_json::json!(w.owner));
                            obj.insert("pid".into(), serde_json::json!(w.pid));
                            if let Some(b) = w.bounds.as_ref() {
                                obj.insert(
                                    "bounds".into(),
                                    serde_json::json!({
                                        "x": b.x, "y": b.y, "width": b.w, "height": b.h,
                                    }),
                                );
                            }
                            if let Some(layer) = w.layer {
                                obj.insert("layer".into(), serde_json::json!(layer));
                            }
                            if let Some(on_screen) = w.on_screen {
                                obj.insert("on_screen".into(), serde_json::json!(on_screen));
                            }
                            serde_json::Value::Object(obj)
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
                    message: Some(super::recovery::with_hint(format!(
                        "Screen capability error: {e}"
                    ))),
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                            message: Some(
                                "launch_app requires 'bundle_id' — the app's name \
                                 (\"Safari\") or its bundle id (\"com.apple.Safari\"). \
                                 `system` with list_installed_apps enumerates both."
                                    .to_string(),
                            ),
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                // `Duration::from_secs_f64(NaN | Infinity)` panics inside the
                // limb; refuse non-finite input explicitly instead of letting
                // it crash the worker.
                let duration_secs = match args.duration {
                    Some(v) if !v.is_finite() => {
                        return Ok(Some(invalid_args(
                            "screen_record duration must be a finite number of seconds",
                        )));
                    }
                    Some(v) => v,
                    None => 5.0,
                };
                let config = aleph_desktop::screen_types::ScreenRecordConfig {
                    duration_secs,
                    fps: args.fps.unwrap_or(30),
                    with_audio: args.with_audio.unwrap_or(false),
                    region,
                };
                match screen.screen_record(config).await {
                    Ok(result) => {
                        let data = serde_json::to_value(&result).map_err(|e| {
                            AlephError::tool(format!(
                                "screen_record: failed to serialize result: {e}"
                            ))
                        })?;
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(data),
                            message: None,
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen recording error: {e}"
                        ))),
                    })),
                }
            }
            "double_click" => {
                let (x, y) = match require_xy(args, "double_click") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
                };
                let button = to_desktop_button(args.button.as_ref());
                let result = match rail {
                    Rail::Targeted(pid) => screen.double_click_targeted(pid, x, y, button).await,
                    Rail::Global => screen.double_click(x, y, button).await,
                };
                match result {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "double_clicked": true, "x": x, "y": y, "delivery": rail.delivery(),
                        })),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
                    })),
                }
            }
            "drag" => {
                let (sx, sy, ex, ey) = match require_drag_points(args) {
                    Ok(points) => points,
                    Err(out) => return Ok(Some(out)),
                };
                let result = match rail {
                    Rail::Targeted(pid) => {
                        screen
                            .drag_targeted(pid, sx, sy, ex, ey, args.duration_ms)
                            .await
                    }
                    Rail::Global => screen.drag(sx, sy, ex, ey, args.duration_ms).await,
                };
                match result {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(
                            serde_json::json!({"dragged": true, "delivery": rail.delivery()}),
                        ),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
                    })),
                }
            }
            "hover" => {
                let (x, y) = match require_xy(args, "hover") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
                };
                let result = match rail {
                    Rail::Targeted(pid) => screen.hover_targeted(pid, x, y).await,
                    Rail::Global => screen.hover(x, y).await,
                };
                match result {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "hovered": true, "x": x, "y": y, "delivery": rail.delivery(),
                        })),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                    message: Some(super::recovery::with_hint(format!(
                        "Screen capability error: {e}"
                    ))),
                })),
            },
            "mouse_button" => {
                let (x, y) = match require_xy(args, "mouse_button") {
                    Ok(xy) => xy,
                    Err(out) => return Ok(Some(out)),
                };
                let button = to_desktop_button(args.button.as_ref());
                let press_action = match parse_press_action(args.press_action.as_deref()) {
                    Ok(action) => action,
                    Err(message) => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(message),
                        }))
                    }
                };
                let session_id = super::held_inputs::current_session_id();
                let result = match rail {
                    Rail::Targeted(pid) => {
                        screen
                            .mouse_button_targeted(pid, x, y, button, press_action)
                            .await
                    }
                    Rail::Global => screen.mouse_button(x, y, button, press_action).await,
                };
                match result {
                    Ok(()) => {
                        // Same ledger discipline as key_button: a held button
                        // stays physically down on the user's mouse until it is
                        // released, and the abort path is the only other thing
                        // that can hand it back.
                        match press_action {
                            aleph_desktop::PressAction::Press => {
                                // Record which rail the press rode so the ledger
                                // releases it the same way (targeted → targeted).
                                let held_pid = match rail {
                                    Rail::Targeted(pid) => Some(pid),
                                    Rail::Global => None,
                                };
                                super::held_inputs::record_button_press(
                                    &session_id,
                                    button,
                                    x,
                                    y,
                                    held_pid,
                                );
                            }
                            aleph_desktop::PressAction::Release => {
                                super::held_inputs::clear_button_release(&session_id, button);
                            }
                            aleph_desktop::PressAction::Click => {}
                        }
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(
                                serde_json::json!({"x": x, "y": y, "delivery": rail.delivery()}),
                            ),
                            message: None,
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                            message: Some(
                                "quit_app requires 'bundle_id' — the app's name \
                                 (\"Safari\") or its bundle id (\"com.apple.Safari\"). \
                                 `system` with list_installed_apps enumerates both."
                                    .to_string(),
                            ),
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                            message: Some(
                                "restart_app requires 'bundle_id' — the app's name \
                                 (\"Safari\") or its bundle id (\"com.apple.Safari\"). \
                                 `system` with list_installed_apps enumerates both."
                                    .to_string(),
                            ),
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
                        Ok(()) => {
                            // Dispatch succeeded; now say whether the app
                            // actually came back, instead of letting "Ok(())"
                            // stand in for it.
                            let verified = verify_app_running(system, bundle_id).await;
                            let message = match verified {
                                Some(true) => None,
                                Some(false) => Some(format!(
                                    "restart of '{bundle_id}' was dispatched, but the app did \
                                     not reappear in the running list within 10s — check \
                                     whether it crashed on launch"
                                )),
                                None => Some(format!(
                                    "restart of '{bundle_id}' was dispatched; this platform \
                                     cannot list running apps, so its return is unverified"
                                )),
                            };
                            DesktopOutput {
                                success: true,
                                data: Some(serde_json::json!({
                                    "restarted": true,
                                    "bundle_id": bundle_id,
                                    "verified": verified,
                                })),
                                message,
                            }
                        }
                        Err(e) => DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(format!(
                                "System capability error: {e}"
                            ))),
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
                                message: Some(super::recovery::with_hint(format!(
                                    "Launch failed after quit: {e}"
                                ))),
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
                        // Redact token-shaped clipboard text before it crosses
                        // the IPC boundary — a credential the user copied out
                        // of a password manager would otherwise land in the
                        // model context with no defence (see
                        // review-results/clipboard-logic-2026-08-26/REPORT.md
                        // Critical 3). The platform limb's read_text stays raw
                        // for the snapshot/restore pipeline; the IPC layer
                        // applies the secret-redaction gate here.
                        let (redacted_text, redacted_flag) = match content.text.as_deref() {
                            Some(t) => {
                                let r = aleph_desktop::clipboard_redact::redact_clipboard_text(t);
                                (Some(r.text), r.redacted)
                            }
                            None => (None, false),
                        };
                        let mut obj = serde_json::Map::new();
                        obj.insert("text".into(), serde_json::json!(redacted_text));
                        obj.insert("has_image".into(), serde_json::json!(content.has_image));
                        obj.insert("redacted".into(), serde_json::json!(redacted_flag));
                        if let Some(img) = content.image_base64 {
                            let fitted = fit_clipboard_image(img).await?;
                            obj.insert("image_base64".into(), serde_json::json!(fitted));
                        }
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::Value::Object(obj)),
                            // Tell the model explicitly when redaction fired so
                            // it can ask the user rather than guessing.
                            message: redacted_flag.then(|| {
                                "Clipboard text was redacted: a token-shaped \
                                 substring (<REDACTED:candidate-secret>) was \
                                 replaced; ask the user if access is needed."
                                    .to_string()
                            }),
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "System capability error: {e}"
                        ))),
                    })),
                },
                // No system capability wired: fall back to the text-only screen
                // path (unchanged behavior).
                None => match screen.clipboard_read().await {
                    Ok(text) => {
                        // Same redaction gate as the system arm — credentials
                        // on the pasteboard must not land in the model context
                        // either way.
                        let r = aleph_desktop::clipboard_redact::redact_clipboard_text(&text);
                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::json!({
                                "text": r.text,
                                "has_image": false,
                                "redacted": r.redacted,
                            })),
                            message: r.redacted.then(|| {
                                "Clipboard text was redacted: a token-shaped \
                                 substring (<REDACTED:candidate-secret>) was \
                                 replaced; ask the user if access is needed."
                                    .to_string()
                            }),
                        }))
                    }
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                        message: Some(super::recovery::with_hint(format!(
                            "Screen capability error: {e}"
                        ))),
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
                    message: Some(super::recovery::with_hint(format!(
                        "Screen capability error: {e}"
                    ))),
                })),
            },
            "paste" => {
                let text = args.text.as_deref().unwrap_or("");

                // Pre-flight the focus, exactly as `type_text` does. paste is an
                // input action that injects `text` via Cmd/Ctrl+V into whatever
                // holds focus, so it must not bypass the gate type_text enforces:
                // refuse when nothing holds focus / focus is the wrong app, and
                // hard-refuse a secure (password) field even under `force:true`.
                // Without this, "paste is better for multiline than type_text"
                // (per the DESCRIPTION) was a hole straight past focus_gate.
                if let Some(refusal) =
                    focus_preflight(platform, rail, args.force == Some(true)).await
                {
                    return Ok(Some(refusal));
                }

                // Snapshot the clipboard *by flavor*, not as a bare string: an
                // image / file / PDF reads back as no text at all, and writing
                // the empty string over it is a clear, not a restore.
                let saved = snapshot_clipboard(platform, screen).await;

                // Run the body as a single async block so that any early
                // `return` (clipboard_write failure, keypress failure) is
                // followed by a single `restore_clipboard` call. The prior
                // shape called `restore_clipboard` only on the keypress arm
                // — the clipboard_write failure arm returned without
                // restoring, leaving the user's clipboard at whatever the OS
                // gave us after a partial write (panic-/cancellation-safety
                // for the snapshot was loose; see
                // review-results/clipboard-logic-2026-08-26/REPORT.md
                // Critical 1). The restore now fires in every path that
                // already took the snapshot.
                let body: Result<(), DesktopOutput> = async {
                    // Write target text to clipboard
                    if let Err(e) = screen.clipboard_write(text).await {
                        return Err(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(format!(
                                "Failed to write to clipboard: {e}"
                            ))),
                        });
                    }

                    // Paste shortcut: Cmd+V on macOS, Ctrl+V on Linux/Windows.
                    #[cfg(target_os = "macos")]
                    let paste_modifier = "meta";
                    #[cfg(not(target_os = "macos"))]
                    let paste_modifier = "ctrl";

                    let modifiers = [paste_modifier.to_string()];
                    let pasted = match rail {
                        Rail::Targeted(pid) => {
                            screen.key_combo_targeted(pid, &modifiers, "v").await
                        }
                        Rail::Global => screen.key_combo(&modifiers, "v").await,
                    };
                    if let Err(e) = pasted {
                        return Err(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(format!(
                                "Failed to paste: {e}"
                            ))),
                        });
                    }

                    // Wait for paste to take effect
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    Ok(())
                }
                .await;

                // Always restore the snapshot from this single tail site, so
                // the user's prior clipboard is put back whether the body
                // succeeded or failed.
                let restored = restore_clipboard(screen, &saved).await;

                match body {
                    Err(out) => Ok(Some(out)),
                    Ok(()) => {
                        // Compose a user-visible warning when a Text snapshot
                        // failed to restore — the prior shape only fired
                        // `unrestorable_note()` for the `Unrestorable`
                        // variant, leaving the Text snapshot silently broken
                        // for a user who expected their prior copy back (see
                        // Critical 4). A Nothing snapshot, by definition, has
                        // nothing to restore — no warning.
                        let extra_warning = match (&saved, restored) {
                            (ClipboardSnapshot::Text(_), false) => Some(
                                "I could not put back what was on your clipboard. \
                                 The clipboard now holds the pasted text. If you \
                                 had something important on the clipboard before, \
                                 please re-copy it."
                                    .to_string()
                            ),
                            _ => None,
                        };
                        let message = match (extra_warning, saved.unrestorable_note()) {
                            (Some(extra), Some(note)) => Some(format!("{extra}\n{note}")),
                            (Some(extra), None) => Some(extra),
                            (None, Some(note)) => Some(note),
                            (None, None) => None,
                        };

                        Ok(Some(DesktopOutput {
                            success: true,
                            data: Some(serde_json::json!({
                                "pasted": true,
                                "chars": text.chars().count(),
                                "clipboard_restored": restored,
                                "delivery": rail.delivery(),
                            })),
                            message,
                        }))
                    }
                }
            }
            "wait_visual" => {
                let region = args.region.clone();
                let output =
                    super::wait_visual::run_wait_visual(screen, args.timeout_ms, region).await;
                Ok(Some(output))
            }
            "verify_state" => {
                let ax = match platform.ax() {
                    Some(a) => a,
                    None => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(
                                "verify_state needs the accessibility layer, which is not \
                                 available on this platform — use wait_visual (pixel settle) \
                                 or re-screenshot to confirm the result instead."
                                    .into(),
                            )),
                        }))
                    }
                };
                // Resolve app/pid/window_id to a pid the same way the rest of
                // the tool does, so a postcondition can be scoped to the app the
                // model just acted on. `None` means the frontmost app, matching
                // the AX query contract.
                let pid = match resolve_target_pid(platform, screen, args).await {
                    Ok(pid) => pid,
                    Err(message) => {
                        return Ok(Some(DesktopOutput {
                            success: false,
                            data: None,
                            message: Some(super::recovery::with_hint(message)),
                        }))
                    }
                };
                let output = super::verify_state::run_verify_state(
                    ax,
                    pid,
                    &args.expect,
                    args.timeout_ms,
                    args.stable_samples,
                )
                .await;
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
                        message: Some(super::recovery::with_hint(format!("set_value failed: {e}"))),
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
                        message: Some(super::recovery::with_hint(format!("ax_action failed: {e}"))),
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
fn ax_action_output(mut r: AxActionResult) -> DesktopOutput {
    // `matched` is the wire element, serialized verbatim into the result — so a
    // write that landed on a password field would echo its contents back into
    // the model's context. The helper already withholds `actual_preview` for a
    // secure element; this covers the element itself.
    r.matched = r.matched.map(super::interactable::redact_secure_values);

    let verified = r
        .verification
        .as_ref()
        .is_some_and(|v| v.state == "verified");
    let message = r.verification.as_ref().and_then(|v| {
        (v.state == "unverified").then(|| {
            super::recovery::with_hint(format!(
                "Value written but read-back did not match ({}). Re-observe before proceeding.",
                v.reason.as_deref().unwrap_or("unknown")
            ))
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

    // ── verify_app_running_within ────────────────────────────────────────────

    /// Scripted replies for `list_running_apps` (`DesktopError` is not
    /// `Clone`, so the failure is a unit variant materialised per call).
    #[derive(Clone)]
    enum ListReply {
        Apps(Vec<AppInfo>),
        Fail,
    }

    /// A `SystemCapability` whose `list_running_apps` answers from a script:
    /// each call takes the next queued reply, and the last one repeats.
    struct ScriptedSystem {
        script: std::sync::Mutex<std::collections::VecDeque<ListReply>>,
    }

    impl ScriptedSystem {
        fn answering(script: Vec<ListReply>) -> Self {
            Self {
                script: std::sync::Mutex::new(script.into()),
            }
        }
    }

    fn running_app(name: &str) -> AppInfo {
        AppInfo {
            name: name.to_string(),
            bundle_id: format!("com.example.{name}"),
            pid: Some(42),
            is_active: false,
        }
    }

    #[async_trait::async_trait]
    impl aleph_desktop::SystemCapability for ScriptedSystem {
        async fn launch_app(&self, _app: &str) -> aleph_desktop::Result<()> {
            Ok(())
        }
        async fn quit_app(&self, _app: &str) -> aleph_desktop::Result<()> {
            Ok(())
        }
        async fn list_running_apps(&self) -> aleph_desktop::Result<Vec<AppInfo>> {
            let mut script = self.script.lock().expect("script lock");
            let reply = if script.len() > 1 {
                script.pop_front().expect("len checked")
            } else {
                script.front().cloned().unwrap_or(ListReply::Apps(vec![]))
            };
            match reply {
                ListReply::Apps(apps) => Ok(apps),
                ListReply::Fail => Err(aleph_desktop::DesktopError::InputFailed(
                    "compositor gone".to_string(),
                )),
            }
        }
        async fn send_notification(&self, _t: &str, _b: &str) -> aleph_desktop::Result<()> {
            Ok(())
        }
        async fn clipboard_read(
            &self,
        ) -> aleph_desktop::Result<aleph_desktop::system_types::ClipboardContent> {
            unimplemented!("not needed by verify_app_running")
        }
        async fn clipboard_write(&self, _t: &str) -> aleph_desktop::Result<()> {
            unimplemented!("not needed by verify_app_running")
        }
        async fn system_info(
            &self,
        ) -> aleph_desktop::Result<aleph_desktop::system_types::SystemInfo> {
            unimplemented!("not needed by verify_app_running")
        }
    }

    #[tokio::test]
    async fn restart_verification_reports_running_once_the_app_appears() {
        let system = ScriptedSystem::answering(vec![
            ListReply::Apps(vec![]),                      // not yet back
            ListReply::Apps(vec![running_app("Safari")]), // back
        ]);
        let verdict = verify_app_running_within(
            &system,
            "com.example.Safari",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(1),
        )
        .await;
        assert_eq!(verdict, Some(true));
    }

    #[tokio::test]
    async fn restart_verification_reports_absent_when_the_list_never_shows_it() {
        let system = ScriptedSystem::answering(vec![ListReply::Apps(vec![])]);
        let verdict = verify_app_running_within(
            &system,
            "com.example.Safari",
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
        )
        .await;
        assert_eq!(verdict, Some(false));
    }

    #[tokio::test]
    async fn restart_verification_reports_unknown_when_listing_never_works() {
        let system = ScriptedSystem::answering(vec![ListReply::Fail]);
        let verdict = verify_app_running_within(
            &system,
            "com.example.Safari",
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
        )
        .await;
        assert_eq!(verdict, None);
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
    fn finite_f64_rejects_non_finite() {
        // The shared helper used by require_xy and require_drag_points; its
        // behavior is what every f64-typed caller ultimately depends on.
        assert!(finite_f64(0.0, "x").is_ok());
        assert!(finite_f64(-1.0, "x").is_ok()); // negatives are finite; bounded elsewhere
        assert!(finite_f64(f64::NAN, "x").is_err());
        assert!(finite_f64(f64::INFINITY, "x").is_err());
        assert!(finite_f64(f64::NEG_INFINITY, "x").is_err());
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
    fn screen_region_rejects_non_finite() {
        // NaN < 0.0 is false, so a plain "< 0.0" check would let NaN through
        // and then `NaN as u32` would silently shrink the region to (0,0,0,0).
        // The value cannot be written as JSON: `serde_json::json!` turns NaN
        // and the infinities into `null` (JSON has no way to spell them), and
        // `null` will not deserialize into an `f64`. Built through the JSON
        // fixture, this test failed on its own input — before reaching the
        // guard it exists to check — on every run since it was written. So the
        // non-finite value is installed after parsing.
        let make = |val: f64| {
            let mut a = args(serde_json::json!({
                "action": "screenshot",
                "region": {"x": 0.0, "y": 0.0, "width": 100.0, "height": 100.0}
            }));
            a.region.as_mut().expect("region present").x = val;
            a
        };
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = screen_region_from_args(&make(v), "screenshot")
                .expect_err("non-finite must reject");
            assert!(err.message.unwrap().contains("finite"));
        }
    }

    #[test]
    fn scroll_pixels_convert_to_wheel_clicks() {
        // The tool's unit is pixels; the limb's is wheel detents.
        assert_eq!(scroll_clicks(300.0), (3, false));
        assert_eq!(scroll_clicks(100.0), (1, false));
        assert_eq!(scroll_clicks(50.0), (1, false));
    }

    #[test]
    fn scroll_pixels_rejects_non_finite_with_clamped_max() {
        // NaN / Infinity must not silently become "0 clicks, success".
        // The dispatcher surfaces a refusal for NaN/Infinity inputs; the
        // helper itself saturates to i32::MAX and flags quantization so a
        // future caller that bypasses the dispatcher still sees a sane
        // answer.
        assert_eq!(scroll_clicks(f64::NAN), (i32::MAX, true));
        assert_eq!(scroll_clicks(f64::INFINITY), (i32::MAX, true));
        assert_eq!(scroll_clicks(f64::NEG_INFINITY), (i32::MAX, true));
    }

    #[test]
    fn sub_detent_scroll_clamps_to_one_click_and_flags_it() {
        // A 5px request must not become 0 clicks reported as a successful
        // scroll; it moves one detent and the caller says so.
        assert_eq!(scroll_clicks(5.0), (1, true));
        assert_eq!(scroll_clicks(49.0), (1, true));
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
            })
        }
        fn capabilities(&self) -> VisionCapabilities {
            VisionCapabilities {
                image_understanding: false,
                ocr: true,
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
            .screenshot_output(
                true,
                ShotSpace::FullScreen,
                "Ymd4".into(),
                100,
                50,
                "png".into(),
            )
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
        // A region crop: normalized coords would map onto the whole display,
        // not the crop, so the guide must be absent.
        let out = tool
            .screenshot_output(
                false,
                ShotSpace::Region,
                "Ymd4".into(),
                100,
                50,
                "png".into(),
            )
            .await;
        let data = out.data.unwrap();
        assert!(data.get("coordinate_space").is_none());
    }

    #[tokio::test]
    async fn screenshot_output_of_a_window_asks_for_window_coords_not_display_coords() {
        let tool = DesktopTool::new();
        let out = tool
            .screenshot_output(
                false,
                ShotSpace::Window {
                    window_id: 8412,
                    bounds: Some(aleph_desktop::BoundingBox {
                        x: 100.0,
                        y: 60.0,
                        w: 1200.0,
                        h: 800.0,
                    }),
                },
                "Ymd4".into(),
                600,
                400,
                "png".into(),
            )
            .await;
        let cs = &out.data.unwrap()["coordinate_space"];
        assert_eq!(cs["window_id"], 8412);
        assert_eq!(cs["image_width"], 600);
        assert_eq!(cs["window_bounds"]["x"], 100.0);
        // The whole point: these pixels are window-relative, so the guide must
        // send the model to the window space — replaying them as display pixels
        // is the miss this exists to prevent.
        let note = cs["note"].as_str().unwrap();
        assert!(note.contains("coord_space=\"window\""), "{note}");
    }

    #[tokio::test]
    async fn screenshot_output_attaches_ocr_when_described() {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(FixedOcrProvider));
        let bridge = Arc::new(VisionBridge::new(Arc::new(pipeline)));
        let tool = DesktopTool::new().with_vision_bridge(bridge);

        let out = tool
            .screenshot_output(
                true,
                ShotSpace::FullScreen,
                "aW1n".into(),
                10,
                10,
                "png".into(),
            )
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
            .screenshot_output(
                false,
                ShotSpace::FullScreen,
                "aW1n".into(),
                10,
                10,
                "png".into(),
            )
            .await;
        let data = out.data.unwrap();
        assert_eq!(data["image_base64"], "aW1n");
        assert!(data.get("ocr_text").is_none());
    }

    // ── Rail policy (pure) ────────────────────────────────────────────

    #[test]
    fn a_coordinate_action_with_no_target_is_refused_when_a_background_rail_exists() {
        // The user's decision for this wave: fail closed. The event would drag
        // the user's physical cursor, so it does not happen by default.
        let err = choose_rail(true, None, false, "click")
            .expect_err("an untargeted click must not silently move the user's cursor");
        assert!(err.contains("global input tap"), "{err}");
        // The refusal has to be actionable — it names every way forward.
        assert!(err.contains("`app`"), "{err}");
        assert!(err.contains("allow_global_pointer"), "{err}");
        assert!(err.contains("set_value"), "{err}");
    }

    #[test]
    fn naming_a_process_routes_to_the_background_rail() {
        assert_eq!(
            choose_rail(true, Some(4242), false, "click").unwrap(),
            Rail::Targeted(4242)
        );
    }

    #[test]
    fn the_operator_can_opt_back_into_the_intrusive_rail() {
        assert_eq!(
            choose_rail(true, None, true, "click").unwrap(),
            Rail::Global
        );
    }

    #[test]
    fn a_platform_without_a_targeted_rail_keeps_its_legacy_behavior() {
        // Windows / Linux today: there is nothing to refuse *in favour of*, so
        // the untargeted global click must still go through — byte-identical to
        // before this wave, and independent of the config knob.
        assert_eq!(
            choose_rail(false, None, false, "click").unwrap(),
            Rail::Global
        );
        assert_eq!(
            choose_rail(false, None, true, "type_text").unwrap(),
            Rail::Global
        );
        // Even a named pid cannot conjure a rail the platform does not have.
        assert_eq!(
            choose_rail(false, Some(7), false, "click").unwrap(),
            Rail::Global
        );
    }

    #[test]
    fn only_event_synthesizing_actions_pick_a_rail() {
        for a in [
            "click",
            "double_click",
            "drag",
            "hover",
            "scroll",
            "mouse_button",
            "type_text",
            "key_combo",
            "paste",
            // A held key is a synthesized keystroke like any other. While it was
            // exempt, `key_button {press_action:"click"}` delivered the same
            // keystroke `key_combo` had just been refused for — a hole in the
            // fail-closed gate rather than an honest gap in the rail.
            "key_button",
        ] {
            assert!(is_input_action(a), "{a} puts an event on a rail");
        }
        for a in [
            "screenshot",
            "ocr",
            "window_list",
            "focus_window",
            "clipboard_read",
            "set_value",
            "ax_action",
        ] {
            assert!(
                !is_input_action(a),
                "{a} must not be gated by the rail policy"
            );
        }
    }

    #[test]
    fn the_delivery_reported_is_the_rail_that_ran() {
        assert_eq!(Rail::Targeted(1).delivery(), "targeted");
        assert_eq!(Rail::Global.delivery(), "global");
    }

    // ── app → pid resolution (pure) ───────────────────────────────────

    fn app(name: &str, bundle: &str, pid: Option<u64>) -> AppInfo {
        AppInfo {
            name: name.into(),
            bundle_id: bundle.into(),
            pid,
            is_active: false,
        }
    }

    #[test]
    fn app_resolves_by_exact_name_or_bundle_id_case_insensitively() {
        let apps = [
            app("Safari", "com.apple.Safari", Some(11)),
            app("Notes", "com.apple.Notes", Some(22)),
        ];
        assert_eq!(match_running_app(&apps, "safari").unwrap().pid, Some(11));
        assert_eq!(
            match_running_app(&apps, "com.apple.notes").unwrap().pid,
            Some(22)
        );
    }

    #[test]
    fn a_unique_substring_resolves_but_an_ambiguous_one_is_handed_back() {
        let apps = [
            app("Google Chrome", "com.google.Chrome", Some(1)),
            app("Google Chrome Helper", "com.google.Chrome.helper", Some(2)),
            app("Notes", "com.apple.Notes", Some(3)),
        ];
        assert_eq!(match_running_app(&apps, "notes").unwrap().pid, Some(3));

        // Which "Chrome" the user meant is a judgement — the model's, not ours.
        let err = match_running_app(&apps, "chrome").expect_err("ambiguous must not be guessed");
        assert!(err.contains("matches 2 running apps"), "{err}");
        assert!(err.contains("Google Chrome Helper"), "{err}");

        // An exact name still wins over its own substring matches.
        assert_eq!(
            match_running_app(&apps, "Google Chrome").unwrap().pid,
            Some(1)
        );
    }

    #[test]
    fn an_app_that_is_not_running_names_the_way_forward() {
        let apps = [app("Safari", "com.apple.Safari", Some(11))];
        let err = match_running_app(&apps, "Xcode").expect_err("not running");
        assert!(err.contains("not running"), "{err}");
        assert!(err.contains("launch_app"), "{err}");
    }

    #[test]
    fn an_installed_but_dead_app_has_no_pid_to_target() {
        // list_running_apps reports pid: None for a known-but-not-running app;
        // it must not be matched, or we would target nothing.
        let apps = [app("Xcode", "com.apple.dt.Xcode", None)];
        assert!(match_running_app(&apps, "Xcode").is_err());
    }

    // ── Routing: which rail the event actually reaches ────────────────
    //
    // The policy above is pure; these prove the dispatcher honours it — that a
    // pid really lands on `click_targeted` and not on the global tap, and that a
    // platform without a background rail is left exactly as it was.

    mod routing {
        use super::*;
        use aleph_desktop::traits::{
            AutomationCapability, MediaCapability, PermissionCapability, PimCapability,
            PowerCapability, ScreenCapability, SystemCapability,
        };
        use aleph_desktop::{
            DesktopError, DesktopPlatform, OcrResult as DOcrResult, Result as DResult,
            ScreenRegion, Screenshot, WindowInfo,
        };
        use std::sync::Mutex;

        /// Every event the screen was asked to deliver, and how.
        #[derive(Default)]
        struct Calls {
            global: Vec<String>,
            targeted: Vec<(i32, String)>,
        }

        struct RailScreen {
            targeted_rail: bool,
            calls: Arc<Mutex<Calls>>,
        }

        #[async_trait::async_trait]
        impl ScreenCapability for RailScreen {
            async fn screenshot(&self, _r: Option<ScreenRegion>) -> DResult<Screenshot> {
                Err(DesktopError::NotImplemented("screenshot".into()))
            }
            async fn ocr(&self, _i: Option<&[u8]>) -> DResult<DOcrResult> {
                Err(DesktopError::NotImplemented("ocr".into()))
            }
            async fn click(&self, _x: f64, _y: f64, _b: aleph_desktop::MouseButton) -> DResult<()> {
                self.calls.lock().unwrap().global.push("click".into());
                Ok(())
            }
            async fn type_text(&self, _t: &str) -> DResult<()> {
                self.calls.lock().unwrap().global.push("type_text".into());
                Ok(())
            }
            async fn key_combo(&self, _m: &[String], _k: &str) -> DResult<()> {
                self.calls.lock().unwrap().global.push("key_combo".into());
                Ok(())
            }
            async fn scroll(&self, _d: &str, _a: i32) -> DResult<()> {
                self.calls.lock().unwrap().global.push("scroll".into());
                Ok(())
            }
            async fn window_list(&self) -> DResult<Vec<WindowInfo>> {
                Ok(vec![WindowInfo {
                    id: 900,
                    title: "Notes".into(),
                    owner: "Notes".into(),
                    pid: 733,
                    ..Default::default()
                }])
            }
            async fn focus_window(&self, _id: u64) -> DResult<()> {
                Ok(())
            }
            async fn launch_app(&self, _n: &str) -> DResult<()> {
                Ok(())
            }

            fn supports_targeted_input(&self) -> bool {
                self.targeted_rail
            }
            async fn click_targeted(
                &self,
                pid: i32,
                _x: f64,
                _y: f64,
                _b: aleph_desktop::MouseButton,
            ) -> DResult<()> {
                self.calls
                    .lock()
                    .unwrap()
                    .targeted
                    .push((pid, "click".into()));
                Ok(())
            }
            async fn key_button(
                &self,
                _keys: &[String],
                _action: aleph_desktop::PressAction,
            ) -> DResult<()> {
                self.calls.lock().unwrap().global.push("key_button".into());
                Ok(())
            }
            async fn key_button_targeted(
                &self,
                pid: i32,
                _keys: &[String],
                _action: aleph_desktop::PressAction,
            ) -> DResult<()> {
                self.calls
                    .lock()
                    .unwrap()
                    .targeted
                    .push((pid, "key_button".into()));
                Ok(())
            }
        }

        struct RailPlatform {
            screen: RailScreen,
        }

        impl DesktopPlatform for RailPlatform {
            fn platform_name(&self) -> &str {
                "rail-mock"
            }
            fn screen(&self) -> Option<&dyn ScreenCapability> {
                Some(&self.screen)
            }
            fn pim(&self) -> Option<&dyn PimCapability> {
                None
            }
            fn system(&self) -> Option<&dyn SystemCapability> {
                None
            }
            fn automation(&self) -> Option<&dyn AutomationCapability> {
                None
            }
            fn permission(&self) -> Option<&dyn PermissionCapability> {
                None
            }
            fn media(&self) -> Option<&dyn MediaCapability> {
                None
            }
            fn power(&self) -> Option<&dyn PowerCapability> {
                None
            }
        }

        fn fixture(
            targeted_rail: bool,
            allow_global_pointer: bool,
        ) -> (
            DesktopTool,
            Arc<dyn aleph_desktop::DesktopPlatform>,
            Arc<Mutex<Calls>>,
        ) {
            let calls = Arc::new(Mutex::new(Calls::default()));
            let platform: Arc<dyn aleph_desktop::DesktopPlatform> = Arc::new(RailPlatform {
                screen: RailScreen {
                    targeted_rail,
                    calls: Arc::clone(&calls),
                },
            });
            let mut tool = DesktopTool::new().with_platform(Arc::clone(&platform));
            tool = tool.with_allow_global_pointer(allow_global_pointer);
            (tool, platform, calls)
        }

        fn click(extra: serde_json::Value) -> DesktopArgs {
            let mut v = serde_json::json!({"action": "click", "x": 10.0, "y": 20.0});
            if let (Some(base), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
                for (k, val) in extra {
                    base.insert(k.clone(), val.clone());
                }
            }
            serde_json::from_value(v).expect("valid DesktopArgs")
        }

        #[tokio::test]
        async fn a_pid_lands_on_the_background_rail_and_never_touches_the_cursor() {
            let (tool, platform, calls) = fixture(true, false);
            let out = tool
                .call_via_platform(&platform, &click(serde_json::json!({"pid": 4242})))
                .await
                .unwrap()
                .expect("click is handled");
            assert!(out.success, "{:?}", out.message);
            assert_eq!(out.data.unwrap()["delivery"], "targeted");

            let calls = calls.lock().unwrap();
            assert_eq!(calls.targeted, vec![(4242, "click".to_string())]);
            assert!(
                calls.global.is_empty(),
                "the global tap must not be touched — that is the user's cursor"
            );
        }

        #[tokio::test]
        async fn a_window_id_resolves_to_its_owning_process() {
            let (tool, platform, calls) = fixture(true, false);
            let out = tool
                .call_via_platform(&platform, &click(serde_json::json!({"window_id": 900})))
                .await
                .unwrap()
                .expect("click is handled");
            assert!(out.success, "{:?}", out.message);
            assert_eq!(
                calls.lock().unwrap().targeted,
                vec![(733, "click".to_string())]
            );
        }

        #[tokio::test]
        async fn an_untargeted_click_is_refused_and_no_event_is_emitted() {
            let (tool, platform, calls) = fixture(true, false);
            let out = tool
                .call_via_platform(&platform, &click(serde_json::json!({})))
                .await
                .unwrap()
                .expect("click is handled");
            assert!(!out.success);
            assert!(
                out.message.unwrap().contains("global input tap"),
                "the refusal must say why"
            );
            let calls = calls.lock().unwrap();
            assert!(
                calls.global.is_empty() && calls.targeted.is_empty(),
                "a refusal must cost the user nothing — no event at all"
            );
        }

        #[tokio::test]
        async fn the_operator_opt_in_restores_the_intrusive_rail() {
            let (tool, platform, calls) = fixture(true, true);
            let out = tool
                .call_via_platform(&platform, &click(serde_json::json!({})))
                .await
                .unwrap()
                .expect("click is handled");
            assert!(out.success);
            assert_eq!(out.data.unwrap()["delivery"], "global");
            assert_eq!(calls.lock().unwrap().global, vec!["click".to_string()]);
        }

        #[tokio::test]
        async fn a_platform_without_a_background_rail_is_byte_identical_to_before() {
            // Windows / Linux: no targeted rail, so an untargeted click still
            // goes out on the global path with the config knob left at its
            // fail-closed default. Nothing about this platform changed.
            let (tool, platform, calls) = fixture(false, false);
            let out = tool
                .call_via_platform(&platform, &click(serde_json::json!({})))
                .await
                .unwrap()
                .expect("click is handled");
            assert!(out.success, "{:?}", out.message);
            assert_eq!(out.data.unwrap()["delivery"], "global");
            assert_eq!(calls.lock().unwrap().global, vec!["click".to_string()]);

            // And a pid cannot conjure a rail that does not exist: it still goes
            // out globally rather than erroring on a NotImplemented default.
            let (tool, platform, calls) = fixture(false, false);
            let out = tool
                .call_via_platform(&platform, &click(serde_json::json!({"pid": 4242})))
                .await
                .unwrap()
                .expect("click is handled");
            assert!(out.success);
            assert_eq!(calls.lock().unwrap().global, vec!["click".to_string()]);
        }

        fn key_button(extra: serde_json::Value) -> DesktopArgs {
            let mut v = serde_json::json!({
                "action": "key_button", "keys": ["cmd"], "press_action": "click"
            });
            if let (Some(base), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
                for (k, val) in extra {
                    base.insert(k.clone(), val.clone());
                }
            }
            serde_json::from_value(v).expect("valid DesktopArgs")
        }

        /// The hole this wave closed: `key_button` was exempt from the rail
        /// policy, so with the fail-closed default a model refused a `key_combo`
        /// could send the identical keystroke through `key_button` — straight at
        /// the user's frontmost window, and without the result even saying so.
        #[tokio::test]
        async fn an_untargeted_key_button_is_refused_like_every_other_keystroke() {
            let (tool, platform, calls) = fixture(true, false);
            let out = tool
                .call_via_platform(&platform, &key_button(serde_json::json!({})))
                .await
                .unwrap()
                .expect("key_button is handled");
            assert!(!out.success, "{:?}", out.message);
            let calls = calls.lock().unwrap();
            assert!(
                calls.global.is_empty() && calls.targeted.is_empty(),
                "a refused keystroke must not reach the keyboard"
            );
        }

        #[tokio::test]
        async fn a_targeted_key_button_rides_the_background_rail_and_says_so() {
            let (tool, platform, calls) = fixture(true, false);
            let out = tool
                .call_via_platform(&platform, &key_button(serde_json::json!({"pid": 4242})))
                .await
                .unwrap()
                .expect("key_button is handled");
            assert!(out.success, "{:?}", out.message);
            assert_eq!(out.data.unwrap()["delivery"], "targeted");
            assert_eq!(
                calls.lock().unwrap().targeted,
                vec![(4242, "key_button".to_string())]
            );
        }

        /// Windows / Linux have no background rail, so `key_button` there must
        /// behave exactly as it always did: global, unrefused, `delivery` told
        /// honestly rather than omitted.
        #[tokio::test]
        async fn key_button_on_a_platform_without_a_background_rail_is_unchanged() {
            let (tool, platform, calls) = fixture(false, false);
            let out = tool
                .call_via_platform(&platform, &key_button(serde_json::json!({})))
                .await
                .unwrap()
                .expect("key_button is handled");
            assert!(out.success, "{:?}", out.message);
            assert_eq!(out.data.unwrap()["delivery"], "global");
            assert_eq!(calls.lock().unwrap().global, vec!["key_button".to_string()]);
        }
    }
}
