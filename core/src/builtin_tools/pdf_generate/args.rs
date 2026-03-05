//! Type definitions for the PDF generation tool
//!
//! Contains all argument types, output types, enums, and default value functions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Page size options
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum PageSize {
    /// A4 (210mm x 297mm) - default
    #[default]
    A4,
    /// Letter (8.5in x 11in)
    Letter,
    /// A3 (297mm x 420mm)
    A3,
    /// Custom size in mm
    Custom { width_mm: f32, height_mm: f32 },
}

impl PageSize {
    /// Get page dimensions in millimeters
    pub fn dimensions_mm(&self) -> (f32, f32) {
        match self {
            PageSize::A4 => (210.0, 297.0),
            PageSize::Letter => (215.9, 279.4),
            PageSize::A3 => (297.0, 420.0),
            PageSize::Custom { width_mm, height_mm } => (*width_mm, *height_mm),
        }
    }

    /// Get page dimensions in inches (mm / 25.4)
    pub fn dimensions_inches(&self) -> (f64, f64) {
        let (w_mm, h_mm) = self.dimensions_mm();
        (f64::from(w_mm) / 25.4, f64::from(h_mm) / 25.4)
    }
}

/// Content format
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContentFormat {
    /// Plain text
    #[default]
    Text,
    /// Markdown
    Markdown,
}

impl ContentFormat {
    /// Auto-detect whether content is Markdown based on common patterns.
    ///
    /// Returns `Markdown` if the content contains typical Markdown syntax
    /// (headings, bold/italic, lists, code blocks, links, etc.).
    pub fn detect(content: &str) -> Self {
        // Check first ~2000 chars for performance
        let sample = if content.len() > 2000 {
            content.get(..2000).unwrap_or(content)
        } else {
            content
        };

        let mut score = 0u32;

        for line in sample.lines() {
            let trimmed = line.trim();
            // Headings: # ## ### etc.
            if trimmed.starts_with("# ")
                || trimmed.starts_with("## ")
                || trimmed.starts_with("### ")
                || trimmed.starts_with("#### ")
            {
                score += 3;
            }
            // Unordered list items: - item or * item
            if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                score += 1;
            }
            // Ordered list items: 1. item
            if trimmed.len() > 2
                && trimmed.as_bytes()[0].is_ascii_digit()
                && trimmed.contains(". ")
            {
                score += 1;
            }
            // Code block fences
            if trimmed.starts_with("```") {
                score += 2;
            }
            // Blockquote
            if trimmed.starts_with("> ") {
                score += 1;
            }
            // Horizontal rule
            if trimmed == "---" || trimmed == "***" || trimmed == "___" {
                score += 1;
            }
        }

        // Inline patterns: **bold**, *italic*, [link](url), ![image](url), `code`
        if sample.contains("**") || sample.contains("__") {
            score += 2;
        }
        if sample.contains("](") {
            score += 2;
        }
        if sample.contains('`') {
            score += 1;
        }

        // Threshold: 3+ points → likely markdown
        if score >= 3 {
            ContentFormat::Markdown
        } else {
            ContentFormat::Text
        }
    }
}

/// Rendering engine for PDF generation
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderEngine {
    /// Automatically select the best engine (native for plain text, browser for rich content)
    #[default]
    Auto,
    /// Browser-based rendering via headless Chrome/Chromium (high-fidelity HTML/CSS)
    Browser,
    /// Native Rust rendering via printpdf (fast, no external dependencies)
    Native,
}

/// Arguments for PDF generation tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PdfGenerateArgs {
    /// Content to convert to PDF
    pub content: String,
    /// Output file path
    pub output_path: String,
    /// Content format (text or markdown)
    #[serde(default)]
    pub format: ContentFormat,
    /// Page size
    #[serde(default)]
    pub page_size: PageSize,
    /// Title (optional, shown at top of first page)
    #[serde(default)]
    pub title: Option<String>,
    /// Font size in points (default: 12)
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Line spacing multiplier (default: 1.5)
    #[serde(default = "default_line_spacing")]
    pub line_spacing: f32,
    /// Page margins in mm (default: 20)
    #[serde(default = "default_margin")]
    pub margin_mm: f32,
    /// Rendering engine (default: auto)
    #[serde(default)]
    pub render_engine: RenderEngine,
}

pub fn default_font_size() -> f32 {
    12.0
}

pub fn default_line_spacing() -> f32 {
    1.5
}

pub fn default_margin() -> f32 {
    20.0
}

/// Output from PDF generation tool
#[derive(Debug, Clone, Serialize)]
pub struct PdfGenerateOutput {
    pub success: bool,
    pub output_path: String,
    pub pages: usize,
    pub message: String,
}
