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

// ---------------------------------------------------------------------------
// Unified MediaType
// ---------------------------------------------------------------------------

/// Detected media type with format-specific metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaType {
    Image {
        format: MediaImageFormat,
    },
    Audio {
        format: AudioFormat,
        duration_secs: Option<f64>,
    },
    Video {
        format: VideoFormat,
        duration_secs: Option<f64>,
    },
    Document {
        format: DocFormat,
        pages: Option<u32>,
    },
    Unknown,
}

impl MediaType {
    /// Human-readable category name.
    #[must_use]
    pub const fn category(&self) -> &'static str {
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

/// Result of a media understanding operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "output_type", rename_all = "snake_case")]
pub enum MediaOutput {
    /// Plain text result (transcription, extracted text).
    Text { text: String },
    /// Natural-language description.
    Description { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_type_category() {
        let img = MediaType::Image {
            format: MediaImageFormat::Png,
        };
        assert_eq!(img.category(), "image");

        let aud = MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: Some(120.0),
        };
        assert_eq!(aud.category(), "audio");

        let vid = MediaType::Video {
            format: VideoFormat::Mp4,
            duration_secs: None,
        };
        assert_eq!(vid.category(), "video");

        let doc = MediaType::Document {
            format: DocFormat::Pdf,
            pages: Some(10),
        };
        assert_eq!(doc.category(), "document");

        assert_eq!(MediaType::Unknown.category(), "unknown");
    }

    #[test]
    fn media_type_serde_round_trip() {
        let mt = MediaType::Image {
            format: MediaImageFormat::Jpeg,
        };
        let json = serde_json::to_value(&mt).unwrap();
        assert_eq!(json["kind"], "image");
        assert_eq!(json["format"], "jpeg");
        let round_trip: MediaType = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip, mt);
    }

    #[test]
    fn media_output_serde_round_trip() {
        let output = MediaOutput::Text {
            text: "Hello world".into(),
        };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["output_type"], "text");
        let _: MediaOutput = serde_json::from_value(json).unwrap();

        let output = MediaOutput::Description {
            text: "A cat".into(),
        };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["output_type"], "description");
    }

    #[test]
    fn media_input_serde() {
        let input = MediaInput::FilePath {
            path: "/tmp/test.png".into(),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "file_path");

        let input = MediaInput::Url {
            url: "https://example.com/img.png".into(),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "url");
    }
}
