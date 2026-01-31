//! Shared type definitions for the core module
//!
//! This module contains all shared types used across the codebase:
//! - MediaAttachment: Multimodal content support
//! - CapturedContext: Context from active application
//! - CompressionStats: Memory compression statistics
//! - MemoryEntry: Memory entry for API responses
//! - AppMemoryInfo: App memory info for UI

use serde::{Deserialize, Serialize};

/// Media attachment for multimodal content (add-multimodal-content-support)
/// Supports images, videos, and documents from clipboard
///
/// # Encoding
/// The `data` field format depends on the `encoding` field:
/// - "base64": Binary content encoded as Base64 (images, PDFs)
/// - "utf8": Plain text content (markdown, txt, extracted text)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    pub media_type: String,       // "image", "document", "video", "file"
    pub mime_type: String,        // "image/png", "text/markdown", "application/pdf", etc.
    pub data: String,             // Content (format depends on `encoding` field)
    pub encoding: String,         // "base64" | "utf8" - specifies data format
    pub filename: Option<String>, // Optional original filename
    pub size_bytes: u64,          // Original size in bytes for logging/validation
}

/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone)]
pub struct CapturedContext {
    pub app_bundle_id: String,
    pub window_title: Option<String>,
    pub attachments: Option<Vec<MediaAttachment>>, // Multimodal content support
    pub topic_id: Option<String>,                  // Topic ID for multi-turn conversations
}

/// Statistics about memory compression state
///
/// Used for displaying compression status in Settings UI
#[derive(Debug, Clone)]
pub struct CompressionStats {
    /// Total number of raw memories (Layer 1)
    pub total_raw_memories: u64,
    /// Total number of compressed facts (Layer 2)
    pub total_facts: u64,
    /// Number of valid (non-invalidated) facts
    pub valid_facts: u64,
    /// Breakdown by fact type (preference, plan, learning, etc.)
    pub facts_by_type: std::collections::HashMap<String, u64>,
}

/// Memory entry for API responses
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub app_bundle_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
    pub similarity_score: Option<f32>,
}

/// App memory info for UI filtering
#[derive(Debug, Clone)]
pub struct AppMemoryInfo {
    pub app_bundle_id: String,
    pub memory_count: u64,
}
