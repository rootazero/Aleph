use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum allowed image file size in bytes (10 MB).
pub const MAX_IMAGE_FILE_SIZE: u64 = 10 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Image Input
// ---------------------------------------------------------------------------

/// Input source for vision operations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageInput {
    /// Base64-encoded image data with explicit format.
    Base64 {
        /// Raw base64-encoded bytes (no data-URI prefix).
        data: String,
        /// Image format of the encoded data.
        format: ImageFormat,
    },

    /// Path to an image file on the local filesystem.
    FilePath { path: PathBuf },

    /// URL pointing to a remote image.
    Url { url: String },
}

// ---------------------------------------------------------------------------
// Image Format
// ---------------------------------------------------------------------------

/// Supported image formats for vision operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
}

// ---------------------------------------------------------------------------
// Vision Result (image understanding)
// ---------------------------------------------------------------------------

/// Result of an image-understanding request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VisionResult {
    /// Natural-language description of the image.
    pub description: String,
}

// ---------------------------------------------------------------------------
// OCR Result
// ---------------------------------------------------------------------------

/// Result of an OCR (Optical Character Recognition) request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrResult {
    /// Full recognized text concatenated from all lines.
    pub full_text: String,
}

// ---------------------------------------------------------------------------
// Vision Capabilities
// ---------------------------------------------------------------------------

/// Declares which vision capabilities a provider supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VisionCapabilities {
    /// Can perform general image understanding (describe, answer questions).
    pub image_understanding: bool,

    /// Can perform OCR (text extraction from images).
    pub ocr: bool,
}

impl VisionCapabilities {
    /// A provider that supports all capabilities.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            image_understanding: true,
            ocr: true,
        }
    }

    /// A provider that supports no capabilities (useful as a default).
    #[must_use]
    pub const fn none() -> Self {
        Self {
            image_understanding: false,
            ocr: false,
        }
    }
}

impl Default for VisionCapabilities {
    fn default() -> Self {
        Self::none()
    }
}
