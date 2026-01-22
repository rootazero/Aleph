//! Checkpoint system for file snapshots and rollback
//!
//! This module provides a checkpoint mechanism similar to Claude Code,
//! allowing users to revert file changes made during agent execution.
//!
//! # Key Features
//!
//! - **Automatic snapshots**: Files are snapshotted before modification
//! - **One-click rollback**: Restore all files to a previous checkpoint
//! - **Session-scoped**: Checkpoints are tied to agent sessions
//! - **Efficient storage**: Uses diff-based storage for large files
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::checkpoint::{CheckpointManager, CheckpointConfig};
//!
//! let mut manager = CheckpointManager::new(CheckpointConfig::default())?;
//!
//! // Before editing files
//! let checkpoint = manager.create_checkpoint(&[
//!     "/path/to/file1.rs",
//!     "/path/to/file2.rs",
//! ]).await?;
//!
//! // ... agent edits files ...
//!
//! // Rollback if needed
//! manager.rollback(&checkpoint.id).await?;
//! ```

mod manager;
mod snapshot;
mod storage;

pub use manager::{CheckpointConfig, CheckpointManager};
pub use snapshot::{Checkpoint, CheckpointId, CheckpointSummary, FileSnapshot};
pub use storage::{CheckpointStorage, MemoryStorage};
