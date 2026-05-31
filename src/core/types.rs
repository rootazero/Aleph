//! Shared type definitions for the core module
//!
//! This module contains all shared types used across the codebase:
//! - MediaAttachment: Multimodal content support
//! - CapturedContext: Context from active application
//! - CompressionStats: Memory compression statistics
//! - MemoryEntry: Memory entry for API responses

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Media type classification for attachments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    /// Image content (PNG, JPEG, GIF, etc.)
    Image,
    /// Document content (PDF, TXT, MD, etc.)
    Document,
    /// Video content (MP4, MOV, etc.)
    Video,
    /// Generic file attachment
    File,
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaType::Image => write!(f, "image"),
            MediaType::Document => write!(f, "document"),
            MediaType::Video => write!(f, "video"),
            MediaType::File => write!(f, "file"),
        }
    }
}

/// Content encoding format for attachment data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentEncoding {
    /// Binary content encoded as Base64 (images, PDFs)
    Base64,
    /// Plain text content (markdown, txt, extracted text)
    Utf8,
}

impl fmt::Display for ContentEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentEncoding::Base64 => write!(f, "base64"),
            ContentEncoding::Utf8 => write!(f, "utf8"),
        }
    }
}

/// Media attachment for multimodal content (add-multimodal-content-support)
/// Supports images, videos, and documents from clipboard
///
/// # Encoding
/// The `data` field format depends on the `encoding` field:
/// - "base64": Binary content encoded as Base64 (images, PDFs)
/// - "utf8": Plain text content (markdown, txt, extracted text)
#[derive(Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    /// Content type classification
    pub media_type: MediaType,
    /// MIME type, e.g. "image/png", "text/markdown", "application/pdf"
    pub mime_type: String,
    /// Content (format depends on `encoding` field)
    pub data: String,
    /// Encoding format: base64 or utf8
    pub encoding: ContentEncoding,
    /// Optional original filename
    pub filename: Option<String>,
    /// Original size in bytes for logging/validation
    pub size_bytes: u64,
}

impl MediaAttachment {
    /// Create a new media attachment with validated size.
    ///
    /// # Arguments
    /// * `media_type` - Content type classification
    /// * `mime_type` - MIME type (e.g. "image/png")
    /// * `data` - Content string (base64-encoded or utf8 text)
    /// * `encoding` - Encoding format
    /// * `filename` - Optional original filename
    pub fn new(
        media_type: MediaType,
        mime_type: impl Into<String>,
        data: impl Into<String>,
        encoding: ContentEncoding,
        filename: Option<String>,
    ) -> Self {
        let data = data.into();
        let size_bytes = match encoding {
            // Base64 encoding inflates size by ~33%; store original decoded size
            // Account for padding characters (=) at the end of base64 strings
            // Strip whitespace (newlines, spaces) before calculating to handle
            // PEM-style or RFC 2045 wrapped base64 strings correctly.
            ContentEncoding::Base64 => {
                let bytes = data.as_bytes();
                let clean_len = bytes.iter().filter(|&&b| !b.is_ascii_whitespace()).count();
                let padding = bytes
                    .iter()
                    .rev()
                    .filter(|&&b| !b.is_ascii_whitespace())
                    .take(2)
                    .filter(|&&b| b == b'=')
                    .count();
                ((clean_len.saturating_sub(padding)).saturating_mul(3) / 4) as u64
            }
            ContentEncoding::Utf8 => data.len() as u64,
        };
        Self {
            media_type,
            mime_type: mime_type.into(),
            data,
            encoding,
            filename,
            size_bytes,
        }
    }
}

impl fmt::Debug for MediaAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaAttachment")
            .field("media_type", &self.media_type)
            .field("mime_type", &self.mime_type)
            .field("data", &format!("<REDACTED: {} bytes>", self.size_bytes))
            .field("encoding", &self.encoding)
            .field("filename", &self.filename)
            .field("size_bytes", &self.size_bytes)
            .finish()
    }
}

/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapturedContext {
    /// Window title of the active application, if available
    pub window_title: Option<String>,
    /// Multimodal attachments from the active context
    pub attachments: Vec<MediaAttachment>,
    /// Session ID for multi-turn conversations
    pub session_id: Option<String>,
}

/// Statistics about memory compression state
///
/// Used for displaying compression status in Settings UI
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressionStats {
    /// Total number of raw memories (Layer 1)
    pub total_raw_memories: u64,
    /// Total number of compressed facts (Layer 2)
    pub total_facts: u64,
    /// Number of valid (non-invalidated) facts
    pub valid_facts: u64,
    /// Breakdown by fact type (preference, plan, learning, etc.)
    /// Uses BTreeMap for deterministic iteration order
    pub facts_by_type: BTreeMap<String, u64>,
}

/// Memory entry for API responses
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier for the memory entry
    pub id: String,
    /// Window title from the capture context
    pub window_title: String,
    /// Original user input text
    pub user_input: String,
    /// AI-generated response text
    pub ai_output: String,
    /// Unix timestamp in seconds since epoch
    pub timestamp: i64,
    /// Cosine similarity score from vector search, if applicable
    pub similarity_score: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_attachment_base64_empty() {
        let attachment = MediaAttachment::new(
            MediaType::Image,
            "image/png",
            "",
            ContentEncoding::Base64,
            None,
        );
        assert_eq!(attachment.size_bytes, 0);
    }

    #[test]
    fn media_attachment_base64_whitespace_only() {
        let attachment = MediaAttachment::new(
            MediaType::Image,
            "image/png",
            "   \n\t  ",
            ContentEncoding::Base64,
            None,
        );
        assert_eq!(attachment.size_bytes, 0);
    }

    #[test]
    fn media_attachment_base64_with_padding() {
        // "SGVsbG8=" is "Hello" in base64 (5 bytes)
        let attachment = MediaAttachment::new(
            MediaType::Image,
            "image/png",
            "SGVsbG8=",
            ContentEncoding::Base64,
            None,
        );
        assert_eq!(attachment.size_bytes, 5);
    }

    #[test]
    fn media_attachment_base64_without_padding() {
        // "SGVsbG8" is base64 without padding
        let attachment = MediaAttachment::new(
            MediaType::Image,
            "image/png",
            "SGVsbG8",
            ContentEncoding::Base64,
            None,
        );
        // (7 * 3) / 4 = 5 (integer division truncates)
        assert_eq!(attachment.size_bytes, 5);
    }

    #[test]
    fn media_attachment_utf8_multibyte() {
        let attachment = MediaAttachment::new(
            MediaType::Document,
            "text/plain",
            "\u{4f60}\u{597d}\u{4e16}\u{754c}",
            ContentEncoding::Utf8,
            None,
        );
        assert_eq!(attachment.size_bytes, 12); // 4 chars * 3 bytes each in UTF-8
    }

    #[test]
    fn media_attachment_utf8_ascii() {
        let attachment = MediaAttachment::new(
            MediaType::Document,
            "text/plain",
            "Hello",
            ContentEncoding::Utf8,
            None,
        );
        assert_eq!(attachment.size_bytes, 5);
    }

    #[test]
    fn memory_entry_default() {
        let entry = MemoryEntry::default();
        assert!(entry.id.is_empty());
        assert!(entry.window_title.is_empty());
        assert!(entry.user_input.is_empty());
        assert!(entry.ai_output.is_empty());
        assert_eq!(entry.timestamp, 0);
        assert!(entry.similarity_score.is_none());
    }
}
