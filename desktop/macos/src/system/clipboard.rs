//! Clipboard access via `NSPasteboard`.

use aleph_desktop::system_types::ClipboardContent;
use aleph_desktop::{DesktopError, Result};
use base64::{engine::general_purpose, Engine as _};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::NSString;

/// Read the current clipboard content (text + optional image).
pub fn read() -> Result<ClipboardContent> {
    let pb = NSPasteboard::generalPasteboard();

    // Read text
    // SAFETY: `pb` is the shared general pasteboard and `NSPasteboardTypeString`
    // is a framework constant; `stringForType:` reads the string for that type
    // (a nil result maps to `None` via the generated binding).
    let text = unsafe {
        pb.stringForType(NSPasteboardTypeString)
            .map(|s| s.to_string())
    };

    // Detect and read image
    let (has_image, image_base64) = read_image(&pb);

    Ok(ClipboardContent {
        text,
        has_image,
        image_base64,
    })
}

/// Write text to the clipboard, replacing existing content.
pub fn write(text: &str) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();

    let ns_str = NSString::from_str(text);
    // SAFETY: `pb` is the general pasteboard, `ns_str` a live `NSString`, and
    // `NSPasteboardTypeString` a framework constant; `setString:forType:` copies
    // the string into the pasteboard and returns whether it succeeded.
    let ok = unsafe { pb.setString_forType(&ns_str, NSPasteboardTypeString) };
    if !ok {
        return Err(DesktopError::InputFailed(
            "NSPasteboard setString:forType: failed".into(),
        ));
    }
    Ok(())
}

/// Try to read an image from the pasteboard.
/// Returns (`has_image`, `optional_base64_png`).
fn read_image(pb: &NSPasteboard) -> (bool, Option<String>) {
    let types = match pb.types() {
        Some(t) => t,
        None => return (false, None),
    };

    // Check if any image type is present
    // SAFETY: both are `NSPasteboardType` framework constant statics, valid for
    // the process lifetime; dereferencing them only reads the constant value.
    let png_type: &NSString = unsafe { NSPasteboardTypePNG };
    let tiff_type: &NSString = unsafe { NSPasteboardTypeTIFF };
    let has_png = types.iter().any(|t| *t == *png_type);
    let has_tiff = types.iter().any(|t| *t == *tiff_type);

    if !has_png && !has_tiff {
        return (false, None);
    }

    // Try PNG first (preferred — no conversion needed)
    if has_png {
        // SAFETY: `pb` is the general pasteboard and `NSPasteboardTypePNG` a
        // framework constant; `dataForType:` returns the data for that type
        // (nil maps to `None`).
        if let Some(data) = unsafe { pb.dataForType(NSPasteboardTypePNG) } {
            let bytes = data.to_vec();
            let b64 = general_purpose::STANDARD.encode(&bytes);
            return (true, Some(b64));
        }
    }

    // Fall back to TIFF → PNG conversion
    if has_tiff {
        // SAFETY: `pb` is the general pasteboard and `NSPasteboardTypeTIFF` a
        // framework constant; `dataForType:` returns the data for that type
        // (nil maps to `None`).
        if let Some(data) = unsafe { pb.dataForType(NSPasteboardTypeTIFF) } {
            let bytes = data.to_vec();
            if let Some(b64) = tiff_to_png_base64(&bytes) {
                return (true, Some(b64));
            }
            // Image present but conversion failed
            return (true, None);
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

    #[test]
    fn test_clipboard_roundtrip() {
        let test_text = "aleph-test-clipboard-native-12345";
        write(test_text).unwrap();
        let content = read().unwrap();
        assert_eq!(content.text.as_deref(), Some(test_text));
    }
}
