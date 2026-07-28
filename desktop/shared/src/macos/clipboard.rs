//! Clipboard access on macOS via `NSPasteboard` — the single source of truth.
//!
//! There were two of these as well, and the difference was not cosmetic. The
//! copy in `action::input` **discarded** `setString:forType:`'s return value and
//! returned `Ok(())` unconditionally, so a write that the pasteboard rejected —
//! another process holding it, a sandbox denial — was reported to the model as a
//! success. The next step, a paste, then produced whatever had been on the
//! clipboard before, which is the confusing kind of wrong: something *does* get
//! pasted, just not the thing that was copied.
//!
//! The copy in the macOS limb's `system::clipboard` checked the return value and
//! also handled images. This module is that one, moved to the side of the
//! dependency edge both callers can reach.

use base64::{engine::general_purpose, Engine as _};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::NSString;

use crate::error::{DesktopError, Result};
use crate::system_types::ClipboardContent;

/// Read the clipboard's text, or the empty string when it holds no text.
///
/// "No text on the pasteboard" is genuinely not an error — an image-only
/// clipboard is a normal state — so it reads back as empty rather than failing.
pub fn read_text() -> Result<String> {
    let pb = NSPasteboard::generalPasteboard();
    // SAFETY: `NSPasteboardTypeString` is a framework constant valid for the
    // process lifetime; `stringForType:` returns nil (→ `None`) when absent.
    let text = unsafe { pb.stringForType(NSPasteboardTypeString) };
    Ok(text.map(|s| s.to_string()).unwrap_or_default())
}

/// Read the clipboard's text and, if present, its image (as base64 PNG).
pub fn read() -> Result<ClipboardContent> {
    let pb = NSPasteboard::generalPasteboard();
    // SAFETY: as in `read_text`.
    let text = unsafe {
        pb.stringForType(NSPasteboardTypeString)
            .map(|s| s.to_string())
    };
    let (has_image, image_base64) = read_image(&pb);
    Ok(ClipboardContent {
        text,
        has_image,
        image_base64,
    })
}

/// Replace the clipboard's contents with `text`.
///
/// # Errors
///
/// [`DesktopError::InputFailed`] when the pasteboard refuses the write. This is
/// the whole reason this function exists in one place: the other implementation
/// threw that answer away.
pub fn write_text(text: &str) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();

    let ns_str = NSString::from_str(text);
    // SAFETY: `pb` is the general pasteboard, `ns_str` a live `NSString`, and
    // `NSPasteboardTypeString` a framework constant; the call copies the string
    // into the pasteboard and reports whether it took.
    let ok = unsafe { pb.setString_forType(&ns_str, NSPasteboardTypeString) };
    if !ok {
        return Err(DesktopError::InputFailed(
            "clipboard write refused by NSPasteboard — another process may be holding the \
             pasteboard. Nothing was copied; the previous clipboard contents are still there."
                .into(),
        ));
    }
    tracing::info!("Clipboard write performed");
    Ok(())
}

/// Try to read an image from the pasteboard.
/// Returns (`has_image`, `optional_base64_png`).
fn read_image(pb: &NSPasteboard) -> (bool, Option<String>) {
    let Some(types) = pb.types() else {
        return (false, None);
    };

    // SAFETY: both are `NSPasteboardType` framework constant statics, valid for
    // the process lifetime; dereferencing them only reads the constant value.
    let png_type: &NSString = unsafe { NSPasteboardTypePNG };
    let tiff_type: &NSString = unsafe { NSPasteboardTypeTIFF };
    let has_png = types.iter().any(|t| *t == *png_type);
    let has_tiff = types.iter().any(|t| *t == *tiff_type);

    if !has_png && !has_tiff {
        return (false, None);
    }

    // PNG first — no conversion needed.
    if has_png {
        // SAFETY: framework constant; `dataForType:` yields nil (→ `None`) when
        // the type is not actually present.
        if let Some(data) = unsafe { pb.dataForType(NSPasteboardTypePNG) } {
            return (true, Some(general_purpose::STANDARD.encode(data.to_vec())));
        }
    }

    if has_tiff {
        // SAFETY: framework constant, as above.
        if let Some(data) = unsafe { pb.dataForType(NSPasteboardTypeTIFF) } {
            // An image that is present but unconvertible is still present:
            // `true` with no payload says exactly that.
            return (true, tiff_to_png_base64(&data.to_vec()));
        }
    }

    (false, None)
}

/// Convert TIFF bytes to base64-encoded PNG.
fn tiff_to_png_base64(tiff_bytes: &[u8]) -> Option<String> {
    use image::ImageReader;
    use std::io::Cursor;

    let reader = ImageReader::new(Cursor::new(tiff_bytes))
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?;
    let mut png_buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_buf), image::ImageFormat::Png)
        .ok()?;
    Some(general_purpose::STANDARD.encode(&png_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, not two, on purpose: the general pasteboard is a single global
    /// resource and two tests writing to it in parallel read each other's
    /// probe. Both assertions belong to the same round trip anyway.
    #[test]
    fn text_round_trips_and_both_readers_agree() {
        let probe = "aleph-test-clipboard-single-source-12345";
        write_text(probe).unwrap();

        let plain = read_text().unwrap();
        let rich = read().unwrap();

        assert_eq!(plain, probe);
        // The two entry points read the same pasteboard — the property that
        // stopped being true once there were two implementations of it.
        assert_eq!(rich.text.as_deref(), Some(probe));
    }
}
