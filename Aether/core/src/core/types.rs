//! Shared type definitions for the core module
//!
//! This module contains all shared types used across the codebase:
//! - MediaAttachment: Multimodal content support
//! - CapturedContext: Context from active application
//! - CompressionStats: Memory compression statistics
//! - MemoryEntryFFI: Memory entry for FFI
//! - AppMemoryInfo: App memory info for UI

/// Media attachment for multimodal content (add-multimodal-content-support)
/// Supports images, videos, and files from clipboard
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    pub media_type: String,   // "image", "video", "file"
    pub mime_type: String,    // "image/png", "image/jpeg", "video/mp4", etc.
    pub data: String,         // Base64-encoded content
    pub filename: Option<String>, // Optional original filename
    pub size_bytes: u64,      // Original size in bytes for logging/validation
}

/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone)]
pub struct CapturedContext {
    pub app_bundle_id: String,
    pub window_title: Option<String>,
    pub attachments: Option<Vec<MediaAttachment>>, // Multimodal content support
    pub topic_id: Option<String>, // Topic ID for multi-turn conversations
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

/// Memory entry type for FFI (UniFFI-compatible)
#[derive(Debug, Clone)]
pub struct MemoryEntryFFI {
    pub id: String,
    pub app_bundle_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
    pub similarity_score: Option<f32>,
}

/// App memory info for UI filtering (UniFFI-compatible)
#[derive(Debug, Clone)]
pub struct AppMemoryInfo {
    pub app_bundle_id: String,
    pub memory_count: u64,
}
