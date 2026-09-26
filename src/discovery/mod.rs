//! Discovery Module - Component Discovery System
//!
//! Unified discovery for configuration files, skills, commands, agents, and
//! plugins across multiple directories.

mod claude_cache;
mod paths;
mod scanner;
mod types;

pub use paths::*;
pub use types::*;

use scanner::DirectoryScanner;

use std::path::PathBuf;
use thiserror::Error;

/// Discovery errors
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("failed to resolve Aleph home directory")]
    HomeDir(#[source] crate::error::AlephError),
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

    /// Replaces `~/.claude` — the whole Claude root: its `skills` /
    /// `commands` / `agents` and Claude Code's plugin cache — **test-only**.
    ///
    /// `$HOME` is process-global, so without this a test that needs a Claude
    /// root of its own either fights every sibling for the environment or
    /// reads the real one. Gated on `cfg(test)` like
    /// `ExtensionConfig::extra_plugin_parents`: production has one Claude
    /// root, and a knob no production code sets is not a feature (R10).
    #[cfg(test)]
    pub claude_home_override: Option<PathBuf>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            scan_claude_dirs: true,
            scan_project_dirs: true,
            max_upward_depth: 10,
            #[cfg(test)]
            claude_home_override: None,
        }
    }
}

/// Discovery Manager - main entry point for the discovery system
#[derive(Debug)]
pub struct DiscoveryManager {
    scanner: DirectoryScanner,
}

impl DiscoveryManager {
    /// Create a new discovery manager
    pub fn new(config: DiscoveryConfig) -> DiscoveryResult<Self> {
        let scanner = DirectoryScanner::new(&config)?;
        Ok(Self { scanner })
    }

    /// Get the Aleph home directory (~/.aleph/)
    ///
    /// Returns the path resolved at construction (honouring the `ALEPH_HOME`
    /// override), so it cannot disagree with the scanner's own view.
    ///
    /// Infallible: the scanner caches the path at construction (resolving
    /// `ALEPH_HOME` or defaulting to `~/.aleph`), and any failure that
    /// could occur here would already have prevented the scanner from
    /// being built. Returning `DiscoveryResult` here just pushed `?` onto
    /// callers for an error that cannot happen.
    #[must_use]
    pub fn aleph_home(&self) -> PathBuf {
        self.scanner.aleph_home().to_path_buf()
    }

    /// Discover all skill directories
    pub fn discover_skill_dirs(&self) -> DiscoveryResult<Vec<DiscoveredPath>> {
        self.scanner.discover_component("skills")
    }

    /// Discover plugins from `~/.aleph/plugins/` plus each supplied extra
    /// plugin-parent directory (e.g. registered projects' `.aleph/plugins`),
    /// so project-local installs are discovered alongside the global ones.
    /// Each extra parent names the project it belongs to; every plugin found
    /// under it carries that root as its [`DiscoveryScope`].
    pub fn discover_plugins_with_extra(
        &self,
        extra_parents: &[ProjectPluginParent],
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
