/// arboard-based clipboard manager implementation
use super::{ClipboardManager, ImageData, ImageFormat};
use crate::error::{AetherError, Result};
use arboard::Clipboard;
use std::borrow::Cow;

/// Clipboard manager using the arboard crate
///
/// Provides cross-platform clipboard access using arboard.
/// Each operation creates a new Clipboard instance to avoid
/// lifetime issues with the internal clipboard connection.
pub struct ArboardManager;

impl ArboardManager {
    /// Create a new arboard clipboard manager
    pub fn new() -> Self {
        Self
    }

    /// Detect image format from raw bytes using magic bytes
    fn detect_format(data: &[u8]) -> Option<ImageFormat> {
        if data.len() < 4 {
            return None;
        }

        // PNG magic bytes: 89 50 4E 47
        if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            return Some(ImageFormat::Png);
        }

        // JPEG magic bytes: FF D8 FF
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(ImageFormat::Jpeg);
        }

        // GIF magic bytes: "GIF87a" or "GIF89a"
        if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            return Some(ImageFormat::Gif);
        }

        None
    }
}

impl Default for ArboardManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardManager for ArboardManager {
    fn read_text(&self) -> Result<String> {
        let mut clipboard = Clipboard::new()
            .map_err(|e| AetherError::clipboard(format!("Failed to access clipboard: {}", e)))?;

        clipboard
            .get_text()
            .map_err(|e| AetherError::clipboard(format!("Failed to read clipboard text: {}", e)))
    }

    fn write_text(&self, content: &str) -> Result<()> {
        let mut clipboard = Clipboard::new()
            .map_err(|e| AetherError::clipboard(format!("Failed to access clipboard: {}", e)))?;

        clipboard
            .set_text(content)
            .map_err(|e| AetherError::clipboard(format!("Failed to write clipboard text: {}", e)))
    }

    fn has_image(&self) -> bool {
        let mut clipboard = match Clipboard::new() {
            Ok(cb) => cb,
            Err(_) => return false,
        };

        // Try to get image - if successful, there's an image
        clipboard.get_image().is_ok()
    }

    fn read_image(&self) -> Result<Option<ImageData>> {
        let mut clipboard = Clipboard::new()
            .map_err(|e| AetherError::clipboard(format!("Failed to access clipboard: {}", e)))?;

        match clipboard.get_image() {
            Ok(img) => {
                // Get raw bytes from arboard ImageData
                let bytes = img.bytes.into_owned();

                // Detect format from magic bytes
                let format = Self::detect_format(&bytes).ok_or_else(|| {
                    AetherError::clipboard(
                        "Unsupported image format. Please use PNG, JPEG, or GIF.".to_string(),
                    )
                })?;

                Ok(Some(ImageData::new(bytes, format)))
            }
            Err(arboard::Error::ContentNotAvailable) => {
                // No image in clipboard
                Ok(None)
            }
            Err(e) => Err(AetherError::clipboard(format!(
                "Failed to read clipboard image: {}",
                e
            ))),
        }
    }

    fn write_image(&self, image: ImageData) -> Result<()> {
        let mut clipboard = Clipboard::new()
            .map_err(|e| AetherError::clipboard(format!("Failed to access clipboard: {}", e)))?;

        // Parse image to get dimensions
        let img_reader = image::io::Reader::new(std::io::Cursor::new(&image.data))
            .with_guessed_format()
            .map_err(|e| {
                AetherError::clipboard(format!("Failed to detect image format: {}", e))
            })?;

        let decoded = img_reader.decode().map_err(|e| {
            AetherError::clipboard(format!("Failed to decode image: {}", e))
        })?;

        let width = decoded.width() as usize;
        let height = decoded.height() as usize;

        // Convert to RGBA for arboard (arboard expects RGBA format)
        let rgba_image = decoded.to_rgba8();
        let rgba_bytes: Vec<u8> = rgba_image.into_raw();

        // Create arboard ImageData
        let arboard_img = arboard::ImageData {
            width,
            height,
            bytes: Cow::Owned(rgba_bytes),
        };

        clipboard
            .set_image(arboard_img)
            .map_err(|e| AetherError::clipboard(format!("Failed to write clipboard image: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_instance() {
        let _manager = ArboardManager::new();
        // Should not panic
    }

    #[test]
    fn test_default() {
        let _manager = ArboardManager;
        // Should not panic
    }

    #[test]
    fn test_read_write_cycle() {
        let manager = ArboardManager::new();

        let test_content = "arboard test content";
        manager.write_text(test_content).unwrap();

        let read_content = manager.read_text().unwrap();
        assert_eq!(read_content, test_content);
    }

    #[test]
    fn test_empty_string() {
        let manager = ArboardManager::new();

        manager.write_text("").unwrap();
        let content = manager.read_text().unwrap();
        assert_eq!(content, "");
    }

    #[test]
    fn test_multiline_text() {
        let manager = ArboardManager::new();

        let multiline = "Line 1\nLine 2\nLine 3";
        manager.write_text(multiline).unwrap();

        let content = manager.read_text().unwrap();
        assert_eq!(content, multiline);
    }

    #[test]
    fn test_detect_format_png() {
        let png_magic = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(
            ArboardManager::detect_format(&png_magic),
            Some(ImageFormat::Png)
        );
    }

    #[test]
    fn test_detect_format_jpeg() {
        let jpeg_magic = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(
            ArboardManager::detect_format(&jpeg_magic),
            Some(ImageFormat::Jpeg)
        );
    }

    #[test]
    fn test_detect_format_gif() {
        let gif_magic = b"GIF89a";
        assert_eq!(
            ArboardManager::detect_format(gif_magic),
            Some(ImageFormat::Gif)
        );
    }

    #[test]
    fn test_detect_format_unsupported() {
        let unknown = vec![0x00, 0x00, 0x00, 0x00];
        assert_eq!(ArboardManager::detect_format(&unknown), None);
    }

    #[test]
    fn test_detect_format_too_short() {
        let too_short = vec![0x89, 0x50];
        assert_eq!(ArboardManager::detect_format(&too_short), None);
    }
}
