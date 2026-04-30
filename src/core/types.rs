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

impl fmt::Debug for MediaAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaAttachment")
            .field("media_type", &self.media_type)
            .field("mime_type", &self.mime_type)
            .field("data", &format!("<REDACTED: {} bytes>", self.data.len()))
            .field("encoding", &self.encoding)
            .field("filename", &self.filename)
            .field("size_bytes", &self.size_bytes)
            .finish()
    }
}

/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone)]
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
