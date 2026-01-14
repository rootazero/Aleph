//! Core types module
//!
//! This module provides shared type definitions used across the Aether library:
//! - `CapturedContext`: Context from active application
//! - `MediaAttachment`: Multimodal content support
//! - `CompressionStats`: Memory compression statistics
//! - `MemoryEntryFFI`: Memory entry for FFI
//! - `AppMemoryInfo`: App memory info for UI
//!
//! Note: The main AetherCore interface is in `uniffi_core` module.

pub mod types;

// Re-export public types for external use
pub use types::{
    AppMemoryInfo, CapturedContext, CompressionStats, MediaAttachment, MemoryEntryFFI,
};
