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

        let (header, base64_data) = data_uri.split_once(',').ok_or_else(|| {
            crate::error::AlephError::other(
                "Invalid Base64 data URI format: missing comma separator".to_string(),
            )
        })?;

        if !header.starts_with("data:") {
            return Err(crate::error::AlephError::other(format!(
                "Invalid Base64 data URI format: expected 'data:' prefix, got: {}",
                header
            )));
        }

        let mime_type = header
            .strip_prefix("data:")
            .and_then(|h| h.split(';').next())
            .ok_or_else(|| {
                crate::error::AlephError::other(format!(
                    "Invalid Base64 data URI format: cannot extract MIME type from: {}",
                    header
                ))
            })?;

        let format = match mime_type.trim() {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            _ => {
                return Err(crate::error::AlephError::other(format!(
                    "Unsupported image MIME type: {}",
                    mime_type
                )));
            }
        };

        let decoded = general_purpose::STANDARD.decode(base64_data).map_err(|e| {
            crate::error::AlephError::other(format!("Base64 decoding failed: {}", e))
        })?;

        Ok(Self::new(decoded, format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_base64_roundtrip() {
        let data = b"hello world";
        let image = ImageData::new(data.to_vec(), ImageFormat::Png);
        let encoded = image.to_base64();
        let decoded = ImageData::from_base64(&encoded).unwrap();

        assert_eq!(decoded.data, data);
        assert_eq!(decoded.format, ImageFormat::Png);
    }

    #[test]
    fn from_base64_png() {
        let data_uri = "data:image/png;base64,aGVsbG8=";
        let result = ImageData::from_base64(data_uri).unwrap();

        assert_eq!(result.data, b"hello");
        assert_eq!(result.format, ImageFormat::Png);
    }

    #[test]
    fn from_base64_jpeg() {
        let data_uri = "data:image/jpeg;base64,aGVsbG8=";
        let result = ImageData::from_base64(data_uri).unwrap();

        assert_eq!(result.data, b"hello");
        assert_eq!(result.format, ImageFormat::Jpeg);
    }

    #[test]
    fn from_base64_gif() {
        let data_uri = "data:image/gif;base64,aGVsbG8=";
        let result = ImageData::from_base64(data_uri).unwrap();

        assert_eq!(result.data, b"hello");
        assert_eq!(result.format, ImageFormat::Gif);
    }

    #[test]
    fn from_base64_invalid_no_comma() {
        let result = ImageData::from_base64("data:image/png;base64");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing comma separator"), "Error should mention missing comma: {}", err);
    }

    #[test]
    fn from_base64_invalid_no_data_prefix() {
        let result = ImageData::from_base64("image/png;base64,aGVsbG8=");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("expected 'data:' prefix"), "Error should mention data: prefix: {}", err);
    }

    #[test]
    fn from_base64_invalid_mime_type() {
        let result = ImageData::from_base64("data:image/webp;base64,aGVsbG8=");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported image MIME type"), "Error should mention unsupported MIME type: {}", err);
    }

    #[test]
    fn from_base64_malformed_base64() {
        let result = ImageData::from_base64("data:image/png;base64,!!!invalid!!!");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Base64 decoding failed"), "Error should mention Base64 decoding: {}", err);
    }

    #[test]
    fn from_base64_loose_match_rejected() {
        // This would have incorrectly matched as PNG with the old `contains()` logic
        let result = ImageData::from_base64("data:image/png-extra;base64,aGVsbG8=");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported image MIME type"), "Error should reject malformed MIME type: {}", err);
    }

    #[test]
    fn size_bytes_and_mb() {
        let data = vec![0u8; 1024 * 1024];
        let image = ImageData::new(data, ImageFormat::Png);

        assert_eq!(image.size_bytes(), 1024 * 1024);
        assert!((image.size_mb() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn new_constructor() {
        let data = vec![1, 2, 3];
        let image = ImageData::new(data.clone(), ImageFormat::Gif);

        assert_eq!(image.data, data);
        assert_eq!(image.format, ImageFormat::Gif);
    }
}
