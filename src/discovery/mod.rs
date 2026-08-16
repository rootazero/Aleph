//! Discovery Module - Component Discovery System
//!
//! Unified discovery for configuration files, skills, commands, agents, and
//! plugins across multiple directories.

mod paths;
mod scanner;
mod types;

pub use paths::*;
pub use scanner::*;
pub use types::*;

use std::path::PathBuf;
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

/// Discovery Manager - main entry point for the discovery system
#[derive(Debug)]
pub struct DiscoveryManager {
    #[allow(dead_code)]
    config: DiscoveryConfig,
    scanner: DirectoryScanner,
}

impl DiscoveryManager {
    /// Create a new discovery manager
    pub fn new(config: DiscoveryConfig) -> DiscoveryResult<Self> {
        let scanner = DirectoryScanner::new(&config)?;
        Ok(Self { config, scanner })
    }

    /// Get the Aleph home directory (~/.aleph/)
    pub fn aleph_home(&self) -> DiscoveryResult<PathBuf> {
        aleph_home_dir()
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

    /// Discover plugins from `~/.aleph/plugins/` plus each supplied extra
    /// plugin-parent directory (e.g. registered projects' `.aleph/plugins`),
    /// so project-local installs are discovered alongside the global ones.
    pub fn discover_plugins_with_extra(
        &self,
        extra_parents: &[PathBuf],
    ) -> DiscoveryResult<Vec<DiscoveredPath>> {
        self.scanner.discover_plugins_with_extra(extra_parents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_config_default() {
        let config = DiscoveryConfig::default();
        assert!(config.scan_claude_dirs);
        assert!(config.scan_project_dirs);
        assert_eq!(config.max_upward_depth, 10);
    }
}
