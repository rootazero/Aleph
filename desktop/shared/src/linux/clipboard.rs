//! Linux clipboard: the single implementation.
//!
//! There used to be two — `action::input`'s X11-only `xclip`/`xsel` pair (used
//! by the `desktop` tool through `ScreenCapability`) and `desktop-linux`'s
//! Wayland-aware one (used by the `system` tool through `SystemCapability`).
//! Both picked their tool by *spawning* candidates in a fixed order and falling
//! through only when the spawn itself failed, so:
//!
//! * On an X11 box that happens to have `wl-clipboard` installed, `wl-copy`
//!   spawns fine, fails to connect to a Wayland display, and the write is
//!   silently lost — the caller is told it succeeded.
//! * A read never checked the exit status at all, so the same situation
//!   returned an empty string as if the clipboard were empty.
//!
//! This module fixes both by choosing the candidate order from
//! [`super::session`] (the actual session type, not spawn luck) and by checking
//! every exit status.
//!
//! # Read vs write asymmetry (deliberate)
//!
//! A **write** that did not happen must be an error: reporting success for a
//! copy the user cannot paste is the worst outcome. So a write fails when every
//! installed candidate fails.
//!
//! A **read** where every installed candidate exits non-zero returns
//! `Ok("")`. `xclip -o` and friends exit non-zero for an *empty* clipboard just
//! as they do for a broken connection, and the two are indistinguishable from
//! the outside; "the clipboard is empty" is overwhelmingly the common case and
//! the harmless reading. Only "no clipboard tool is installed at all" is an
//! error, and it names what to install.

use std::io::Write as _;
use std::process::{Command, Stdio};

use base64::{engine::general_purpose, Engine as _};

use super::session::{missing_tool_error, session, tools, SessionKind, ToolBox};
use crate::error::Result;
use crate::system_types::ClipboardContent;

/// Clipboard tools in preference order for the given session.
///
/// Wayland leads with `wl-clipboard` and still lists the X11 tools, because a
/// Wayland session usually runs XWayland and an X11 tool can be the only one
/// installed. X11 is the mirror image. `Unknown` uses the X11 order: a headless
/// or unusual session is far more likely to have an X server reachable through
/// `DISPLAY` than a Wayland compositor with no `WAYLAND_DISPLAY`.
#[must_use]
pub const fn read_order(kind: SessionKind) -> &'static [&'static str] {
    match kind {
        SessionKind::Wayland => &["wl-paste", "xclip", "xsel"],
        SessionKind::X11 | SessionKind::Unknown => &["xclip", "xsel", "wl-paste"],
    }
}

/// Clipboard write tools in preference order — the mirror of [`read_order`].
#[must_use]
pub const fn write_order(kind: SessionKind) -> &'static [&'static str] {
    match kind {
        SessionKind::Wayland => &["wl-copy", "xclip", "xsel"],
        SessionKind::X11 | SessionKind::Unknown => &["xclip", "xsel", "wl-copy"],
    }
}

/// Argv for reading the clipboard with `tool`.
#[must_use]
pub fn read_args(tool: &str) -> Vec<String> {
    match tool {
        "wl-paste" => vec!["--no-newline".into()],
        "xclip" => vec!["-selection".into(), "clipboard".into(), "-o".into()],
        "xsel" => vec!["--clipboard".into(), "--output".into()],
        _ => Vec::new(),
    }
}

/// Argv for writing the clipboard with `tool` (text arrives on stdin).
#[must_use]
pub fn write_args(tool: &str) -> Vec<String> {
    match tool {
        // `--` stops option parsing; without it `wl-copy` would treat text
        // beginning with `-` as flags. `-n` keeps it from appending a newline,
        // matching the `--no-newline` read side so a round-trip is lossless.
        "wl-copy" => vec!["-n".into(), "--".into()],
        "xclip" => vec!["-selection".into(), "clipboard".into()],
        "xsel" => vec!["--clipboard".into(), "--input".into()],
        _ => Vec::new(),
    }
}

/// Argv for listing the MIME types currently offered on the clipboard.
#[must_use]
pub fn list_targets_args(tool: &str) -> Vec<String> {
    match tool {
        "wl-paste" => vec!["--list-types".into()],
        "xclip" => vec![
            "-selection".into(),
            "clipboard".into(),
            "-t".into(),
            "TARGETS".into(),
            "-o".into(),
        ],
        _ => Vec::new(),
    }
}

/// Argv for reading one specific MIME type off the clipboard.
#[must_use]
pub fn read_typed_args(tool: &str, mime: &str) -> Vec<String> {
    match tool {
        "wl-paste" => vec!["--type".into(), mime.to_string()],
        "xclip" => vec![
            "-selection".into(),
            "clipboard".into(),
            "-t".into(),
            mime.to_string(),
            "-o".into(),
        ],
        _ => Vec::new(),
    }
}

// ── Runtime ──────────────────────────────────────────────────────────────────

/// Read clipboard text.
///
/// # Errors
///
/// [`crate::DesktopError::NotAvailable`] when no clipboard tool is installed.
pub fn read_text() -> Result<String> {
    read_text_with(session().kind, tools())
}

fn read_text_with(kind: SessionKind, tb: &ToolBox) -> Result<String> {
    let order = read_order(kind);
    let mut any_installed = false;

    for tool in order {
        if !tb.has(tool) {
            continue;
        }
        any_installed = true;
        if let Ok(out) = Command::new(tool).args(read_args(tool)).output() {
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }

    if any_installed {
        // Every installed tool refused. The dominant cause is an empty
        // clipboard — see the module doc for why this is Ok, not Err.
        Ok(String::new())
    } else {
        Err(missing_tool_error("Reading the clipboard", order))
    }
}

/// Write text to the clipboard, replacing its contents.
///
/// # Errors
///
/// [`crate::DesktopError::NotAvailable`] when no clipboard tool is installed,
/// [`crate::DesktopError::InputFailed`] when every installed tool failed —
/// never silent success.
pub fn write_text(text: &str) -> Result<()> {
    write_text_with(session().kind, tools(), text)
}

fn write_text_with(kind: SessionKind, tb: &ToolBox, text: &str) -> Result<()> {
    let order = write_order(kind);
    let mut failures: Vec<String> = Vec::new();

    for tool in order {
        if !tb.has(tool) {
            continue;
        }
        match write_once(tool, &write_args(tool), text.as_bytes()) {
            Ok(()) => return Ok(()),
            Err(reason) => failures.push(format!("{tool}: {reason}")),
        }
    }

    if failures.is_empty() {
        return Err(missing_tool_error("Writing the clipboard", order));
    }
    Err(crate::error::DesktopError::InputFailed(format!(
        "Clipboard write failed with every installed tool ({}). \
         A Wayland tool cannot serve an X11 session (or vice versa) — install the one \
         matching your session type.",
        failures.join("; ")
    )))
}

/// Run one clipboard-write candidate to completion, feeding `data` on stdin.
///
/// `xclip`, `xsel` and `wl-copy` all fork a background process to own the
/// selection and then exit, so waiting here does not hang.
fn write_once(tool: &str, args: &[String], data: &[u8]) -> std::result::Result<(), String> {
    let mut child = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;

    // Take stdin so it is *dropped* (closed) before the wait: a tool that keeps
    // reading until EOF would otherwise deadlock against our wait.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(data)
            .map_err(|e| format!("write to stdin failed: {e}"))?;
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait failed: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = stderr.trim().lines().last().unwrap_or("non-zero exit");
    Err(detail.to_string())
}

/// Read the clipboard as text plus an optional image (base64 PNG).
///
/// Text failure is a hard error; the image half is best-effort — a clipboard
/// with no image on it is the normal case, not a fault.
///
/// # Errors
///
/// Propagates [`read_text`].
pub fn read_content() -> Result<ClipboardContent> {
    let text = read_text()?;
    let image_base64 = read_image_png_base64();
    Ok(ClipboardContent {
        text: Some(text),
        has_image: image_base64.is_some(),
        image_base64,
    })
}

/// Best-effort clipboard image read, normalized to a base64-encoded PNG.
#[must_use]
pub fn read_image_png_base64() -> Option<String> {
    let tb = tools();
    // Only the two tools that can enumerate and fetch a typed target; `xsel`
    // is text-only.
    for tool in read_order(session().kind) {
        if *tool == "xsel" || !tb.has(tool) {
            continue;
        }
        if let Some(png) = read_image_with(tool) {
            return Some(png);
        }
    }
    None
}

fn read_image_with(tool: &str) -> Option<String> {
    let targets = Command::new(tool)
        .args(list_targets_args(tool))
        .output()
        .ok()?;
    if !targets.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&targets.stdout);
    let mime = pick_image_target(&listing)?.to_string();

    let out = Command::new(tool)
        .args(read_typed_args(tool, &mime))
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    to_png_base64(&out.stdout, mime == "image/png")
}

/// Pick the best image MIME type from a newline-separated target listing.
///
/// Prefers `image/png` (no re-encode needed), then any other `image/*` type.
#[must_use]
pub fn pick_image_target(listing: &str) -> Option<&str> {
    let mut first_image: Option<&str> = None;
    for line in listing.lines() {
        let mime = line.trim();
        if mime == "image/png" {
            return Some(mime);
        }
        if first_image.is_none() && mime.starts_with("image/") {
            first_image = Some(mime);
        }
    }
    first_image
}

/// Encode raw image bytes as a base64 PNG, transcoding non-PNG via the `image`
/// crate. Mirrors the macOS TIFF→PNG fallback.
#[must_use]
pub fn to_png_base64(bytes: &[u8], is_png: bool) -> Option<String> {
    use image::ImageReader;
    use std::io::Cursor;

    if is_png {
        return Some(general_purpose::STANDARD.encode(bytes));
    }

    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let mut png_buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_buf), image::ImageFormat::Png)
        .ok()?;
    Some(general_purpose::STANDARD.encode(&png_buf))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_leads_with_wl_clipboard() {
        assert_eq!(read_order(SessionKind::Wayland)[0], "wl-paste");
        assert_eq!(write_order(SessionKind::Wayland)[0], "wl-copy");
    }

    #[test]
    fn x11_leads_with_x11_tools() {
        assert_eq!(read_order(SessionKind::X11)[0], "xclip");
        assert_eq!(write_order(SessionKind::X11)[0], "xclip");
        // Unknown behaves like X11 rather than dropping to no tools at all.
        assert_eq!(read_order(SessionKind::Unknown)[0], "xclip");
    }

    #[test]
    fn every_order_lists_both_worlds() {
        // The cross-world tool must remain a candidate: XWayland sessions run
        // X11 tools, and some X11 setups only ship wl-clipboard.
        for kind in [SessionKind::X11, SessionKind::Wayland, SessionKind::Unknown] {
            assert!(read_order(kind).contains(&"wl-paste"), "{kind:?}");
            assert!(read_order(kind).contains(&"xclip"), "{kind:?}");
            assert!(write_order(kind).contains(&"wl-copy"), "{kind:?}");
            assert!(write_order(kind).contains(&"xclip"), "{kind:?}");
        }
    }

    #[test]
    fn write_args_stop_option_parsing_for_wl_copy() {
        // Without `--`, text starting with `-` would be parsed as flags.
        assert!(write_args("wl-copy").contains(&"--".to_string()));
    }

    #[test]
    fn read_and_write_args_round_trip_without_a_stray_newline() {
        // -n on write and --no-newline on read must agree, or a copy/paste
        // round-trip silently grows a trailing newline each time.
        assert!(write_args("wl-copy").contains(&"-n".to_string()));
        assert!(read_args("wl-paste").contains(&"--no-newline".to_string()));
    }

    #[test]
    fn unknown_tool_yields_empty_argv() {
        assert!(read_args("pbpaste").is_empty());
        assert!(write_args("pbcopy").is_empty());
        assert!(list_targets_args("xsel").is_empty());
    }

    #[test]
    fn no_tool_installed_is_an_error_not_an_empty_string() {
        let empty = ToolBox::from_names(&[]);
        let err = read_text_with(SessionKind::X11, &empty).unwrap_err();
        assert!(err.to_string().contains("xclip"), "{err}");

        let err = write_text_with(SessionKind::X11, &empty, "hi").unwrap_err();
        assert!(err.to_string().contains("xclip"), "{err}");
    }

    #[test]
    fn write_reports_failure_rather_than_silent_success() {
        // `wl-copy` is "installed" but the binary does not exist, so every
        // candidate fails. The old code returned Ok here.
        let tb = ToolBox::from_names(&["wl-copy"]);
        let err = write_text_with(SessionKind::Wayland, &tb, "hello").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("wl-copy"), "must name what it tried: {msg}");
    }

    #[test]
    fn prefers_png_over_other_image_types() {
        let listing = "TARGETS\nimage/bmp\nimage/png\ntext/plain\n";
        assert_eq!(pick_image_target(listing), Some("image/png"));
    }

    #[test]
    fn falls_back_to_first_non_png_image() {
        let listing = "TARGETS\ntext/html\nimage/jpeg\nimage/bmp\n";
        assert_eq!(pick_image_target(listing), Some("image/jpeg"));
    }

    #[test]
    fn returns_none_when_no_image_offered() {
        let listing = "TARGETS\nUTF8_STRING\ntext/plain\ntext/html\n";
        assert_eq!(pick_image_target(listing), None);
    }

    #[test]
    fn handles_whitespace_and_empty_lines() {
        assert_eq!(pick_image_target("  image/png  \n\n"), Some("image/png"));
    }

    #[test]
    fn png_bytes_pass_through_as_base64() {
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52,
        ];
        let b64 = to_png_base64(png, true).unwrap();
        assert_eq!(general_purpose::STANDARD.decode(b64).unwrap(), png);
    }

    #[test]
    fn invalid_image_bytes_return_none() {
        assert!(to_png_base64(b"not an image", false).is_none());
    }
}
