//! Shared Foundation Services Module
//!
//! This module provides reusable services that can be used by tools, MCP, Skills, and
//! future extensions. All services are designed for zero external dependencies
//! and trait-based abstractions for testability.
//!
//! # Modules
//!
//! - `fs`: File system operations with path security
//! - `git`: Git repository operations using git2-rs (no CLI dependency)
//! - `system_info`: System information queries
//!
//! # Design Principles
//!
//! - **Zero External Dependencies**: All operations use pure Rust libraries
//! - **Async-First**: All operations are async using tokio
//! - **Trait-Based**: Abstract traits allow mock implementations for testing
//! - **Security-Aware**: Path validation and sandboxing built-in
//!
//! # Note
//!
//! Native tools (fs, git, shell, etc.) are now implemented via the `AgentTool` trait
//! in the `tools` module at the crate root. The old `SystemTool` trait has been removed.

pub mod fs;
pub mod git;
pub mod system_info;

// Re-export commonly used types
pub use fs::{DirEntry, FileOps, LocalFs};
pub use git::{GitCommit, GitDiff, GitFileStatus, GitOps, GitRepository};
pub use system_info::{MacOsSystemInfo, SystemInfo, SystemInfoProvider};
