//! Image types for AI provider integration
//!
//! NOTE: Clipboard operations are now handled by Swift `ClipboardManager`.
//! These types are kept only for AI provider image encoding/decoding.
//!
//! See: refactor-native-api-separation proposal
use crate::error::{AlephError, Result};
use base64::{engine::general_purpose, Engine as _};

/// Maximum allowed image size in bytes (20 MB)
///
/// This limit prevents `DoS` from maliciously large base64-encoded images.
const MAX_IMAGE_SIZE_BYTES: usize = 20 * 1024 * 1024;

/// Image format enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG format (lossless compression)
    Png,
    /// JPEG format (lossy compression)
    Jpeg,
    /// GIF format (supports animation)
    Gif,
    /// WebP format (modern lossy/lossless compression)
    WebP,
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
    /// Create a new `ImageData` instance
    #[must_use]
    pub const fn new(data: Vec<u8>, format: ImageFormat) -> Self {
        Self { data, format }
    }

    /// Get the size of the image data in bytes
    #[must_use]
    pub const fn size_bytes(&self) -> usize {
        self.data.len()
    }

    /// Get the size of the image data in megabytes
    #[must_use]
    pub fn size_mb(&self) -> f64 {
        self.data.len() as f64 / (1024.0 * 1024.0)
    }

    /// Convert image to Base64 data URI format for API requests
    ///
    /// Returns a string in the format: "data:image/<format>;base64,<`encoded_data`>"
    #[must_use]
    pub fn to_base64(&self) -> String {
        let mime_type = match self.format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Gif => "image/gif",
            ImageFormat::WebP => "image/webp",
        };

        let encoded = general_purpose::STANDARD.encode(&self.data);
        format!("data:{mime_type};base64,{encoded}")
    }

    /// Parse image from Base64 data URI
    ///
    /// Accepts strings in the format: "data:image/<format>;base64,<`encoded_data`>"
    pub fn from_base64(data_uri: &str) -> Result<Self> {
        let input_byte_len = data_uri.len();
        if input_byte_len > MAX_IMAGE_SIZE_BYTES * 2 {
            return Err(AlephError::other(format!(
                "Image data URI too large: {} bytes exceeds maximum of {} bytes",
                input_byte_len,
                MAX_IMAGE_SIZE_BYTES * 2
            )));
        }

        let (header, base64_data) = data_uri.split_once(',').ok_or_else(|| {
            AlephError::other("Invalid Base64 data URI format: missing comma separator".to_string())
        })?;

        let after_data = header.strip_prefix("data:").ok_or_else(|| {
            AlephError::other(format!(
                "Invalid Base64 data URI format: expected 'data:' prefix, got: {header}"
            ))
        })?;

        let has_base64_param = after_data
            .split(';')
            .any(|param| param.trim().eq_ignore_ascii_case("base64"));

        if !has_base64_param {
            return Err(AlephError::other(format!(
                "Invalid Base64 data URI format: expected 'base64' encoding in header: {header}"
            )));
        }

        let mime_type = after_data.split(';').next().ok_or_else(|| {
            AlephError::other(format!(
                "Invalid Base64 data URI format: empty MIME type in header: {header}"
            ))
        })?;

        let trimmed_mime = mime_type.trim();
        let format = if trimmed_mime.eq_ignore_ascii_case("image/png") {
            ImageFormat::Png
        } else if trimmed_mime.eq_ignore_ascii_case("image/jpeg") {
            ImageFormat::Jpeg
        } else if trimmed_mime.eq_ignore_ascii_case("image/gif") {
            ImageFormat::Gif
        } else if trimmed_mime.eq_ignore_ascii_case("image/webp") {
            ImageFormat::WebP
        } else {
            return Err(AlephError::other(format!(
                "Unsupported image MIME type: {mime_type}"
            )));
        };

        // Tolerate ASCII whitespace in the payload: data URIs are frequently
        // line-wrapped (MIME 76-column wrapping) or copy-pasted with stray
        // newlines. Whitespace carries no base64 bits, so stripping it does not
        // weaken validation. Only allocate when whitespace is actually present
        // to keep the common (clean) path zero-cost.
        let decoded = if base64_data.bytes().any(|b| b.is_ascii_whitespace()) {
            let cleaned: String = base64_data
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            general_purpose::STANDARD.decode(&cleaned)
        } else {
            general_purpose::STANDARD.decode(base64_data)
        }
        .map_err(|e| AlephError::other(format!("Base64 decoding failed: {e}")))?;

        if decoded.len() > MAX_IMAGE_SIZE_BYTES {
            return Err(AlephError::other(format!(
                "Decoded image too large: {} bytes exceeds maximum of {} bytes",
                decoded.len(),
                MAX_IMAGE_SIZE_BYTES
            )));
        }

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
    fn from_base64_webp() {
        let data_uri = "data:image/webp;base64,aGVsbG8=";
        let result = ImageData::from_base64(data_uri).unwrap();

        assert_eq!(result.data, b"hello");
        assert_eq!(result.format, ImageFormat::WebP);
    }

    #[test]
    fn from_base64_invalid_no_comma() {
        let result = ImageData::from_base64("data:image/png;base64");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing comma separator"),
            "Error should mention missing comma: {}",
            err
        );
    }

    #[test]
    fn from_base64_missing_base64_encoding() {
        let result = ImageData::from_base64("data:image/png,SGVsbG8=");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("base64"),
            "Error should mention missing base64 encoding: {}",
            err
        );
    }

    #[test]
    fn from_base64_invalid_no_data_prefix() {
        let result = ImageData::from_base64("image/png;base64,aGVsbG8=");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("expected 'data:' prefix"),
            "Error should mention data: prefix: {}",
            err
        );
    }

    #[test]
    fn from_base64_invalid_mime_type() {
        let result = ImageData::from_base64("data:image/webp-extra;base64,aGVsbG8=");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unsupported image MIME type"),
            "Error should mention unsupported MIME type: {}",
            err
        );
    }

    #[test]
    fn from_base64_malformed_base64() {
        let result = ImageData::from_base64("data:image/png;base64,!!!invalid!!!");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Base64 decoding failed"),
            "Error should mention Base64 decoding: {}",
            err
        );
    }

    #[test]
    fn from_base64_loose_match_rejected() {
        let result = ImageData::from_base64("data:image/png-extra;base64,aGVsbG8=");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unsupported image MIME type"),
            "Error should reject malformed MIME type: {}",
            err
        );
    }

    #[test]
    fn from_base64_case_insensitive_mime() {
        let result = ImageData::from_base64("data:image/PNG;base64,aGVsbG8=");
        assert!(result.is_ok(), "Should accept uppercase MIME type");
        assert_eq!(result.unwrap().format, ImageFormat::Png);

        let result = ImageData::from_base64("data:IMAGE/PNG;base64,aGVsbG8=");
        assert!(result.is_ok(), "Should accept fully uppercase MIME type");
        assert_eq!(result.unwrap().format, ImageFormat::Png);
    }

    #[test]
    fn from_base64_rejects_charset_base64_trick() {
        let result = ImageData::from_base64("data:image/png;charset=base64,aGVsbG8=");
        assert!(result.is_err(), "Should reject charset=base64 trick");
    }

    #[test]
    fn from_base64_rejects_oversized() {
        let huge_data = "A".repeat(45_000_000);
        let data_uri = format!("data:image/png;base64,{}", huge_data);
        let result = ImageData::from_base64(&data_uri);
        assert!(result.is_err(), "Should reject oversized images");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("too large") || err.contains("exceeds maximum"),
            "Error should mention size limit: {}",
            err
        );
    }

    #[test]
    fn from_base64_tolerates_whitespace_in_payload() {
        // "hello" -> base64 "aGVsbG8=", wrapped with a newline as MIME encoders do.
        let data_uri = "data:image/png;base64,aGVs\nbG8=";
        let result = ImageData::from_base64(data_uri).unwrap();
        assert_eq!(result.data, b"hello");
        assert_eq!(result.format, ImageFormat::Png);

        // Leading/trailing/interior spaces are also tolerated.
        let spaced = "data:image/png;base64, aGVs bG8= ";
        assert_eq!(ImageData::from_base64(spaced).unwrap().data, b"hello");
    }

    #[test]
    fn from_base64_empty_data() {
        let result = ImageData::from_base64("data:image/png;base64,");
        assert!(result.is_ok(), "Empty base64 data should be valid");
        assert_eq!(result.unwrap().data, Vec::<u8>::new());
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

    #[test]
    fn from_base64_empty_input() {
        let result = ImageData::from_base64("");
        assert!(result.is_err(), "Empty input should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing comma separator"),
            "Error should mention missing comma: {}",
            err
        );
    }

    #[test]
    fn from_base64_exact_size_limit() {
        // 20MB of zeros -> base64 encoded = ceil(20MB / 3) * 4 = 27,866,667 bytes
        let data = vec![0u8; MAX_IMAGE_SIZE_BYTES];
        let encoded = general_purpose::STANDARD.encode(&data);
        let data_uri = format!("data:image/png;base64,{}", encoded);
        let result = ImageData::from_base64(&data_uri);
        assert!(result.is_ok(), "Exact size limit should be accepted");
        assert_eq!(result.unwrap().data.len(), MAX_IMAGE_SIZE_BYTES);
    }

    #[test]
    fn from_base64_over_size_limit() {
        // 20MB + 1 byte -> should be rejected after decoding
        let data = vec![0u8; MAX_IMAGE_SIZE_BYTES + 1];
        let encoded = general_purpose::STANDARD.encode(&data);
        let data_uri = format!("data:image/png;base64,{}", encoded);
        let result = ImageData::from_base64(&data_uri);
        assert!(result.is_err(), "Over size limit should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("too large") || err.contains("exceeds maximum"),
            "Error should mention size limit: {}",
            err
        );
    }
}
