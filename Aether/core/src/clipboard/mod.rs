/// Clipboard management trait and implementations
///
/// This module provides a trait-based abstraction for clipboard operations,
/// with an implementation using the arboard crate.
mod arboard_manager;

use crate::error::Result;
pub use arboard_manager::ArboardManager;

/// Image format enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG format (lossless compression)
    Png,
    /// JPEG format (lossy compression)
    Jpeg,
    /// GIF format (supports animation)
    Gif,
}

/// Image data structure for clipboard operations
#[derive(Debug, Clone)]
pub struct ImageData {
    /// Raw image bytes
    pub data: Vec<u8>,
    /// Image format
    pub format: ImageFormat,
}

impl ImageData {
    /// Create a new ImageData instance
    pub fn new(data: Vec<u8>, format: ImageFormat) -> Self {
        Self { data, format }
    }

    /// Get the size of the image data in bytes
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }

    /// Get the size of the image data in megabytes
    pub fn size_mb(&self) -> f64 {
        self.data.len() as f64 / (1024.0 * 1024.0)
    }

    /// Convert image to Base64 data URI format for API requests
    ///
    /// Returns a string in the format: "data:image/<format>;base64,<encoded_data>"
    pub fn to_base64(&self) -> String {
        use base64::{engine::general_purpose, Engine as _};

        let mime_type = match self.format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
        };

        let encoded = general_purpose::STANDARD.encode(&self.data);
        format!("data:{};base64,{}", mime_type, encoded)
    }

    /// Parse image from Base64 data URI
    ///
    /// Accepts strings in the format: "data:image/<format>;base64,<encoded_data>"
    /// Returns `Ok(ImageData)` if successfully decoded.
    /// Returns `Err` if the format is invalid or decoding fails.
    pub fn from_base64(data_uri: &str) -> Result<Self> {
        use base64::{engine::general_purpose, Engine as _};

        // Parse data URI format: data:image/png;base64,iVBORw0KGgo...
        let parts: Vec<&str> = data_uri.split(',').collect();
        if parts.len() != 2 {
            return Err(crate::error::AetherError::clipboard(
                "Invalid Base64 data URI format".to_string(),
            ));
        }

        let header = parts[0];
        let base64_data = parts[1];

        // Extract MIME type
        let format = if header.contains("image/png") {
            ImageFormat::Png
        } else if header.contains("image/jpeg") {
            ImageFormat::Jpeg
        } else if header.contains("image/gif") {
            ImageFormat::Gif
        } else {
            return Err(crate::error::AetherError::clipboard(format!(
                "Unsupported image MIME type in data URI: {}",
                header
            )));
        };

        // Decode Base64
        let decoded = general_purpose::STANDARD
            .decode(base64_data)
            .map_err(|e| {
                crate::error::AetherError::clipboard(format!("Base64 decoding failed: {}", e))
            })?;

        Ok(Self::new(decoded, format))
    }
}

/// Trait for clipboard management operations
///
/// This trait allows for swappable clipboard implementations
/// and enables easy mocking in tests.
pub trait ClipboardManager: Send + Sync {
    /// Read plain text from clipboard
    fn read_text(&self) -> Result<String>;

    /// Write plain text to clipboard
    fn write_text(&self, content: &str) -> Result<()>;

    /// Check if clipboard has image content
    fn has_image(&self) -> bool {
        false // Default implementation for backwards compatibility
    }

    /// Read image from clipboard
    ///
    /// Returns `Ok(None)` if clipboard contains no image data.
    /// Returns `Ok(Some(ImageData))` if image is successfully read.
    /// Returns `Err` if an error occurs during clipboard access.
    fn read_image(&self) -> Result<Option<ImageData>> {
        Ok(None) // Default implementation for backwards compatibility
    }

    /// Write image to clipboard
    ///
    /// Returns `Ok(())` if image is successfully written.
    /// Returns `Err` if an error occurs during clipboard access.
    fn write_image(&self, _image: ImageData) -> Result<()> {
        Ok(()) // Default implementation for backwards compatibility
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_data_creation() {
        let data = vec![0x89, 0x50, 0x4E, 0x47]; // PNG magic bytes
        let image = ImageData::new(data.clone(), ImageFormat::Png);

        assert_eq!(image.data, data);
        assert_eq!(image.format, ImageFormat::Png);
    }

    #[test]
    fn test_image_data_size_bytes() {
        let data = vec![0u8; 1024]; // 1KB
        let image = ImageData::new(data, ImageFormat::Jpeg);

        assert_eq!(image.size_bytes(), 1024);
    }

    #[test]
    fn test_image_data_size_mb() {
        let data = vec![0u8; 1024 * 1024 * 5]; // 5MB
        let image = ImageData::new(data, ImageFormat::Png);

        assert!((image.size_mb() - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_image_format_equality() {
        assert_eq!(ImageFormat::Png, ImageFormat::Png);
        assert_ne!(ImageFormat::Png, ImageFormat::Jpeg);
        assert_ne!(ImageFormat::Jpeg, ImageFormat::Gif);
    }

    #[test]
    fn test_image_to_base64_png() {
        let data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let image = ImageData::new(data, ImageFormat::Png);

        let base64 = image.to_base64();
        assert!(base64.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_image_to_base64_jpeg() {
        let data = vec![0xFF, 0xD8, 0xFF, 0xE0];
        let image = ImageData::new(data, ImageFormat::Jpeg);

        let base64 = image.to_base64();
        assert!(base64.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn test_image_to_base64_gif() {
        let data = b"GIF89a".to_vec();
        let image = ImageData::new(data, ImageFormat::Gif);

        let base64 = image.to_base64();
        assert!(base64.starts_with("data:image/gif;base64,"));
    }

    #[test]
    fn test_image_from_base64_roundtrip() {
        let original_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let original = ImageData::new(original_data.clone(), ImageFormat::Png);

        // Encode to base64
        let base64_uri = original.to_base64();

        // Decode back
        let decoded = ImageData::from_base64(&base64_uri).unwrap();

        assert_eq!(decoded.data, original_data);
        assert_eq!(decoded.format, ImageFormat::Png);
    }

    #[test]
    fn test_image_from_base64_invalid_format() {
        let result = ImageData::from_base64("invalid_data");
        assert!(result.is_err());
    }

    #[test]
    fn test_image_from_base64_unsupported_mime() {
        let result = ImageData::from_base64("data:image/bmp;base64,AQIDBA==");
        assert!(result.is_err());
    }

    #[test]
    fn test_image_from_base64_invalid_base64() {
        let result = ImageData::from_base64("data:image/png;base64,!!!invalid!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_arboard_manager_write_read() {
        let manager = ArboardManager::new();

        // Write test content
        manager.write_text("test clipboard content").unwrap();

        // Read it back
        let content = manager.read_text().unwrap();
        assert_eq!(content, "test clipboard content");
    }

    #[test]
    fn test_arboard_manager_overwrite() {
        let manager = ArboardManager::new();

        manager.write_text("first").unwrap();
        manager.write_text("second").unwrap();

        let content = manager.read_text().unwrap();
        assert_eq!(content, "second");
    }

    #[test]
    fn test_arboard_manager_unicode() {
        let manager = ArboardManager::new();

        let unicode_text = "Hello 世界 🌍";
        manager.write_text(unicode_text).unwrap();

        let content = manager.read_text().unwrap();
        assert_eq!(content, unicode_text);
    }
}
