use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum allowed image file size in bytes (10 MB).
pub const MAX_IMAGE_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum size of a base64-encoded [`ImageInput::Base64::data`] string, derived
/// from [`MAX_IMAGE_FILE_SIZE`] so that *no* base64 payload larger than this
/// constant can decode to a value within the size cap.
///
/// Standard base64 alphabet (no line breaks, no whitespace): `N` decoded bytes
/// expand to `((N + 2) / 3) * 4` encoded chars. We use the looser integer form
/// `((N / 3) + 1) * 4` (one byte of slack) which is still a sound upper bound
/// for the encoded length of `N` bytes.
///
/// The cap is checked **before** `base64::decode` runs, so a 1 GiB base64
/// string is rejected before the decoder allocates the decoded buffer.
#[allow(
    clippy::integer_division,
    reason = "intentional: integer floor division is part of the encoded-size formula (`((N / 3) + 1) * 4` is a sound upper bound for base64 of `N` bytes)"
)]
pub const MAX_BASE64_ENCODED_SIZE: usize = ((MAX_IMAGE_FILE_SIZE / 3) + 1) as usize * 4;

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

/// A single line of text recognised by OCR, with optional bounding box and
/// confidence. Mirrors `aleph_desktop::OcrLine` at the vision-module boundary
/// so platform-side detail (raw recognition candidates, locale, etc.) does
/// not leak into the core result type.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrLine {
    /// The recognised text content.
    pub text: String,
    /// Optional bounding box for this line (screen coordinates).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bounding_box: Option<BoundingBox>,
    /// Optional confidence score in the range `0.0..=1.0`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
}

/// A bounding box in screen coordinates.
///
/// Field types are `f64` to match the desktop-side [`aleph_desktop::BoundingBox`]
/// exactly; conversion happens at the boundary so the core result type
/// does not force downstream consumers to know about pixel-precision loss.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Result of an OCR (Optical Character Recognition) request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrResult {
    /// Full recognised text concatenated from all lines.
    pub full_text: String,
    /// Per-line structured output (bounding box, confidence, text).
    /// Empty when the underlying provider does not surface line-level
    /// detail. Additive — existing consumers reading only `full_text`
    /// are unaffected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<OcrLine>,
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
