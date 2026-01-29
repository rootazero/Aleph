//! Core types module
//!
//! This module provides shared type definitions used across the Aether library:
//! - `Capability`: Agent capability types (Memory, Mcp, Skills)
//! - `CapturedContext`: Context from active application
//! - `MediaAttachment`: Multimodal content support
//! - `CompressionStats`: Memory compression statistics
//! - `AppMemoryInfo`: App memory info for UI
//!
//! Note: `Capability` here is for agent capabilities, distinct from
//!       `cowork::model_router::Capability` which represents model capabilities.

pub mod capability;
pub mod types;

// Re-export public types for external use
pub use capability::Capability;
pub use types::{
    AppMemoryInfo, CapturedContext, CompressionStats, MediaAttachment, MemoryEntryFFI,
};
