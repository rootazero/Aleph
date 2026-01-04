/// Image types for AI provider integration
///
/// NOTE: Clipboard operations are now handled by Swift ClipboardManager.
/// These types are kept only for AI provider image encoding/decoding.
///
/// See: refactor-native-api-separation proposal
use crate::error::Result;

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

/// Image data structure for AI provider integration
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
    pub fn from_base64(data_uri: &str) -> Result<Self> {
        use base64::{engine::general_purpose, Engine as _};

        let parts: Vec<&str> = data_uri.split(',').collect();
        if parts.len() != 2 {
            return Err(crate::error::AetherError::other(
                "Invalid Base64 data URI format".to_string(),
            ));
        }

        let header = parts[0];
        let base64_data = parts[1];

        let format = if header.contains("image/png") {
            ImageFormat::Png
        } else if header.contains("image/jpeg") {
            ImageFormat::Jpeg
        } else if header.contains("image/gif") {
            ImageFormat::Gif
        } else {
            return Err(crate::error::AetherError::other(format!(
                "Unsupported image MIME type: {}",
                header
            )));
        };

        let decoded = general_purpose::STANDARD.decode(base64_data).map_err(|e| {
            crate::error::AetherError::other(format!("Base64 decoding failed: {}", e))
        })?;

        Ok(Self::new(decoded, format))
    }
}
