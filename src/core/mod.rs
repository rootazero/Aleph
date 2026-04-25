//! Core types module
//!
//! This module provides shared type definitions used across the Aleph library:
//! - `CapturedContext`: Context from active application
//! - `MediaAttachment`: Multimodal content support
//! - `CompressionStats`: Memory compression statistics
//! - `MemoryEntry`: Memory storage entry type

pub mod types;

// Re-export public types for external use
pub use types::{CapturedContext, CompressionStats, MediaAttachment, MemoryEntry};
