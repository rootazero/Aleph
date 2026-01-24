//! Tool Output Management
//!
//! This module provides utilities for managing tool output, including:
//! - Truncation of large outputs to prevent context overflow
//! - Storage of full outputs to files for later retrieval
//! - Automatic cleanup of old output files
//!
//! Inspired by OpenCode's tool/truncation.ts.

pub mod cleanup;
pub mod truncation;

pub use cleanup::{cleanup_old_outputs, start_cleanup_scheduler, CleanupConfig};
pub use truncation::{
    truncate_output, TruncatedOutput, TruncationConfig, TruncationDirection, TruncationResult,
};
