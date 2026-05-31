//! Linux clipboard access via wl-clipboard (Wayland) and xclip / xsel (X11).
//!
//! Mirrors the macOS `system/clipboard.rs` contract: text plus an optional
//! image returned as a base64-encoded PNG. The image half closes a parity gap
//! where Linux previously dropped copied images (`has_image` was hardcoded to
//! `false`), while macOS surfaced them and the desktop tool consumer was
//! already prepared to forward them to vision-capable agents.
//!
//! Shells out to the standard freedesktop CLI tools instead of taking an X11 /
//! Wayland client dependency (R3: core minimalism). Image bytes are normalized
//! to PNG with the `image` crate that is already compiled for camera frames.

use std::io::Write;
use std::process::{Command, Stdio};

use aleph_desktop::system_types::ClipboardContent;
use aleph_desktop::{DesktopError, Result};
use base64::{engine::general_purpose, Engine as _};

/// Read the current clipboard content (text + optional image as base64 PNG).
///
/// Text read failure (no clipboard tool installed) is a hard error, matching
/// the prior behavior. Image read is best-effort: any failure leaves
/// `has_image = false` rather than failing the whole read.
pub(crate) fn read() -> Result<ClipboardContent> {
    let text = read_text()?;
    let image_base64 = read_image_png_base64();
    Ok(ClipboardContent {
        text: Some(text),
        has_image: image_base64.is_some(),
        image_base64,
    })
}

/// Write text to the clipboard, replacing existing content.
pub(crate) fn write(text: &str) -> Result<()> {
    // Try wl-copy first (Wayland), then xclip/xsel (X11).
    let mut child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .spawn()
        .or_else(|_| {
            Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(Stdio::piped())
                .spawn()
        })
        .or_else(|_| {
            Command::new("xsel")
                .args(["--clipboard", "--input"])
                .stdin(Stdio::piped())
                .spawn()
        })
        .map_err(|e| {
            DesktopError::InputFailed(format!(
                "Failed to write clipboard (install wl-clipboard, xclip, or xsel): {e}"
            ))
        })?;

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| DesktopError::InputFailed(format!("Clipboard write failed: {e}")))?;
    }

    child
        .wait()
        .map_err(|e| DesktopError::InputFailed(format!("Clipboard process failed: {e}")))?;

    Ok(())
}

/// Read clipboard text. Tries wl-paste (Wayland), then xclip/xsel (X11).
fn read_text() -> Result<String> {
    let output = Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .or_else(|_| {
            Command::new("xclip")
                .args(["-selection", "clipboard", "-o"])
                .output()
        })
        .or_else(|_| {
            Command::new("xsel")
                .args(["--clipboard", "--output"])
                .output()
        })
        .map_err(|e| {
            DesktopError::InputFailed(format!(
                "Failed to read clipboard (install wl-clipboard, xclip, or xsel): {e}"
            ))
        })?;

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Best-effort clipboard image read, normalized to a base64-encoded PNG.
///
/// Tries Wayland (`wl-paste`) then X11 (`xclip`); returns `None` when no image
/// is on the clipboard, no tool is available, or decoding fails.
fn read_image_png_base64() -> Option<String> {
    read_image_wayland().or_else(read_image_x11)
}

/// Wayland: enumerate offered MIME types, pick the best image target, read it.
fn read_image_wayland() -> Option<String> {
    let types = Command::new("wl-paste").arg("--list-types").output().ok()?;
    if !types.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&types.stdout);
    let mime = pick_image_target(&listing)?;

    let output = Command::new("wl-paste")
        .args(["--type", mime])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    to_png_base64(&output.stdout, mime == "image/png")
}

/// X11: query clipboard TARGETS, pick the best image target, read it.
fn read_image_x11() -> Option<String> {
    let targets = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
        .output()
        .ok()?;
    if !targets.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&targets.stdout);
    let mime = pick_image_target(&listing)?;

    let output = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", mime, "-o"])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    to_png_base64(&output.stdout, mime == "image/png")
}

/// Pick the best image MIME type from a newline-separated target listing.
///
/// Prefers `image/png` (no re-encode needed), then any other `image/*` type.
/// Pure function — host-testable without a display server.
fn pick_image_target(listing: &str) -> Option<&str> {
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
fn to_png_base64(bytes: &[u8], is_png: bool) -> Option<String> {
    if is_png {
        return Some(general_purpose::STANDARD.encode(bytes));
    }
    use image::ImageReader;
    use std::io::Cursor;

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

#[cfg(test)]
mod tests {
    use super::*;

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
        let listing = "  image/png  \n\n";
        assert_eq!(pick_image_target(listing), Some("image/png"));
    }

    #[test]
    fn png_bytes_pass_through_as_base64() {
        // 1x1 transparent PNG.
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
