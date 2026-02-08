//! Atomic Operations Module
//!
//! This module provides a modular, composition-based architecture for atomic operations.
//! It refactors the monolithic `atomic_executor.rs` into specialized handlers using the
//! Strategy Pattern.
//!
//! ## Architecture
//!
//! - **ExecutorContext**: Shared environment (working directory, security checks)
//! - **Handler Traits**: Strongly-typed contracts for each operation category
//! - **Handler Implementations**: Isolated, testable operation logic
//!
//! ## Design Principles
//!
//! 1. **Composition over Inheritance** - Use handler composition instead of monolithic implementation
//! 2. **Explicit over Implicit** - Dedicated method traits instead of generic handlers
//! 3. **Separation of Concerns** - Environment (Context) vs. Constraints (Handler-specific config)
//! 4. **Defense in Depth** - Centralized security checks in ExecutorContext

use async_trait::async_trait;
use crate::error::Result;

// Re-export types from sibling modules
use super::atomic_executor::AtomicResult;
use super::{
    LineRange, WriteMode, Patch, SearchPattern, SearchScope, FileFilter,
};

// Module declarations
pub mod context;
pub mod file;
pub mod edit;
pub mod bash;
pub mod search;

// Re-export ExecutorContext and handlers
pub use context::ExecutorContext;
pub use file::FileOpsHandler;
pub use edit::EditOpsHandler;
pub use bash::BashOpsHandler;
pub use search::SearchOpsHandler;

/// File operations trait
///
/// Handles file I/O operations: Read, Write, Move
#[async_trait]
pub trait FileOps: Send + Sync {
    /// Read file content with optional line range
    async fn read(&self, path: &str, range: Option<&LineRange>) -> Result<AtomicResult>;

    /// Write content to file with specified mode
    async fn write(&self, path: &str, content: &str, mode: &WriteMode) -> Result<AtomicResult>;

    /// Move file or directory with optional import updates
    async fn move_file(
        &self,
        source: &str,
        dest: &str,
        update_imports: bool,
        create_parent: bool,
    ) -> Result<AtomicResult>;
}

/// Edit operations trait
///
/// Handles text editing and replacement operations
#[async_trait]
pub trait EditOps: Send + Sync {
    /// Apply patches to a file
    async fn edit(&self, path: &str, patches: &[Patch]) -> Result<AtomicResult>;

    /// Replace text across files with preview/dry-run support
    async fn replace(
        &self,
        search: &SearchPattern,
        replacement: &str,
        scope: &SearchScope,
        preview: bool,
        dry_run: bool,
    ) -> Result<AtomicResult>;
}

/// Bash operations trait
///
/// Handles shell command execution
#[async_trait]
pub trait BashOps: Send + Sync {
    /// Execute shell command with optional working directory
    async fn execute(&self, command: &str, cwd: Option<&str>) -> Result<AtomicResult>;
}

/// Search operations trait
///
/// Handles file search with pattern matching
#[async_trait]
pub trait SearchOps: Send + Sync {
    /// Search files with pattern and filters
    async fn search(
        &self,
        pattern: &SearchPattern,
        scope: &SearchScope,
        filters: &[FileFilter],
    ) -> Result<AtomicResult>;
}
