//! Discovery Module - Component Discovery System
//!
//! This module provides a unified discovery system for finding configuration files,
//! skills, commands, agents, and plugins across multiple directories.
//!
//! # Directory Strategy
//!
//! **Read Paths (by priority, later overrides earlier):**
//! - `~/.claude/skills/` - Claude Code compatible (read-only)
//! - `~/.claude/commands/` - Claude Code compatible (read-only)
//! - `~/.aether/skills/` - Aether native
//! - `~/.aether/commands/` - Aether native
//! - `~/.aether/plugins/` - Aether native
//! - `./.claude/skills/` - Project-level Claude Code (read-only)
//! - `./.claude/commands/` - Project-level Claude Code (read-only)
//!
//! **Write Paths:**
//! - Always use `~/.aether/` for writing
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::discovery::{DiscoveryManager, DiscoveryConfig};
//!
//! let config = DiscoveryConfig::default();
//! let manager = DiscoveryManager::new(config)?;
//!
//! // Discover all skills
//! let skills = manager.discover_skills()?;
//!
//! // Find config files with upward traversal
//! let configs = manager.find_config_files("aether.jsonc")?;
//! ```

mod paths;
mod scanner;
mod types;

pub use paths::*;
pub use scanner::*;
pub use types::*;

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Discovery errors
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Home directory not found")]
    HomeDirNotFound,

    #[error("Git root not found from: {0}")]
    GitRootNotFound(PathBuf),

    #[error("Parse error in {path}: {message}")]
    ParseError { path: PathBuf, message: String },
}

pub type DiscoveryResult<T> = Result<T, DiscoveryError>;

/// Discovery configuration
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Working directory (defaults to current directory)
    pub working_dir: PathBuf,

    /// Whether to scan Claude Code directories (.claude/)
    pub scan_claude_dirs: bool,

    /// Whether to scan project-level directories
    pub scan_project_dirs: bool,

    /// Maximum depth for upward directory traversal
    pub max_upward_depth: usize,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            scan_claude_dirs: true,
            scan_project_dirs: true,
            max_upward_depth: 10,
        }
    }
}

impl DiscoveryConfig {
    /// Create config with a specific working directory
    pub fn with_working_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.working_dir = dir.as_ref().to_path_buf();
        self
    }

    /// Disable Claude Code directory scanning
    pub fn without_claude_dirs(mut self) -> Self {
        self.scan_claude_dirs = false;
        self
    }

    /// Disable project-level directory scanning
    pub fn without_project_dirs(mut self) -> Self {
        self.scan_project_dirs = false;
        self
    }
}

/// Discovery Manager - main entry point for the discovery system
#[derive(Debug)]
pub struct DiscoveryManager {
    config: DiscoveryConfig,
    scanner: DirectoryScanner,
}

impl DiscoveryManager {
    /// Create a new discovery manager
    pub fn new(config: DiscoveryConfig) -> DiscoveryResult<Self> {
        let scanner = DirectoryScanner::new(&config)?;
        Ok(Self { config, scanner })
    }

    /// Create with default configuration
    pub fn with_defaults() -> DiscoveryResult<Self> {
        Self::new(DiscoveryConfig::default())
    }

    /// Get the Aether home directory (~/.aether/)
    pub fn aether_home(&self) -> DiscoveryResult<PathBuf> {
        aether_home_dir()
    }

    /// Get all directories to scan for components
    pub fn get_scan_directories(&self) -> DiscoveryResult<Vec<ScanDirectory>> {
        self.scanner.get_all_directories()
    }

    /// Find configuration files with upward traversal
    ///
    /// Searches from working directory up to git root or filesystem root.
    /// Returns paths in priority order (global first, project last).
    pub fn find_config_files(&self, filename: &str) -> DiscoveryResult<Vec<PathBuf>> {
        self.scanner.find_upward(filename)
    }

    /// Discover all skill directories
    pub fn discover_skill_dirs(&self) -> DiscoveryResult<Vec<DiscoveredPath>> {
        self.scanner.discover_component("skills")
    }

    /// Discover all command directories
    pub fn discover_command_dirs(&self) -> DiscoveryResult<Vec<DiscoveredPath>> {
        self.scanner.discover_component("commands")
    }

    /// Discover all agent directories
    pub fn discover_agent_dirs(&self) -> DiscoveryResult<Vec<DiscoveredPath>> {
        self.scanner.discover_component("agents")
    }

    /// Discover all plugin directories
    pub fn discover_plugin_dirs(&self) -> DiscoveryResult<Vec<DiscoveredPath>> {
        self.scanner.discover_component("plugins")
    }

    /// Get the git root directory if available
    pub fn git_root(&self) -> Option<&Path> {
        self.scanner.git_root()
    }

    /// Get the working directory
    pub fn working_dir(&self) -> &Path {
        &self.config.working_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_discovery_config_default() {
        let config = DiscoveryConfig::default();
        assert!(config.scan_claude_dirs);
        assert!(config.scan_project_dirs);
        assert_eq!(config.max_upward_depth, 10);
    }

    #[test]
    fn test_discovery_config_builder() {
        let temp = TempDir::new().unwrap();
        let config = DiscoveryConfig::default()
            .with_working_dir(temp.path())
            .without_claude_dirs();

        assert_eq!(config.working_dir, temp.path());
        assert!(!config.scan_claude_dirs);
    }
}
