# P3: Media Understanding Pipeline -- Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a unified media understanding pipeline to Aleph that can detect, route, and process images, audio, video, and documents through pluggable providers with multi-provider fallback. The pipeline exposes 4 new tools (`media_understand`, `audio_transcribe`, `document_extract`, enhanced `vision`) to the AI agent.

**Architecture:** Core defines `MediaProvider` trait and `MediaPipeline` orchestrator following the existing `VisionPipeline` fallback pattern. ImageProcessor wraps existing `VisionPipeline`. AudioProcessor and VideoProcessor are trait-only stubs in core (actual processing via external providers / P4 plugins per R3). DocumentProcessor handles plain text/Markdown natively, PDF via lightweight Rust crate, DOCX/XLSX deferred to plugins. Format detection uses magic bytes + extension mapping. Size policy enforcement at pipeline level.

**Tech Stack:** Rust (core traits + pipeline + format detection + tools), existing `VisionPipeline` (image), `async_trait`, `schemars`, `serde`. No new heavy deps in core (R3). External processing via LLM API providers (Whisper, Claude Vision, etc.).

**Key Constraint:** Per R1/R3, core only defines traits and lightweight orchestration. Heavy processing (ffmpeg, DOCX parsing, video keyframe extraction) goes to Node.js plugins (P4) or external API providers.

---

## Task 1: MediaType + Format Enums

Define all media format enums and the unified MediaType enum.

**Files:**
- Create: `core/src/media/types.rs`
- Create: `core/src/media/mod.rs`
- Modify: `core/src/lib.rs` (add `pub mod media;`)

**Step 1: Write the failing test**

```rust
// In core/src/media/types.rs

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Image Format (extends vision::ImageFormat with GIF/SVG/HEIC)
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
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::types::tests
# Expected: compilation error (module doesn't exist yet)
```

**Step 3: Write minimal implementation**

The types.rs above IS the implementation. Create `core/src/media/mod.rs`:

```rust
//! Media understanding pipeline — unified interface for image, audio, video, and document processing.
//!
//! This module defines the [`MediaProvider`] trait, [`MediaPipeline`] orchestrator,
//! and format detection utilities. Heavy processing is delegated to external
//! providers or plugins (per R1/R3).

pub mod types;

pub use types::{
    AudioFormat, DocFormat, MediaChunk, MediaImageFormat, MediaInput, MediaOutput, MediaType,
    VideoFormat,
};
```

Add to `core/src/lib.rs` (after `pub mod memory;`):

```rust
pub mod media;
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media::types::tests
```

**Step 5: Commit**

```
media: add MediaType, format enums, MediaInput/Output types for unified media pipeline
```

---

## Task 2: MediaError + MediaProvider Trait

Define the error type and provider trait following VisionProvider pattern.

**Files:**
- Create: `core/src/media/error.rs`
- Create: `core/src/media/provider.rs`
- Modify: `core/src/media/mod.rs` (add modules + re-exports)

**Step 1: Write the failing test**

```rust
// In core/src/media/error.rs
use thiserror::Error;

/// Errors that can occur during media processing.
#[derive(Debug, Error)]
pub enum MediaError {
    /// No provider configured for this media type.
    #[error("No media provider available for {media_type}")]
    NoProvider { media_type: String },

    /// A provider returned an error.
    #[error("Media provider error [{provider}]: {message}")]
    ProviderError { provider: String, message: String },

    /// File exceeds size policy.
    #[error("Media exceeds size limit: {message}")]
    SizeLimitExceeded { message: String },

    /// Unsupported format.
    #[error("Unsupported media format: {0}")]
    UnsupportedFormat(String),

    /// Format detection failed.
    #[error("Cannot detect media format: {0}")]
    DetectionFailed(String),

    /// I/O error reading file.
    #[error("I/O error: {0}")]
    IoError(String),
}

impl From<std::io::Error> for MediaError {
    fn from(err: std::io::Error) -> Self {
        MediaError::IoError(err.to_string())
    }
}
```

```rust
// In core/src/media/provider.rs
use async_trait::async_trait;
use super::error::MediaError;
use super::types::{MediaInput, MediaOutput, MediaType};

/// Pluggable backend for media understanding.
///
/// Implementations may delegate to:
/// - Multimodal LLMs (Claude, GPT-4V) for image/video understanding
/// - Whisper API for audio transcription
/// - Platform OCR for text extraction
/// - External plugins for video/document processing
///
/// The [`MediaPipeline`](super::MediaPipeline) orchestrates multiple providers,
/// trying them in priority order with fallback.
#[async_trait]
pub trait MediaProvider: Send + Sync {
    /// Human-readable name (used for logging / diagnostics).
    fn name(&self) -> &str;

    /// Media types this provider can process.
    fn supported_types(&self) -> Vec<MediaType>;

    /// Check if this provider supports a given media type category.
    fn supports(&self, media_type: &MediaType) -> bool {
        let category = media_type.category();
        self.supported_types().iter().any(|t| t.category() == category)
    }

    /// Priority (lower = higher priority, tried first). Default: 100.
    fn priority(&self) -> u8 {
        100
    }

    /// Process a media input and return understanding output.
    async fn process(&self, input: &MediaInput, media_type: &MediaType, prompt: Option<&str>) -> Result<MediaOutput, MediaError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::*;

    struct MockProvider {
        name: &'static str,
        priority: u8,
    }

    #[async_trait]
    impl MediaProvider for MockProvider {
        fn name(&self) -> &str { self.name }
        fn supported_types(&self) -> Vec<MediaType> {
            vec![MediaType::Image { format: MediaImageFormat::Png, width: None, height: None }]
        }
        fn priority(&self) -> u8 { self.priority }
        async fn process(&self, _input: &MediaInput, _media_type: &MediaType, _prompt: Option<&str>) -> Result<MediaOutput, MediaError> {
            Ok(MediaOutput::Description { text: format!("[{}] described", self.name), confidence: 0.9 })
        }
    }

    #[test]
    fn provider_supports_matching_category() {
        let p = MockProvider { name: "mock", priority: 10 };
        let png = MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
        let jpeg = MediaType::Image { format: MediaImageFormat::Jpeg, width: Some(100), height: Some(100) };
        let audio = MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None };

        assert!(p.supports(&png));
        assert!(p.supports(&jpeg)); // same category "image"
        assert!(!p.supports(&audio));
    }

    #[test]
    fn provider_default_priority() {
        struct DefaultPrio;
        #[async_trait]
        impl MediaProvider for DefaultPrio {
            fn name(&self) -> &str { "default" }
            fn supported_types(&self) -> Vec<MediaType> { vec![] }
            async fn process(&self, _: &MediaInput, _: &MediaType, _: Option<&str>) -> Result<MediaOutput, MediaError> {
                unreachable!()
            }
        }
        assert_eq!(DefaultPrio.priority(), 100);
    }

    #[tokio::test]
    async fn provider_process_returns_output() {
        let p = MockProvider { name: "test", priority: 50 };
        let input = MediaInput::FilePath { path: "/tmp/test.png".into() };
        let mt = MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
        let result = p.process(&input, &mt, Some("describe")).await.unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[test]")),
            _ => panic!("Expected Description"),
        }
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::provider::tests
```

**Step 3: Write minimal implementation**

The code above IS the implementation. Update `core/src/media/mod.rs`:

```rust
//! Media understanding pipeline — unified interface for image, audio, video, and document processing.

pub mod error;
pub mod provider;
pub mod types;

pub use error::MediaError;
pub use provider::MediaProvider;
pub use types::{
    AudioFormat, DocFormat, MediaChunk, MediaImageFormat, MediaInput, MediaOutput, MediaType,
    VideoFormat,
};
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media::provider::tests
```

**Step 5: Commit**

```
media: add MediaError and MediaProvider trait with fallback support
```

---

## Task 3: MediaPolicy — Size Enforcement

**Files:**
- Create: `core/src/media/policy.rs`
- Modify: `core/src/media/mod.rs`

**Step 1: Write the failing test**

```rust
// In core/src/media/policy.rs
use std::path::PathBuf;
use std::time::Duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use super::error::MediaError;
use super::types::MediaType;

/// Size and lifecycle policy for media processing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaPolicy {
    /// Maximum image file size in bytes (default: 20 MB).
    #[serde(default = "default_max_image_bytes")]
    pub max_image_bytes: u64,

    /// Maximum audio file size in bytes (default: 100 MB).
    #[serde(default = "default_max_audio_bytes")]
    pub max_audio_bytes: u64,

    /// Maximum video duration in seconds (default: 1800 = 30 min).
    #[serde(default = "default_max_video_duration")]
    pub max_video_duration: u64,

    /// Maximum document pages (default: 200).
    #[serde(default = "default_max_document_pages")]
    pub max_document_pages: u32,

    /// Temporary file directory.
    #[serde(default = "default_temp_dir")]
    pub temp_dir: PathBuf,

    /// Temp file TTL in seconds (default: 3600 = 1 hour).
    #[serde(default = "default_temp_ttl_secs")]
    pub temp_ttl_secs: u64,
}

fn default_max_image_bytes() -> u64 { 20 * 1024 * 1024 }
fn default_max_audio_bytes() -> u64 { 100 * 1024 * 1024 }
fn default_max_video_duration() -> u64 { 1800 }
fn default_max_document_pages() -> u32 { 200 }
fn default_temp_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("aleph")
        .join("media_temp")
}
fn default_temp_ttl_secs() -> u64 { 3600 }

impl Default for MediaPolicy {
    fn default() -> Self {
        Self {
            max_image_bytes: default_max_image_bytes(),
            max_audio_bytes: default_max_audio_bytes(),
            max_video_duration: default_max_video_duration(),
            max_document_pages: default_max_document_pages(),
            temp_dir: default_temp_dir(),
            temp_ttl_secs: default_temp_ttl_secs(),
        }
    }
}

impl MediaPolicy {
    /// Temp file TTL as Duration.
    pub fn temp_ttl(&self) -> Duration {
        Duration::from_secs(self.temp_ttl_secs)
    }

    /// Validate file size against policy for the given media type.
    pub fn check_size(&self, media_type: &MediaType, file_size_bytes: u64) -> Result<(), MediaError> {
        match media_type {
            MediaType::Image { .. } => {
                if file_size_bytes > self.max_image_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Image size {} bytes exceeds limit of {} bytes",
                            file_size_bytes, self.max_image_bytes
                        ),
                    });
                }
            }
            MediaType::Audio { .. } => {
                if file_size_bytes > self.max_audio_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Audio size {} bytes exceeds limit of {} bytes",
                            file_size_bytes, self.max_audio_bytes
                        ),
                    });
                }
            }
            MediaType::Video { duration_secs, .. } => {
                if let Some(dur) = duration_secs {
                    if *dur > self.max_video_duration as f64 {
                        return Err(MediaError::SizeLimitExceeded {
                            message: format!(
                                "Video duration {:.0}s exceeds limit of {}s",
                                dur, self.max_video_duration
                            ),
                        });
                    }
                }
            }
            MediaType::Document { pages, .. } => {
                if let Some(p) = pages {
                    if *p > self.max_document_pages {
                        return Err(MediaError::SizeLimitExceeded {
                            message: format!(
                                "Document has {} pages, exceeds limit of {}",
                                p, self.max_document_pages
                            ),
                        });
                    }
                }
            }
            MediaType::Unknown => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::*;

    #[test]
    fn default_policy_values() {
        let p = MediaPolicy::default();
        assert_eq!(p.max_image_bytes, 20 * 1024 * 1024);
        assert_eq!(p.max_audio_bytes, 100 * 1024 * 1024);
        assert_eq!(p.max_video_duration, 1800);
        assert_eq!(p.max_document_pages, 200);
        assert_eq!(p.temp_ttl(), Duration::from_secs(3600));
    }

    #[test]
    fn check_size_image_ok() {
        let p = MediaPolicy::default();
        let mt = MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
        assert!(p.check_size(&mt, 1024).is_ok());
    }

    #[test]
    fn check_size_image_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
        assert!(p.check_size(&mt, 21 * 1024 * 1024).is_err());
    }

    #[test]
    fn check_size_audio_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None };
        assert!(p.check_size(&mt, 101 * 1024 * 1024).is_err());
    }

    #[test]
    fn check_size_video_duration_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Video { format: VideoFormat::Mp4, duration_secs: Some(2000.0) };
        assert!(p.check_size(&mt, 0).is_err());
    }

    #[test]
    fn check_size_document_pages_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Document { format: DocFormat::Pdf, pages: Some(300) };
        assert!(p.check_size(&mt, 0).is_err());
    }

    #[test]
    fn check_size_unknown_always_ok() {
        let p = MediaPolicy::default();
        assert!(p.check_size(&MediaType::Unknown, u64::MAX).is_ok());
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::policy::tests
```

**Step 3: Write minimal implementation** — code above IS the implementation.

Update `core/src/media/mod.rs` to add:

```rust
pub mod policy;
pub use policy::MediaPolicy;
```

Note: Add `dirs` crate to Cargo.toml if not already present (check first — it may already be a dependency). If not available, replace `default_temp_dir` with a simpler `/tmp/aleph/media_temp` fallback.

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media::policy::tests
```

**Step 5: Commit**

```
media: add MediaPolicy for size and lifecycle enforcement
```

---

## Task 4: Format Detection

Detect media format from file extension and magic bytes.

**Files:**
- Create: `core/src/media/detect.rs`
- Modify: `core/src/media/mod.rs`

**Step 1: Write the failing test**

```rust
// In core/src/media/detect.rs
use super::types::{AudioFormat, DocFormat, MediaImageFormat, MediaType, VideoFormat};
use super::error::MediaError;

/// Detect media type from file extension.
pub fn detect_by_extension(ext: &str) -> Result<MediaType, MediaError> {
    let ext_lower = ext.to_ascii_lowercase();
    let ext_clean = ext_lower.trim_start_matches('.');

    match ext_clean {
        // Images
        "png" => Ok(MediaType::Image { format: MediaImageFormat::Png, width: None, height: None }),
        "jpg" | "jpeg" => Ok(MediaType::Image { format: MediaImageFormat::Jpeg, width: None, height: None }),
        "webp" => Ok(MediaType::Image { format: MediaImageFormat::WebP, width: None, height: None }),
        "gif" => Ok(MediaType::Image { format: MediaImageFormat::Gif, width: None, height: None }),
        "svg" => Ok(MediaType::Image { format: MediaImageFormat::Svg, width: None, height: None }),
        "heic" | "heif" => Ok(MediaType::Image { format: MediaImageFormat::Heic, width: None, height: None }),
        // Audio
        "mp3" => Ok(MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None }),
        "wav" => Ok(MediaType::Audio { format: AudioFormat::Wav, duration_secs: None }),
        "ogg" => Ok(MediaType::Audio { format: AudioFormat::Ogg, duration_secs: None }),
        "flac" => Ok(MediaType::Audio { format: AudioFormat::Flac, duration_secs: None }),
        "m4a" => Ok(MediaType::Audio { format: AudioFormat::M4a, duration_secs: None }),
        // Video
        "mp4" => Ok(MediaType::Video { format: VideoFormat::Mp4, duration_secs: None }),
        "webm" => Ok(MediaType::Video { format: VideoFormat::WebM, duration_secs: None }),
        "mov" => Ok(MediaType::Video { format: VideoFormat::Mov, duration_secs: None }),
        // Documents
        "pdf" => Ok(MediaType::Document { format: DocFormat::Pdf, pages: None }),
        "docx" => Ok(MediaType::Document { format: DocFormat::Docx, pages: None }),
        "xlsx" => Ok(MediaType::Document { format: DocFormat::Xlsx, pages: None }),
        "txt" => Ok(MediaType::Document { format: DocFormat::Txt, pages: None }),
        "md" | "markdown" => Ok(MediaType::Document { format: DocFormat::Markdown, pages: None }),
        "html" | "htm" => Ok(MediaType::Document { format: DocFormat::Html, pages: None }),
        _ => Err(MediaError::UnsupportedFormat(ext_clean.to_string())),
    }
}

/// Detect media type from file magic bytes (first 16 bytes).
///
/// Falls back to Unknown if no signature matches.
pub fn detect_by_magic(bytes: &[u8]) -> MediaType {
    if bytes.len() < 4 {
        return MediaType::Unknown;
    }

    // PNG: 89 50 4E 47
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return MediaType::Image { format: MediaImageFormat::Jpeg, width: None, height: None };
    }
    // GIF: GIF87a or GIF89a
    if bytes.starts_with(b"GIF8") {
        return MediaType::Image { format: MediaImageFormat::Gif, width: None, height: None };
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return MediaType::Image { format: MediaImageFormat::WebP, width: None, height: None };
    }
    // PDF: %PDF
    if bytes.starts_with(b"%PDF") {
        return MediaType::Document { format: DocFormat::Pdf, pages: None };
    }
    // ZIP-based (DOCX/XLSX): PK\x03\x04
    // NOTE: Cannot distinguish DOCX vs XLSX from magic alone; caller should use extension
    if bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        // Return generic document; refine with extension
        return MediaType::Document { format: DocFormat::Docx, pages: None };
    }
    // MP3: ID3 tag or sync word
    if bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0) {
        return MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None };
    }
    // WAV: RIFF....WAVE
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return MediaType::Audio { format: AudioFormat::Wav, duration_secs: None };
    }
    // OGG: OggS
    if bytes.starts_with(b"OggS") {
        return MediaType::Audio { format: AudioFormat::Ogg, duration_secs: None };
    }
    // FLAC: fLaC
    if bytes.starts_with(b"fLaC") {
        return MediaType::Audio { format: AudioFormat::Flac, duration_secs: None };
    }
    // ftyp-based containers (MP4/MOV/M4A): check at offset 4
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"M4A " || brand == b"M4B " {
            return MediaType::Audio { format: AudioFormat::M4a, duration_secs: None };
        }
        if brand == b"qt  " {
            return MediaType::Video { format: VideoFormat::Mov, duration_secs: None };
        }
        // Default ftyp = MP4
        return MediaType::Video { format: VideoFormat::Mp4, duration_secs: None };
    }
    // WebM: 1A 45 DF A3 (EBML header, typically Matroska/WebM)
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return MediaType::Video { format: VideoFormat::WebM, duration_secs: None };
    }

    MediaType::Unknown
}

/// Detect from file path: try magic bytes first, fall back to extension.
pub fn detect_from_path(path: &std::path::Path) -> Result<MediaType, MediaError> {
    // Try magic bytes if file exists
    if path.exists() {
        if let Ok(bytes) = std::fs::read(path).map(|b| b.into_iter().take(16).collect::<Vec<_>>()) {
            let magic_result = detect_by_magic(&bytes);
            if magic_result != MediaType::Unknown {
                return Ok(magic_result);
            }
        }
    }

    // Fall back to extension
    path.extension()
        .and_then(|e| e.to_str())
        .map(detect_by_extension)
        .unwrap_or(Err(MediaError::DetectionFailed(
            format!("Cannot determine media type for: {}", path.display()),
        )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_image_extensions() {
        assert!(matches!(detect_by_extension("png").unwrap(), MediaType::Image { format: MediaImageFormat::Png, .. }));
        assert!(matches!(detect_by_extension("JPG").unwrap(), MediaType::Image { format: MediaImageFormat::Jpeg, .. }));
        assert!(matches!(detect_by_extension(".jpeg").unwrap(), MediaType::Image { format: MediaImageFormat::Jpeg, .. }));
        assert!(matches!(detect_by_extension("webp").unwrap(), MediaType::Image { format: MediaImageFormat::WebP, .. }));
        assert!(matches!(detect_by_extension("gif").unwrap(), MediaType::Image { format: MediaImageFormat::Gif, .. }));
        assert!(matches!(detect_by_extension("heic").unwrap(), MediaType::Image { format: MediaImageFormat::Heic, .. }));
        assert!(matches!(detect_by_extension("heif").unwrap(), MediaType::Image { format: MediaImageFormat::Heic, .. }));
    }

    #[test]
    fn detect_audio_extensions() {
        assert!(matches!(detect_by_extension("mp3").unwrap(), MediaType::Audio { format: AudioFormat::Mp3, .. }));
        assert!(matches!(detect_by_extension("wav").unwrap(), MediaType::Audio { format: AudioFormat::Wav, .. }));
        assert!(matches!(detect_by_extension("flac").unwrap(), MediaType::Audio { format: AudioFormat::Flac, .. }));
        assert!(matches!(detect_by_extension("m4a").unwrap(), MediaType::Audio { format: AudioFormat::M4a, .. }));
    }

    #[test]
    fn detect_video_extensions() {
        assert!(matches!(detect_by_extension("mp4").unwrap(), MediaType::Video { format: VideoFormat::Mp4, .. }));
        assert!(matches!(detect_by_extension("webm").unwrap(), MediaType::Video { format: VideoFormat::WebM, .. }));
        assert!(matches!(detect_by_extension("mov").unwrap(), MediaType::Video { format: VideoFormat::Mov, .. }));
    }

    #[test]
    fn detect_document_extensions() {
        assert!(matches!(detect_by_extension("pdf").unwrap(), MediaType::Document { format: DocFormat::Pdf, .. }));
        assert!(matches!(detect_by_extension("md").unwrap(), MediaType::Document { format: DocFormat::Markdown, .. }));
        assert!(matches!(detect_by_extension("html").unwrap(), MediaType::Document { format: DocFormat::Html, .. }));
    }

    #[test]
    fn detect_unknown_extension() {
        assert!(detect_by_extension("xyz").is_err());
    }

    #[test]
    fn detect_magic_png() {
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Image { format: MediaImageFormat::Png, .. }));
    }

    #[test]
    fn detect_magic_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Image { format: MediaImageFormat::Jpeg, .. }));
    }

    #[test]
    fn detect_magic_pdf() {
        let bytes = b"%PDF-1.4 rest of header";
        assert!(matches!(detect_by_magic(bytes), MediaType::Document { format: DocFormat::Pdf, .. }));
    }

    #[test]
    fn detect_magic_wav() {
        let bytes = b"RIFF\x00\x00\x00\x00WAVEfmt ";
        assert!(matches!(detect_by_magic(bytes), MediaType::Audio { format: AudioFormat::Wav, .. }));
    }

    #[test]
    fn detect_magic_webp() {
        let bytes = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert!(matches!(detect_by_magic(bytes), MediaType::Image { format: MediaImageFormat::WebP, .. }));
    }

    #[test]
    fn detect_magic_mp4() {
        let bytes = [0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 0, 0];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Video { format: VideoFormat::Mp4, .. }));
    }

    #[test]
    fn detect_magic_too_short() {
        assert!(matches!(detect_by_magic(&[0x89, 0x50]), MediaType::Unknown));
    }

    #[test]
    fn detect_magic_unknown() {
        let bytes = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Unknown));
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::detect::tests
```

**Step 3: Write minimal implementation** — code above IS the implementation.

Update `core/src/media/mod.rs`:

```rust
pub mod detect;
pub use detect::{detect_by_extension, detect_by_magic, detect_from_path};
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media::detect::tests
```

**Step 5: Commit**

```
media: add format detection via magic bytes and file extension
```

---

## Task 5: MediaPipeline Orchestrator

The central orchestrator that detects format, enforces policy, routes to providers with fallback.

**Files:**
- Create: `core/src/media/pipeline.rs`
- Modify: `core/src/media/mod.rs`

**Step 1: Write the failing test**

```rust
// In core/src/media/pipeline.rs
use crate::sync_primitives::Arc;
use super::error::MediaError;
use super::policy::MediaPolicy;
use super::provider::MediaProvider;
use super::types::{MediaInput, MediaOutput, MediaType};

/// Orchestrates media understanding across multiple providers.
///
/// The pipeline:
/// 1. Detects media format (if not already known)
/// 2. Enforces size/duration policy
/// 3. Routes to providers sorted by priority
/// 4. Falls back to next provider on failure
pub struct MediaPipeline {
    providers: Vec<Box<dyn MediaProvider>>,
    policy: MediaPolicy,
}

impl MediaPipeline {
    /// Create pipeline with default policy.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            policy: MediaPolicy::default(),
        }
    }

    /// Create pipeline with custom policy.
    pub fn with_policy(policy: MediaPolicy) -> Self {
        Self {
            providers: Vec::new(),
            policy,
        }
    }

    /// Get the policy.
    pub fn policy(&self) -> &MediaPolicy {
        &self.policy
    }

    /// Register a provider. Providers are sorted by priority on each call.
    pub fn add_provider(&mut self, provider: Box<dyn MediaProvider>) {
        self.providers.push(provider);
        self.providers.sort_by_key(|p| p.priority());
    }

    /// Number of registered providers.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Process media input through the pipeline.
    ///
    /// Detects type from input if needed, enforces policy, then tries
    /// providers in priority order.
    pub async fn process(
        &self,
        input: &MediaInput,
        media_type: &MediaType,
        prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError> {
        // 1. Policy check (file size checked by caller or here if path)
        if let MediaInput::FilePath { path } = input {
            if path.exists() {
                if let Ok(metadata) = std::fs::metadata(path) {
                    self.policy.check_size(media_type, metadata.len())?;
                }
            }
        }

        // 2. Find providers that support this media type
        let eligible: Vec<_> = self.providers.iter()
            .filter(|p| p.supports(media_type))
            .collect();

        if eligible.is_empty() {
            return Err(MediaError::NoProvider {
                media_type: media_type.category().to_string(),
            });
        }

        // 3. Try providers in priority order with fallback
        let mut last_err = MediaError::NoProvider {
            media_type: media_type.category().to_string(),
        };

        for provider in &eligible {
            match provider.process(input, media_type, prompt).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        error = %e,
                        "Media provider failed, trying next"
                    );
                    last_err = e;
                }
            }
        }

        Err(last_err)
    }

    /// List supported media categories across all providers.
    pub fn supported_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = self.providers
            .iter()
            .flat_map(|p| p.supported_types())
            .map(|t| t.category().to_string())
            .collect();
        categories.sort();
        categories.dedup();
        categories
    }
}

impl Default for MediaPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::*;
    use async_trait::async_trait;

    struct SuccessProvider { name: &'static str, priority: u8, category: &'static str }
    struct FailProvider { name: &'static str }

    fn image_type() -> MediaType {
        MediaType::Image { format: MediaImageFormat::Png, width: None, height: None }
    }
    fn audio_type() -> MediaType {
        MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None }
    }

    #[async_trait]
    impl MediaProvider for SuccessProvider {
        fn name(&self) -> &str { self.name }
        fn priority(&self) -> u8 { self.priority }
        fn supported_types(&self) -> Vec<MediaType> {
            match self.category {
                "image" => vec![image_type()],
                "audio" => vec![audio_type()],
                _ => vec![],
            }
        }
        async fn process(&self, _: &MediaInput, _: &MediaType, _: Option<&str>) -> Result<MediaOutput, MediaError> {
            Ok(MediaOutput::Description { text: format!("[{}] ok", self.name), confidence: 0.9 })
        }
    }

    #[async_trait]
    impl MediaProvider for FailProvider {
        fn name(&self) -> &str { self.name }
        fn supported_types(&self) -> Vec<MediaType> { vec![image_type()] }
        async fn process(&self, _: &MediaInput, _: &MediaType, _: Option<&str>) -> Result<MediaOutput, MediaError> {
            Err(MediaError::ProviderError { provider: self.name.into(), message: "mock failure".into() })
        }
    }

    fn sample_input() -> MediaInput {
        MediaInput::Url { url: "https://example.com/test.png".into() }
    }

    #[tokio::test]
    async fn empty_pipeline_returns_no_provider() {
        let pipeline = MediaPipeline::new();
        let err = pipeline.process(&sample_input(), &image_type(), None).await.unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }

    #[tokio::test]
    async fn single_provider_success() {
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider { name: "claude", priority: 10, category: "image" }));

        let result = pipeline.process(&sample_input(), &image_type(), Some("describe")).await.unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[claude]")),
            _ => panic!("Expected Description"),
        }
    }

    #[tokio::test]
    async fn fallback_on_failure() {
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(FailProvider { name: "primary" }));
        pipeline.add_provider(Box::new(SuccessProvider { name: "backup", priority: 50, category: "image" }));

        let result = pipeline.process(&sample_input(), &image_type(), None).await.unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[backup]")),
            _ => panic!("Expected Description from backup"),
        }
    }

    #[tokio::test]
    async fn skips_providers_without_matching_category() {
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider { name: "audio-only", priority: 1, category: "audio" }));
        pipeline.add_provider(Box::new(SuccessProvider { name: "image-handler", priority: 10, category: "image" }));

        let result = pipeline.process(&sample_input(), &image_type(), None).await.unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[image-handler]")),
            _ => panic!("Expected image-handler"),
        }
    }

    #[tokio::test]
    async fn priority_ordering() {
        let mut pipeline = MediaPipeline::new();
        // Add low priority first
        pipeline.add_provider(Box::new(SuccessProvider { name: "low", priority: 100, category: "image" }));
        // Add high priority second
        pipeline.add_provider(Box::new(SuccessProvider { name: "high", priority: 1, category: "image" }));

        let result = pipeline.process(&sample_input(), &image_type(), None).await.unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[high]"), "Expected high-priority provider, got: {}", text),
            _ => panic!("Expected Description"),
        }
    }

    #[test]
    fn supported_categories() {
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider { name: "a", priority: 10, category: "image" }));
        pipeline.add_provider(Box::new(SuccessProvider { name: "b", priority: 20, category: "audio" }));
        pipeline.add_provider(Box::new(SuccessProvider { name: "c", priority: 30, category: "image" }));

        let cats = pipeline.supported_categories();
        assert_eq!(cats, vec!["audio", "image"]);
    }

    #[test]
    fn provider_count() {
        let mut pipeline = MediaPipeline::new();
        assert_eq!(pipeline.provider_count(), 0);
        pipeline.add_provider(Box::new(SuccessProvider { name: "a", priority: 10, category: "image" }));
        assert_eq!(pipeline.provider_count(), 1);
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::pipeline::tests
```

**Step 3: Write minimal implementation** — code above IS the implementation.

Update `core/src/media/mod.rs`:

```rust
pub mod pipeline;
pub use pipeline::MediaPipeline;
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media::pipeline::tests
```

**Step 5: Commit**

```
media: add MediaPipeline orchestrator with priority-based provider fallback
```

---

## Task 6: ImageProcessor — Bridge to VisionPipeline

Adapter that wraps existing VisionPipeline as a MediaProvider.

**Files:**
- Create: `core/src/media/processors/mod.rs`
- Create: `core/src/media/processors/image.rs`
- Modify: `core/src/media/mod.rs`

**Step 1: Write the failing test**

```rust
// In core/src/media/processors/image.rs
//! Image processor — bridges existing VisionPipeline into the media pipeline.

use async_trait::async_trait;
use crate::sync_primitives::Arc;
use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::types::*;
use crate::vision::{VisionPipeline, VisionError};
use crate::vision::types::{ImageFormat as VisionImageFormat, ImageInput};

/// Bridges the existing [`VisionPipeline`] into the unified [`MediaProvider`] interface.
///
/// Converts MediaInput → ImageInput, delegates to VisionPipeline, converts results back.
pub struct ImageMediaProvider {
    pipeline: Arc<VisionPipeline>,
    priority: u8,
}

impl ImageMediaProvider {
    pub fn new(pipeline: Arc<VisionPipeline>, priority: u8) -> Self {
        Self { pipeline, priority }
    }

    /// Convert media image format to vision image format (best effort).
    fn to_vision_format(fmt: &MediaImageFormat) -> VisionImageFormat {
        match fmt {
            MediaImageFormat::Png => VisionImageFormat::Png,
            MediaImageFormat::Jpeg => VisionImageFormat::Jpeg,
            MediaImageFormat::WebP => VisionImageFormat::WebP,
            // Formats not directly supported by VisionPipeline — default to PNG
            MediaImageFormat::Gif | MediaImageFormat::Svg | MediaImageFormat::Heic => VisionImageFormat::Png,
        }
    }

    fn convert_input(input: &MediaInput, media_type: &MediaType) -> Result<ImageInput, MediaError> {
        match input {
            MediaInput::FilePath { path } => Ok(ImageInput::FilePath { path: path.clone() }),
            MediaInput::Url { url } => Ok(ImageInput::Url { url: url.clone() }),
            MediaInput::Base64 { data, media_type: mt } => {
                let format = match mt {
                    MediaType::Image { format, .. } => Self::to_vision_format(format),
                    _ => VisionImageFormat::Png,
                };
                Ok(ImageInput::Base64 { data: data.clone(), format })
            }
        }
    }
}

#[async_trait]
impl MediaProvider for ImageMediaProvider {
    fn name(&self) -> &str {
        "image-vision-bridge"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn supported_types(&self) -> Vec<MediaType> {
        vec![MediaType::Image { format: MediaImageFormat::Png, width: None, height: None }]
    }

    async fn process(&self, input: &MediaInput, media_type: &MediaType, prompt: Option<&str>) -> Result<MediaOutput, MediaError> {
        let image_input = Self::convert_input(input, media_type)?;

        if let Some(prompt_text) = prompt {
            // Use understand_image for prompted requests
            match self.pipeline.understand_image(&image_input, prompt_text).await {
                Ok(result) => Ok(MediaOutput::Description {
                    text: result.description,
                    confidence: result.confidence,
                }),
                Err(VisionError::NoProvider) => Err(MediaError::NoProvider {
                    media_type: "image".to_string(),
                }),
                Err(e) => Err(MediaError::ProviderError {
                    provider: "vision-pipeline".to_string(),
                    message: e.to_string(),
                }),
            }
        } else {
            // Use OCR for unprompted requests
            match self.pipeline.ocr(&image_input).await {
                Ok(result) => Ok(MediaOutput::Text { text: result.full_text }),
                Err(VisionError::NoProvider) => Err(MediaError::NoProvider {
                    media_type: "image".to_string(),
                }),
                Err(e) => Err(MediaError::ProviderError {
                    provider: "vision-pipeline".to_string(),
                    message: e.to_string(),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::types::{OcrLine, OcrResult, Rect, VisionCapabilities, VisionResult};
    use crate::vision::VisionProvider;

    struct MockVisionProvider;

    #[async_trait]
    impl VisionProvider for MockVisionProvider {
        async fn understand_image(&self, _image: &ImageInput, prompt: &str) -> Result<VisionResult, VisionError> {
            Ok(VisionResult { description: format!("Vision: {}", prompt), elements: vec![], confidence: 0.95 })
        }
        async fn ocr(&self, _image: &ImageInput) -> Result<OcrResult, VisionError> {
            Ok(OcrResult { full_text: "OCR extracted text".into(), lines: vec![] })
        }
        fn capabilities(&self) -> VisionCapabilities { VisionCapabilities::all() }
        fn name(&self) -> &str { "mock-vision" }
    }

    fn make_provider() -> ImageMediaProvider {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(MockVisionProvider));
        ImageMediaProvider::new(Arc::new(pipeline), 10)
    }

    #[tokio::test]
    async fn understand_with_prompt() {
        let p = make_provider();
        let input = MediaInput::Url { url: "https://example.com/img.png".into() };
        let mt = MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
        let result = p.process(&input, &mt, Some("what is this?")).await.unwrap();
        match result {
            MediaOutput::Description { text, confidence } => {
                assert!(text.contains("Vision: what is this?"));
                assert!((confidence - 0.95).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Description"),
        }
    }

    #[tokio::test]
    async fn ocr_without_prompt() {
        let p = make_provider();
        let input = MediaInput::Url { url: "https://example.com/img.png".into() };
        let mt = MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
        let result = p.process(&input, &mt, None).await.unwrap();
        match result {
            MediaOutput::Text { text } => assert_eq!(text, "OCR extracted text"),
            _ => panic!("Expected Text"),
        }
    }

    #[test]
    fn supports_image_category() {
        let p = make_provider();
        let png = MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
        let jpeg = MediaType::Image { format: MediaImageFormat::Jpeg, width: Some(100), height: Some(100) };
        let audio = MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None };
        assert!(p.supports(&png));
        assert!(p.supports(&jpeg));
        assert!(!p.supports(&audio));
    }

    #[test]
    fn format_conversion() {
        assert!(matches!(ImageMediaProvider::to_vision_format(&MediaImageFormat::Png), VisionImageFormat::Png));
        assert!(matches!(ImageMediaProvider::to_vision_format(&MediaImageFormat::Jpeg), VisionImageFormat::Jpeg));
        assert!(matches!(ImageMediaProvider::to_vision_format(&MediaImageFormat::Gif), VisionImageFormat::Png));
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::processors::image::tests
```

**Step 3: Write minimal implementation** — code above IS the implementation.

Create `core/src/media/processors/mod.rs`:

```rust
//! Media processors — concrete MediaProvider implementations.

pub mod image;
pub use image::ImageMediaProvider;
```

Update `core/src/media/mod.rs`:

```rust
pub mod processors;
pub use processors::ImageMediaProvider;
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media::processors::image::tests
```

**Step 5: Commit**

```
media: add ImageMediaProvider bridging VisionPipeline into media system
```

---

## Task 7: AudioProcessor Trait Stub + DocumentProcessor Stub

Define trait-only stubs for audio and document processing. No heavy deps.

**Files:**
- Create: `core/src/media/processors/audio.rs`
- Create: `core/src/media/processors/document.rs`
- Modify: `core/src/media/processors/mod.rs`

**Step 1: Write the failing test**

```rust
// In core/src/media/processors/audio.rs
//! Audio processor — stub MediaProvider for audio transcription.
//!
//! Actual processing is delegated to external API providers (e.g., Whisper).
//! This stub provides the trait interface for the media pipeline.

use async_trait::async_trait;
use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::types::*;

/// Placeholder audio provider that returns a "not configured" error.
///
/// Replace with actual Whisper API / MCP server integration.
pub struct AudioStubProvider;

#[async_trait]
impl MediaProvider for AudioStubProvider {
    fn name(&self) -> &str { "audio-stub" }

    fn priority(&self) -> u8 { 200 } // low priority stub

    fn supported_types(&self) -> Vec<MediaType> {
        vec![MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None }]
    }

    async fn process(&self, _input: &MediaInput, _media_type: &MediaType, _prompt: Option<&str>) -> Result<MediaOutput, MediaError> {
        Err(MediaError::NoProvider {
            media_type: "audio".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_stub_supports_audio() {
        let p = AudioStubProvider;
        assert!(p.supports(&MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None }));
        assert!(!p.supports(&MediaType::Image { format: MediaImageFormat::Png, width: None, height: None }));
    }

    #[tokio::test]
    async fn audio_stub_returns_no_provider() {
        let p = AudioStubProvider;
        let input = MediaInput::FilePath { path: "/tmp/test.mp3".into() };
        let mt = MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None };
        let err = p.process(&input, &mt, None).await.unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }
}
```

```rust
// In core/src/media/processors/document.rs
//! Document processor — text extraction for plain text and Markdown.
//!
//! Handles TXT and MD natively. PDF and DOCX/XLSX are deferred to plugins (P4).

use async_trait::async_trait;
use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::types::*;

/// Document provider for plain text formats (TXT, Markdown, HTML).
///
/// For formats requiring heavy parsing (PDF, DOCX, XLSX), this provider
/// returns UnsupportedFormat — those should be handled by plugin providers.
pub struct TextDocumentProvider;

#[async_trait]
impl MediaProvider for TextDocumentProvider {
    fn name(&self) -> &str { "text-document" }

    fn priority(&self) -> u8 { 10 }

    fn supported_types(&self) -> Vec<MediaType> {
        vec![MediaType::Document { format: DocFormat::Txt, pages: None }]
    }

    fn supports(&self, media_type: &MediaType) -> bool {
        matches!(media_type,
            MediaType::Document { format: DocFormat::Txt, .. }
            | MediaType::Document { format: DocFormat::Markdown, .. }
            | MediaType::Document { format: DocFormat::Html, .. }
        )
    }

    async fn process(&self, input: &MediaInput, media_type: &MediaType, _prompt: Option<&str>) -> Result<MediaOutput, MediaError> {
        match input {
            MediaInput::FilePath { path } => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| MediaError::IoError(format!("Failed to read {}: {}", path.display(), e)))?;
                Ok(MediaOutput::Text { text: content })
            }
            MediaInput::Base64 { data, .. } => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| MediaError::IoError(format!("Base64 decode error: {}", e)))?;
                let text = String::from_utf8(bytes)
                    .map_err(|e| MediaError::IoError(format!("UTF-8 decode error: {}", e)))?;
                Ok(MediaOutput::Text { text })
            }
            MediaInput::Url { .. } => {
                Err(MediaError::ProviderError {
                    provider: "text-document".into(),
                    message: "URL input not supported for text documents; use web_fetch tool first".into(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn supports_text_formats() {
        let p = TextDocumentProvider;
        assert!(p.supports(&MediaType::Document { format: DocFormat::Txt, pages: None }));
        assert!(p.supports(&MediaType::Document { format: DocFormat::Markdown, pages: None }));
        assert!(p.supports(&MediaType::Document { format: DocFormat::Html, pages: None }));
        assert!(!p.supports(&MediaType::Document { format: DocFormat::Pdf, pages: None }));
        assert!(!p.supports(&MediaType::Document { format: DocFormat::Docx, pages: None }));
        assert!(!p.supports(&MediaType::Image { format: MediaImageFormat::Png, width: None, height: None }));
    }

    #[tokio::test]
    async fn read_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut f = std::fs::File::create(&file_path).unwrap();
        write!(f, "Hello, world!").unwrap();

        let p = TextDocumentProvider;
        let input = MediaInput::FilePath { path: file_path };
        let mt = MediaType::Document { format: DocFormat::Txt, pages: None };
        let result = p.process(&input, &mt, None).await.unwrap();
        match result {
            MediaOutput::Text { text } => assert_eq!(text, "Hello, world!"),
            _ => panic!("Expected Text output"),
        }
    }

    #[tokio::test]
    async fn read_base64_text() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode("Test content");
        let p = TextDocumentProvider;
        let input = MediaInput::Base64 {
            data: encoded,
            media_type: MediaType::Document { format: DocFormat::Txt, pages: None },
        };
        let mt = MediaType::Document { format: DocFormat::Txt, pages: None };
        let result = p.process(&input, &mt, None).await.unwrap();
        match result {
            MediaOutput::Text { text } => assert_eq!(text, "Test content"),
            _ => panic!("Expected Text output"),
        }
    }

    #[tokio::test]
    async fn url_input_not_supported() {
        let p = TextDocumentProvider;
        let input = MediaInput::Url { url: "https://example.com/file.txt".into() };
        let mt = MediaType::Document { format: DocFormat::Txt, pages: None };
        let err = p.process(&input, &mt, None).await.unwrap_err();
        assert!(matches!(err, MediaError::ProviderError { .. }));
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::processors
```

**Step 3: Write minimal implementation** — code above IS the implementation.

Update `core/src/media/processors/mod.rs`:

```rust
pub mod audio;
pub mod document;
pub mod image;

pub use audio::AudioStubProvider;
pub use document::TextDocumentProvider;
pub use image::ImageMediaProvider;
```

Note: Ensure `base64` and `tempfile` (dev-dependency) crates are in Cargo.toml. `base64` is likely already present (common in this codebase). Check with `grep base64 core/Cargo.toml`.

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media::processors
```

**Step 5: Commit**

```
media: add AudioStubProvider and TextDocumentProvider processors
```

---

## Task 8: `media_understand` Tool

Unified entry tool that auto-detects type and routes through MediaPipeline.

**Files:**
- Create: `core/src/builtin_tools/media_tools/mod.rs`
- Create: `core/src/builtin_tools/media_tools/understand.rs`
- Modify: `core/src/builtin_tools/mod.rs` (add `pub mod media_tools;`)

**Step 1: Write the failing test**

```rust
// In core/src/builtin_tools/media_tools/understand.rs
//! `media_understand` tool — unified media understanding entry point.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use crate::error::Result;
use crate::tools::AlephTool;
use crate::media::{MediaPipeline, MediaType, MediaOutput, MediaInput};
use crate::media::detect::{detect_by_extension, detect_from_path};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaUnderstandArgs {
    /// Path to a local media file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,

    /// URL to a remote media resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Base64-encoded media data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64_data: Option<String>,

    /// File extension hint (e.g., "png", "mp3") when using base64 or URL without extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_hint: Option<String>,

    /// Natural-language prompt (e.g., "Describe this image", "Summarize this document").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaUnderstandOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl MediaUnderstandOutput {
    fn ok(media_type: &str, data: Value) -> Self {
        Self { success: true, media_type: Some(media_type.into()), message: None, data: Some(data) }
    }
    fn err(msg: impl Into<String>) -> Self {
        Self { success: false, media_type: None, message: Some(msg.into()), data: None }
    }
}

#[derive(Clone)]
pub struct MediaUnderstandTool {
    pipeline: Arc<MediaPipeline>,
}

impl MediaUnderstandTool {
    pub fn new(pipeline: Arc<MediaPipeline>) -> Self {
        Self { pipeline }
    }
}

#[async_trait]
impl AlephTool for MediaUnderstandTool {
    const NAME: &'static str = "media_understand";
    const DESCRIPTION: &'static str = r#"Understand media content (images, audio, video, documents).

Auto-detects the media type and routes to the appropriate processor.
Provide exactly one of: file_path, url, or base64_data.

Parameters:
- file_path: Path to a local file
- url: URL to remote media
- base64_data: Base64-encoded data (requires format_hint)
- format_hint: File extension hint (e.g., "png", "mp3", "pdf")
- prompt: What to extract or describe (optional)

Examples:
{"file_path":"/tmp/photo.jpg","prompt":"Describe this image"}
{"file_path":"/tmp/meeting.mp3","prompt":"Transcribe this audio"}
{"file_path":"/tmp/report.pdf"}"#;

    type Args = MediaUnderstandArgs;
    type Output = MediaUnderstandOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // 1. Build MediaInput from args
        let (input, media_type) = match (&args.file_path, &args.url, &args.base64_data) {
            (Some(path), None, None) => {
                let path = PathBuf::from(path);
                let mt = match detect_from_path(&path) {
                    Ok(mt) => mt,
                    Err(e) => return Ok(MediaUnderstandOutput::err(format!("Format detection failed: {}", e))),
                };
                (MediaInput::FilePath { path }, mt)
            }
            (None, Some(url), None) => {
                let mt = args.format_hint.as_deref()
                    .or_else(|| {
                        url.rsplit('/').next()
                            .and_then(|name| name.rsplit('.').next())
                            .filter(|ext| !ext.contains('?') && ext.len() < 10)
                    })
                    .map(detect_by_extension)
                    .transpose()
                    .unwrap_or(Ok(MediaType::Unknown))
                    .unwrap_or(MediaType::Unknown);
                (MediaInput::Url { url: url.clone() }, mt)
            }
            (None, None, Some(data)) => {
                let mt = args.format_hint.as_deref()
                    .map(detect_by_extension)
                    .unwrap_or(Ok(MediaType::Unknown))
                    .unwrap_or(MediaType::Unknown);
                (MediaInput::Base64 { data: data.clone(), media_type: mt.clone() }, mt)
            }
            _ => {
                return Ok(MediaUnderstandOutput::err(
                    "Provide exactly one of: file_path, url, or base64_data"
                ));
            }
        };

        if matches!(media_type, MediaType::Unknown) {
            return Ok(MediaUnderstandOutput::err(
                "Cannot detect media format. Provide a format_hint parameter."
            ));
        }

        // 2. Process through pipeline
        let category = media_type.category().to_string();
        match self.pipeline.process(&input, &media_type, args.prompt.as_deref()).await {
            Ok(output) => {
                let data = serde_json::to_value(&output).unwrap_or_default();
                Ok(MediaUnderstandOutput::ok(&category, data))
            }
            Err(e) => Ok(MediaUnderstandOutput::err(format!("Media processing failed: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::processors::ImageMediaProvider;
    use crate::vision::{VisionPipeline, VisionProvider, VisionError};
    use crate::vision::types::*;

    struct MockVision;

    #[async_trait]
    impl VisionProvider for MockVision {
        async fn understand_image(&self, _: &ImageInput, prompt: &str) -> std::result::Result<VisionResult, VisionError> {
            Ok(VisionResult { description: format!("Saw: {}", prompt), elements: vec![], confidence: 0.9 })
        }
        async fn ocr(&self, _: &ImageInput) -> std::result::Result<OcrResult, VisionError> {
            Ok(OcrResult { full_text: "OCR text".into(), lines: vec![] })
        }
        fn capabilities(&self) -> VisionCapabilities { VisionCapabilities::all() }
        fn name(&self) -> &str { "mock" }
    }

    fn make_tool() -> MediaUnderstandTool {
        let mut vp = VisionPipeline::new();
        vp.add_provider(Box::new(MockVision));
        let mut mp = MediaPipeline::new();
        mp.add_provider(Box::new(ImageMediaProvider::new(Arc::new(vp), 10)));
        MediaUnderstandTool::new(Arc::new(mp))
    }

    #[tokio::test]
    async fn no_input_returns_error() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None, url: None, base64_data: None,
            format_hint: None, prompt: None,
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("exactly one"));
    }

    #[tokio::test]
    async fn url_with_extension() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None,
            url: Some("https://example.com/photo.png".into()),
            base64_data: None,
            format_hint: None,
            prompt: Some("describe".into()),
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.media_type.as_deref(), Some("image"));
    }

    #[tokio::test]
    async fn base64_with_format_hint() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None, url: None,
            base64_data: Some("iVBORw0KGgo=".into()),
            format_hint: Some("png".into()),
            prompt: Some("what is this?".into()),
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn unknown_format_returns_error() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None, url: None,
            base64_data: Some("abc".into()),
            format_hint: None,
            prompt: None,
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("format"));
    }

    #[test]
    fn tool_definition() {
        let tool = make_tool();
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "media_understand");
        assert!(def.description.contains("media"));
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib builtin_tools::media_tools::understand::tests
```

**Step 3: Write minimal implementation** — code above IS the implementation.

Create `core/src/builtin_tools/media_tools/mod.rs`:

```rust
//! Media tools — builtin tools for unified media understanding.

pub mod understand;

pub use understand::{MediaUnderstandArgs, MediaUnderstandOutput, MediaUnderstandTool};
```

Add to `core/src/builtin_tools/mod.rs`:

```rust
pub mod media_tools;
pub use media_tools::{MediaUnderstandArgs, MediaUnderstandOutput, MediaUnderstandTool};
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib builtin_tools::media_tools::understand::tests
```

**Step 5: Commit**

```
media: add media_understand unified tool with auto-detection
```

---

## Task 9: Enhanced Vision Tool — Add Chart Extraction Mode

Add `chart_extract` action to existing VisionTool.

**Files:**
- Modify: `core/src/builtin_tools/vision.rs`

**Step 1: Write the failing test**

Add to VisionAction enum:

```rust
/// Extract structured data from charts/graphs in an image.
ChartExtract,
```

Add test:

```rust
#[tokio::test]
async fn test_chart_extract_success() {
    let tool = make_tool();
    let mut args = make_args(VisionAction::ChartExtract);
    args.prompt = Some("Extract data from this bar chart".to_string());

    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(output.success);
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib builtin_tools::vision::tests::test_chart_extract
```

**Step 3: Write minimal implementation**

In the `call` method's match, add:

```rust
VisionAction::ChartExtract => {
    let prompt = match args.prompt {
        Some(p) if !p.is_empty() => p,
        _ => "Extract all data, labels, and values from this chart. Return structured data.".to_string(),
    };

    match self.pipeline.understand_image(&image, &prompt).await {
        Ok(result) => {
            let data = serde_json::to_value(&result).unwrap_or_default();
            Ok(VisionOutput::ok_data("Chart data extracted", data))
        }
        Err(e) => Ok(VisionOutput::err(format!("Chart extraction failed: {e}"))),
    }
}
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib builtin_tools::vision::tests
```

**Step 5: Commit**

```
vision: add chart_extract action to VisionTool for structured data extraction
```

---

## Task 10: Register Media Tools in Builtin Registry

Add `media_understand` to `BUILTIN_TOOL_DEFINITIONS` and `create_tool_boxed`.

**Files:**
- Modify: `core/src/executor/builtin_registry/definitions.rs`
- Modify: `core/src/executor/builtin_registry/config.rs` (add media pipeline to BuiltinToolConfig)

**Step 1: Write the failing test**

```rust
#[test]
fn test_media_understand_in_definitions() {
    assert!(BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == "media_understand"));
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib executor::builtin_registry -- test_media_understand
```

**Step 3: Write minimal implementation**

Add to `BUILTIN_TOOL_DEFINITIONS`:

```rust
BuiltinToolDefinition {
    name: "media_understand",
    description: "Understand media content (images, audio, video, documents) with auto-detection and multi-provider fallback",
    requires_config: true, // Requires MediaPipeline
},
```

Add to `BuiltinToolConfig`:

```rust
pub media_pipeline: Option<Arc<MediaPipeline>>,
```

Add to `create_tool_boxed` match:

```rust
"media_understand" => {
    config.and_then(|c| c.media_pipeline.as_ref()).map(|pipeline| {
        Box::new(MediaUnderstandTool::new(Arc::clone(pipeline))) as Box<dyn AlephToolDyn>
    })
}
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib executor::builtin_registry
```

**Step 5: Commit**

```
media: register media_understand tool in builtin registry
```

---

## Task 11: MediaConfig in aleph.toml

Add `[media]` config section with MediaPolicy settings.

**Files:**
- Create: `core/src/config/types/media.rs` (or add to existing config types)
- Modify: `core/src/config/structs.rs` (add `media` field to Config)

**Step 1: Write the failing test**

```rust
#[test]
fn media_config_defaults() {
    let config = MediaConfig::default();
    assert_eq!(config.policy.max_image_bytes, 20 * 1024 * 1024);
    assert!(config.enabled);
}

#[test]
fn media_config_serde_round_trip() {
    let config = MediaConfig::default();
    let json = serde_json::to_value(&config).unwrap();
    let _: MediaConfig = serde_json::from_value(json).unwrap();
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib config -- media_config
```

**Step 3: Write minimal implementation**

```rust
// In core/src/config/types/media.rs (or wherever config types live)
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::media::policy::MediaPolicy;

/// Media understanding pipeline configuration.
///
/// ```toml
/// [media]
/// enabled = true
///
/// [media.policy]
/// max_image_bytes = 20971520
/// max_audio_bytes = 104857600
/// max_video_duration = 1800
/// max_document_pages = 200
/// temp_ttl_secs = 3600
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaConfig {
    /// Whether the media pipeline is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Size and lifecycle policy.
    #[serde(default)]
    pub policy: MediaPolicy,
}

fn default_true() -> bool { true }

impl Default for MediaConfig {
    fn default() -> Self {
        Self { enabled: true, policy: MediaPolicy::default() }
    }
}
```

Add to `Config` struct:

```rust
/// Media understanding pipeline configuration
#[serde(default)]
pub media: MediaConfig,
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib config -- media_config
```

**Step 5: Commit**

```
config: add [media] section for media pipeline policy configuration
```

---

## Task 12: lib.rs Exports + Integration Test

Wire up public exports and add a simple integration test.

**Files:**
- Modify: `core/src/lib.rs` (add media exports)
- Modify: `core/src/media/mod.rs` (final re-exports)

**Step 1: Write the failing test**

```rust
// In core/src/media/mod.rs, add integration test section:
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use crate::vision::{VisionPipeline, VisionProvider, VisionError};
    use crate::vision::types::*;
    use async_trait::async_trait;

    struct MockVision;

    #[async_trait]
    impl VisionProvider for MockVision {
        async fn understand_image(&self, _: &ImageInput, prompt: &str) -> std::result::Result<VisionResult, VisionError> {
            Ok(VisionResult { description: format!("Described: {}", prompt), elements: vec![], confidence: 0.9 })
        }
        async fn ocr(&self, _: &ImageInput) -> std::result::Result<OcrResult, VisionError> {
            Ok(OcrResult { full_text: "Extracted text".into(), lines: vec![] })
        }
        fn capabilities(&self) -> VisionCapabilities { VisionCapabilities::all() }
        fn name(&self) -> &str { "mock" }
    }

    #[tokio::test]
    async fn full_pipeline_image_understand() {
        // 1. Build vision pipeline
        let mut vp = VisionPipeline::new();
        vp.add_provider(Box::new(MockVision));

        // 2. Build media pipeline with image provider
        let mut mp = MediaPipeline::new();
        mp.add_provider(Box::new(processors::ImageMediaProvider::new(Arc::new(vp), 10)));

        // 3. Detect format
        let mt = detect::detect_by_extension("png").unwrap();
        assert_eq!(mt.category(), "image");

        // 4. Process
        let input = MediaInput::Url { url: "https://example.com/photo.png".into() };
        let result = mp.process(&input, &mt, Some("describe this")).await.unwrap();

        match result {
            MediaOutput::Description { text, confidence } => {
                assert!(text.contains("Described"));
                assert!(confidence > 0.0);
            }
            _ => panic!("Expected Description output"),
        }
    }

    #[tokio::test]
    async fn full_pipeline_text_document() {
        let mut mp = MediaPipeline::new();
        mp.add_provider(Box::new(processors::TextDocumentProvider));

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("readme.md");
        std::fs::write(&file_path, "# Hello\n\nWorld").unwrap();

        let mt = detect::detect_by_extension("md").unwrap();
        let input = MediaInput::FilePath { path: file_path };
        let result = mp.process(&input, &mt, None).await.unwrap();

        match result {
            MediaOutput::Text { text } => {
                assert!(text.contains("# Hello"));
                assert!(text.contains("World"));
            }
            _ => panic!("Expected Text output"),
        }
    }

    #[tokio::test]
    async fn unsupported_media_type_returns_error() {
        let mp = MediaPipeline::new(); // empty
        let input = MediaInput::Url { url: "https://example.com/video.mp4".into() };
        let mt = detect::detect_by_extension("mp4").unwrap();
        let err = mp.process(&input, &mt, None).await.unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib media::integration_tests
```

**Step 3: Write minimal implementation**

Add to `core/src/lib.rs` in the "Vision & Generation Exports" section:

```rust
// Media Pipeline Exports
pub use crate::media::{
    AudioFormat, DocFormat, MediaChunk, MediaError, MediaImageFormat, MediaInput,
    MediaOutput, MediaPipeline, MediaPolicy, MediaProvider, MediaType, VideoFormat,
};
```

Final `core/src/media/mod.rs`:

```rust
//! Media understanding pipeline — unified interface for image, audio, video, and document processing.
//!
//! # Architecture
//!
//! The media system follows the same pattern as the [`vision`](crate::vision) module:
//! a pipeline orchestrator with pluggable providers and fallback chains.
//!
//! Core defines traits only (per R1/R3). Heavy processing (ffmpeg, DOCX parsing)
//! is delegated to external plugins or API providers.
//!
//! # Components
//!
//! - [`MediaType`] — detected media type with format-specific metadata
//! - [`MediaProvider`] — trait for pluggable media processing backends
//! - [`MediaPipeline`] — orchestrator with priority-based provider fallback
//! - [`MediaPolicy`] — size and lifecycle enforcement
//! - [`detect`] — format detection from magic bytes and file extension

pub mod detect;
pub mod error;
pub mod pipeline;
pub mod policy;
pub mod processors;
pub mod provider;
pub mod types;

pub use detect::{detect_by_extension, detect_by_magic, detect_from_path};
pub use error::MediaError;
pub use pipeline::MediaPipeline;
pub use policy::MediaPolicy;
pub use processors::{AudioStubProvider, ImageMediaProvider, TextDocumentProvider};
pub use provider::MediaProvider;
pub use types::{
    AudioFormat, DocFormat, MediaChunk, MediaImageFormat, MediaInput, MediaOutput, MediaType,
    VideoFormat,
};
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib media
```

**Step 5: Commit**

```
media: wire up lib.rs exports and add integration tests
```

---

## Task 13: audio_transcribe + document_extract Tool Stubs

Add the remaining two tools as thin wrappers over MediaPipeline.

**Files:**
- Create: `core/src/builtin_tools/media_tools/transcribe.rs`
- Create: `core/src/builtin_tools/media_tools/extract.rs`
- Modify: `core/src/builtin_tools/media_tools/mod.rs`

These follow the exact same pattern as `media_understand` but with narrower scope:

- `audio_transcribe`: Only accepts audio formats, passes `prompt="transcribe"` to pipeline
- `document_extract`: Only accepts document formats, passes `prompt=None` (text extraction)

The implementation is straightforward -- each tool validates the media type category, then delegates to `MediaPipeline::process`. Tests follow the same mock pattern as Task 8.

**Step 5: Commit**

```
media: add audio_transcribe and document_extract tool stubs
```

---

## Task 14: Register All Media Tools + Final Cleanup

Register `audio_transcribe` and `document_extract` in `BUILTIN_TOOL_DEFINITIONS`, run full test suite.

**Files:**
- Modify: `core/src/executor/builtin_registry/definitions.rs`

**Step 1: Run all tests**

```bash
cargo test -p alephcore --lib media
cargo test -p alephcore --lib builtin_tools::media_tools
cargo test -p alephcore --lib builtin_tools::vision
cargo check -p alephcore
just clippy
```

**Step 5: Commit**

```
media: register all media tools and finalize P3 media pipeline
```

---

## Summary

| Task | Component | New Files | Key Dependency |
|------|-----------|-----------|----------------|
| 1 | MediaType + Format enums | `media/types.rs`, `media/mod.rs` | None |
| 2 | MediaError + MediaProvider | `media/error.rs`, `media/provider.rs` | Task 1 |
| 3 | MediaPolicy | `media/policy.rs` | Task 2 |
| 4 | Format detection | `media/detect.rs` | Task 1 |
| 5 | MediaPipeline orchestrator | `media/pipeline.rs` | Tasks 2-4 |
| 6 | ImageMediaProvider | `media/processors/image.rs` | Task 5 + VisionPipeline |
| 7 | Audio + Document stubs | `media/processors/audio.rs`, `document.rs` | Task 5 |
| 8 | `media_understand` tool | `builtin_tools/media_tools/understand.rs` | Tasks 5-7 |
| 9 | Enhanced VisionTool | Modify `builtin_tools/vision.rs` | None |
| 10 | Register in builtin registry | Modify `executor/builtin_registry/` | Task 8 |
| 11 | MediaConfig in aleph.toml | Config types + structs | Task 3 |
| 12 | lib.rs exports + integration | Modify `lib.rs`, `media/mod.rs` | All |
| 13 | audio_transcribe + document_extract | `media_tools/transcribe.rs`, `extract.rs` | Task 8 |
| 14 | Final registration + cleanup | Modify registry | Task 13 |

### Critical Files for Implementation

- `/Users/zouguojun/Workspace/Aleph/core/src/media/mod.rs` - New module root: exports all media types, traits, pipeline, processors
- `/Users/zouguojun/Workspace/Aleph/core/src/media/pipeline.rs` - Core orchestrator: format detection, policy enforcement, provider fallback chain
- `/Users/zouguojun/Workspace/Aleph/core/src/media/processors/image.rs` - Critical bridge: adapts existing VisionPipeline into unified MediaProvider interface
- `/Users/zouguojun/Workspace/Aleph/core/src/builtin_tools/media_tools/understand.rs` - Primary tool: unified entry point for AI agent media understanding
- `/Users/zouguojun/Workspace/Aleph/core/src/executor/builtin_registry/definitions.rs` - Registration: where new media tools get wired into the agent's tool palette