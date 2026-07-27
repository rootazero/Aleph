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

impl ImageFormat {
    /// Return the MIME type string for this format.
    #[must_use]
    pub const fn mime_type(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
        }
    }

    /// Return the canonical file extension (without leading dot).
    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::WebP => "webp",
        }
    }
}

// ---------------------------------------------------------------------------
// Bounding Rectangle
// ---------------------------------------------------------------------------

/// Axis-aligned bounding rectangle in pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// Create a new Rect, validating that all coordinates are finite and
    /// width/height are non-negative.
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Result<Self, &'static str> {
        if !x.is_finite() {
            return Err("x must be finite");
        }
        if !y.is_finite() {
            return Err("y must be finite");
        }
        if !width.is_finite() {
            return Err("width must be finite");
        }
        if !height.is_finite() {
            return Err("height must be finite");
        }
        if width < 0.0 {
            return Err("width must be non-negative");
        }
        if height < 0.0 {
            return Err("height must be non-negative");
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Create a new Rect without validation (use with caution).
    #[must_use]
    pub const fn new_unchecked(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Check if the rectangle has valid dimensions (non-negative width/height).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 0.0
            && self.height >= 0.0
    }

    /// Return the area of the rectangle.
    #[must_use]
    pub fn area(&self) -> f64 {
        if self.width < 0.0
            || self.height < 0.0
            || !self.width.is_finite()
            || !self.height.is_finite()
        {
            0.0
        } else {
            self.width * self.height
        }
    }
}

// ---------------------------------------------------------------------------
// Vision Result (image understanding)
// ---------------------------------------------------------------------------

/// Result of an image-understanding request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VisionResult {
    /// Natural-language description of the image.
    pub description: String,

    /// Detected visual elements (objects, UI components, text regions, etc.).
    #[serde(default)]
    pub elements: Vec<VisualElement>,

    /// Overall confidence score in [0.0, 1.0].
    pub confidence: f64,
}

/// A single visual element detected in an image.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VisualElement {
    /// Human-readable label for this element.
    pub label: String,

    /// Semantic type of the element (e.g. "button", "text", "icon", "image").
    pub element_type: String,

    /// Bounding box in pixel coordinates, if available.
    pub bounds: Option<Rect>,

    /// Detection confidence in [0.0, 1.0].
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// OCR Result
// ---------------------------------------------------------------------------

/// Result of an OCR (Optical Character Recognition) request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrResult {
    /// Full recognized text concatenated from all lines.
    pub full_text: String,

    /// Individual recognized text lines with optional spatial information.
    #[serde(default)]
    pub lines: Vec<OcrLine>,
}

/// A single line of text recognized by OCR.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrLine {
    /// Recognized text content.
    pub text: String,

    /// Bounding box of this line in pixel coordinates, if available.
    pub bounding_box: Option<Rect>,

    /// Recognition confidence in [0.0, 1.0].
    pub confidence: f64,
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

    /// Can detect and localise objects with bounding boxes.
    pub object_detection: bool,
}

/// Validate that a confidence value is within the valid range [0.0, 1.0].
pub fn validate_confidence(confidence: f64) -> Result<f64, &'static str> {
    if !(0.0..=1.0).contains(&confidence) {
        return Err("confidence must be in [0.0, 1.0]");
    }
    Ok(confidence)
}

impl VisionCapabilities {
    /// A provider that supports all capabilities.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            image_understanding: true,
            ocr: true,
            object_detection: true,
        }
    }

    /// A provider that supports no capabilities (useful as a default).
    #[must_use]
    pub const fn none() -> Self {
        Self {
            image_understanding: false,
            ocr: false,
            object_detection: false,
        }
    }
}

impl Default for VisionCapabilities {
    fn default() -> Self {
        Self::none()
    }
}
