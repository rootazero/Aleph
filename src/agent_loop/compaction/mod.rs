//! Compaction orchestration — cascade degradation for the agent loop.
//!
//! This module provides the traits and types that drive context-window
//! compaction at multiple pressure levels:
//!
//! - [`types::PressureLevel`] — discrete pressure classification
//! - [`types::CompactionStrategy`] — pluggable compaction implementations
//! - [`types::PostCompactCleanup`] — hooks that run after each compaction pass
//! - [`types::CompactionContext`] / [`types::CompactionResult`] — data carriers

pub mod types;

pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PostCompactCleanup, PressureLevel,
    TokenEstimate,
};
