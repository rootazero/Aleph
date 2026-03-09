//! Media format types, input sources, and output structures.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Image Format
// ---------------------------------------------------------------------------

/// Supported image formats for media understanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MediaImageFormat {
    Png,
    Jpeg,
    WebP,
    Gif,
    Svg,
    Heic,
}

impl MediaImageFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Svg => "image/svg+xml",
            Self::Heic => "image/heic",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::WebP => "webp",
            Self::Gif => "gif",
            Self::Svg => "svg",
            Self::Heic => "heic",
        }
    }
}

// ---------------------------------------------------------------------------
// Audio Format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Mp3,
    Wav,
    Ogg,
    Flac,
    M4a,
}

impl AudioFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Ogg => "audio/ogg",
            Self::Flac => "audio/flac",
            Self::M4a => "audio/mp4",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
            Self::M4a => "m4a",
        }
    }
}

// ---------------------------------------------------------------------------
// Video Format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    Mp4,
    WebM,
    Mov,
}

impl VideoFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp4 => "video/mp4",
            Self::WebM => "video/webm",
            Self::Mov => "video/quicktime",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::WebM => "webm",
            Self::Mov => "mov",
        }
    }
}

// ---------------------------------------------------------------------------
// Document Format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DocFormat {
    Pdf,
    Docx,
    Xlsx,
    Txt,
    Markdown,
    Html,
}

impl DocFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Txt => "text/plain",
            Self::Markdown => "text/markdown",
            Self::Html => "text/html",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Txt => "txt",
            Self::Markdown => "md",
            Self::Html => "html",
        }
    }
}

// ---------------------------------------------------------------------------
// Unified MediaType
// ---------------------------------------------------------------------------

/// Detected media type with format-specific metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaType {
    Image { format: MediaImageFormat, width: Option<u32>, height: Option<u32> },
    Audio { format: AudioFormat, duration_secs: Option<f64> },
    Video { format: VideoFormat, duration_secs: Option<f64> },
    Document { format: DocFormat, pages: Option<u32> },
    Unknown,
}

impl MediaType {
    /// Human-readable category name.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Image { .. } => "image",
            Self::Audio { .. } => "audio",
            Self::Video { .. } => "video",
            Self::Document { .. } => "document",
            Self::Unknown => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// MediaInput — source of media data
// ---------------------------------------------------------------------------

/// Input source for media understanding operations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaInput {
    /// Local file path.
    FilePath { path: PathBuf },
    /// Base64-encoded data with explicit media type.
    Base64 { data: String, media_type: MediaType },
    /// Remote URL.
    Url { url: String },
}

// ---------------------------------------------------------------------------
// MediaOutput — result of processing
// ---------------------------------------------------------------------------

/// A chunk of media output (for large media split into segments).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaChunk {
    /// Segment index (0-based).
    pub index: u32,
    /// Start offset in seconds (audio/video) or page number (document).
    pub offset: f64,
    /// Duration in seconds (audio/video) or page count (document).
    pub length: f64,
    /// Content for this chunk.
    pub content: String,
}

/// Result of a media understanding operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "output_type", rename_all = "snake_case")]
pub enum MediaOutput {
    /// Plain text result (transcription, extracted text).
    Text { text: String },
    /// Natural-language description.
    Description { text: String, confidence: f64 },
    /// Structured data (tables, charts, metadata).
    Structured { data: serde_json::Value },
    /// Chunked output for long media.
    Chunks { chunks: Vec<MediaChunk> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_type_category() {
        let img = MediaType::Image { format: MediaImageFormat::Png, width: Some(800), height: Some(600) };
        assert_eq!(img.category(), "image");

        let aud = MediaType::Audio { format: AudioFormat::Mp3, duration_secs: Some(120.0) };
        assert_eq!(aud.category(), "audio");

        let vid = MediaType::Video { format: VideoFormat::Mp4, duration_secs: None };
        assert_eq!(vid.category(), "video");

        let doc = MediaType::Document { format: DocFormat::Pdf, pages: Some(10) };
        assert_eq!(doc.category(), "document");

        assert_eq!(MediaType::Unknown.category(), "unknown");
    }

    #[test]
    fn format_mime_types() {
        assert_eq!(MediaImageFormat::Png.mime_type(), "image/png");
        assert_eq!(AudioFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(VideoFormat::Mp4.mime_type(), "video/mp4");
        assert_eq!(DocFormat::Pdf.mime_type(), "application/pdf");
    }

    #[test]
    fn format_extensions() {
        assert_eq!(MediaImageFormat::Heic.extension(), "heic");
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(VideoFormat::Mov.extension(), "mov");
        assert_eq!(DocFormat::Markdown.extension(), "md");
    }

    #[test]
    fn media_type_serde_round_trip() {
        let mt = MediaType::Image { format: MediaImageFormat::Jpeg, width: Some(1920), height: Some(1080) };
        let json = serde_json::to_value(&mt).unwrap();
        assert_eq!(json["kind"], "image");
        assert_eq!(json["format"], "jpeg");
        let round_trip: MediaType = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip, mt);
    }

    #[test]
    fn media_output_serde_round_trip() {
        let output = MediaOutput::Text { text: "Hello world".into() };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["output_type"], "text");
        let _: MediaOutput = serde_json::from_value(json).unwrap();

        let output = MediaOutput::Description { text: "A cat".into(), confidence: 0.95 };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["output_type"], "description");

        let output = MediaOutput::Chunks { chunks: vec![
            MediaChunk { index: 0, offset: 0.0, length: 30.0, content: "First segment".into() },
        ]};
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["output_type"], "chunks");
    }

    #[test]
    fn media_input_serde() {
        let input = MediaInput::FilePath { path: "/tmp/test.png".into() };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "file_path");

        let input = MediaInput::Url { url: "https://example.com/img.png".into() };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "url");
    }
}
