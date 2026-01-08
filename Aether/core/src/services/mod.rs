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
//! - `tools`: Tier 1 System Tools (JSON interface for LLM tool invocation)
//!
//! # Design Principles
//!
//! - **Zero External Dependencies**: All operations use pure Rust libraries
//! - **Async-First**: All operations are async using tokio
//! - **Trait-Based**: Abstract traits allow mock implementations for testing
//! - **Security-Aware**: Path validation and sandboxing built-in

pub mod fs;
pub mod git;
pub mod system_info;
pub mod tools;

// Re-export commonly used types
pub use fs::{DirEntry, FileOps, LocalFs};
pub use git::{GitCommit, GitDiff, GitFileStatus, GitOps, GitRepository};
pub use system_info::{MacOsSystemInfo, SystemInfo, SystemInfoProvider};

// Re-export system tools
pub use tools::{
    FsService, FsServiceConfig, GitService, GitServiceConfig, ShellService, ShellServiceConfig,
    SystemInfoService, SystemTool,
};
