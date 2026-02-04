//! Plugin Discovery System
//!
//! Implements 4-layer discovery with priority-based conflict resolution:
//! 1. Config-specified paths (highest)
//! 2. Workspace local
//! 3. Global user-level
//! 4. Bundled (lowest)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::extension::discovery::{DiscoveryConfig, discover_all};
//!
//! let config = DiscoveryConfig {
//!     workspace_dir: Some("/path/to/project".into()),
//!     ..Default::default()
//! };
//!
//! let resolved = discover_all(&config)?;
//! println!("Found {} active plugins", resolved.active.len());
//! println!("{} plugins were overridden", resolved.overridden.len());
//! ```

mod resolver;
mod scanner;

pub use resolver::{resolve_conflicts, ResolvedPlugins};
pub use scanner::{scan_directory, PluginCandidate};

use std::path::PathBuf;
use tracing::debug;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::types::PluginOrigin;

/// Discovery manager configuration
#[derive(Debug, Clone, Default)]
pub struct DiscoveryConfig {
    /// Extra paths from config (Priority 1)
    pub extra_paths: Vec<PathBuf>,
    /// Workspace root directory
    pub workspace_dir: Option<PathBuf>,
    /// User home directory override
    pub home_dir: Option<PathBuf>,
    /// Bundled plugins directory
    pub bundled_dir: Option<PathBuf>,
}

impl DiscoveryConfig {
    /// Create a new discovery config with workspace directory
    pub fn with_workspace(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: Some(workspace.into()),
            ..Default::default()
        }
    }

    /// Add an extra path to scan (config-specified, highest priority)
    pub fn add_extra_path(&mut self, path: impl Into<PathBuf>) {
        self.extra_paths.push(path.into());
    }

    /// Set the bundled plugins directory
    pub fn with_bundled_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.bundled_dir = Some(path.into());
        self
    }

    /// Set the home directory override (for testing)
    pub fn with_home_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.home_dir = Some(path.into());
        self
    }
}

/// Discover all plugins from all configured sources
///
/// Scans all 4 layers in order:
/// 1. Config-specified paths (highest priority)
/// 2. Workspace `.aleph/extensions` and `.claude/extensions`
/// 3. Global `~/.aleph/extensions` and `~/.claude/extensions`
/// 4. Bundled plugins (lowest priority)
///
/// Returns resolved plugins with conflicts resolved based on origin priority.
pub fn discover_all(config: &DiscoveryConfig) -> ExtensionResult<ResolvedPlugins> {
    let mut all_candidates = Vec::new();

    // Priority 1: Config-specified paths
    for path in &config.extra_paths {
        debug!("Scanning config path: {:?}", path);
        let results = scan_directory(path, PluginOrigin::Config);
        for result in results {
            match result {
                Ok(candidate) => all_candidates.push(candidate),
                Err(e) => debug!("Error scanning {:?}: {}", path, e),
            }
        }
    }

    // Priority 2: Workspace local
    if let Some(workspace) = &config.workspace_dir {
        for subdir in [".aleph/extensions", ".claude/extensions"] {
            let path = workspace.join(subdir);
            debug!("Scanning workspace path: {:?}", path);
            let results = scan_directory(&path, PluginOrigin::Workspace);
            for result in results {
                match result {
                    Ok(candidate) => all_candidates.push(candidate),
                    Err(e) => debug!("Error scanning {:?}: {}", path, e),
                }
            }
        }
    }

    // Priority 3: Global user-level
    let home = config
        .home_dir
        .clone()
        .or_else(dirs::home_dir)
        .unwrap_or_default();

    for subdir in [".aleph/extensions", ".claude/extensions"] {
        let path = home.join(subdir);
        debug!("Scanning global path: {:?}", path);
        let results = scan_directory(&path, PluginOrigin::Global);
        for result in results {
            match result {
                Ok(candidate) => all_candidates.push(candidate),
                Err(e) => debug!("Error scanning {:?}: {}", path, e),
            }
        }
    }

    // Priority 4: Bundled
    if let Some(bundled) = &config.bundled_dir {
        debug!("Scanning bundled path: {:?}", bundled);
        let results = scan_directory(bundled, PluginOrigin::Bundled);
        for result in results {
            match result {
                Ok(candidate) => all_candidates.push(candidate),
                Err(e) => debug!("Error scanning {:?}: {}", bundled, e),
            }
        }
    }

    // Resolve conflicts
    Ok(resolve_conflicts(all_candidates))
}

/// Scan a single path and return discovered plugins without conflict resolution
///
/// Useful when you need to scan additional paths after initial discovery.
pub fn scan_path(
    path: &std::path::Path,
    origin: PluginOrigin,
) -> Vec<Result<PluginCandidate, ExtensionError>> {
    scan_directory(path, origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_discover_all_layers() {
        let home = tempdir().unwrap();
        let workspace = tempdir().unwrap();

        // Create global plugin
        let global_ext = home.path().join(".aleph/extensions/global-plugin");
        fs::create_dir_all(&global_ext).unwrap();
        fs::write(global_ext.join("SKILL.md"), "# Global").unwrap();

        // Create workspace plugin
        let ws_ext = workspace.path().join(".aleph/extensions/ws-plugin");
        fs::create_dir_all(&ws_ext).unwrap();
        fs::write(ws_ext.join("SKILL.md"), "# Workspace").unwrap();

        let config = DiscoveryConfig {
            workspace_dir: Some(workspace.path().to_path_buf()),
            home_dir: Some(home.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert_eq!(resolved.active.len(), 2);
    }

    #[test]
    fn test_discover_workspace_overrides_global() {
        let home = tempdir().unwrap();
        let workspace = tempdir().unwrap();

        // Create same-named plugin in both locations
        let global_ext = home.path().join(".aleph/extensions/same-plugin");
        fs::create_dir_all(&global_ext).unwrap();
        fs::write(global_ext.join("SKILL.md"), "# Global").unwrap();

        let ws_ext = workspace.path().join(".aleph/extensions/same-plugin");
        fs::create_dir_all(&ws_ext).unwrap();
        fs::write(ws_ext.join("SKILL.md"), "# Workspace").unwrap();

        let config = DiscoveryConfig {
            workspace_dir: Some(workspace.path().to_path_buf()),
            home_dir: Some(home.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert_eq!(resolved.active.len(), 1);
        assert_eq!(resolved.overridden.len(), 1);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Workspace);
    }

    #[test]
    fn test_discover_config_overrides_workspace() {
        let home = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let config_path = tempdir().unwrap();

        // Create plugin in workspace
        let ws_ext = workspace.path().join(".aleph/extensions/my-plugin");
        fs::create_dir_all(&ws_ext).unwrap();
        fs::write(ws_ext.join("SKILL.md"), "# Workspace").unwrap();

        // Create plugin in config path
        let config_ext = config_path.path().join("my-plugin");
        fs::create_dir_all(&config_ext).unwrap();
        fs::write(config_ext.join("SKILL.md"), "# Config").unwrap();

        let config = DiscoveryConfig {
            extra_paths: vec![config_path.path().to_path_buf()],
            workspace_dir: Some(workspace.path().to_path_buf()),
            home_dir: Some(home.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert_eq!(resolved.active.len(), 1);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Config);
    }

    #[test]
    fn test_discover_bundled() {
        let home = tempdir().unwrap();
        let bundled = tempdir().unwrap();

        // Create bundled plugin
        let bundled_ext = bundled.path().join("bundled-plugin");
        fs::create_dir_all(&bundled_ext).unwrap();
        fs::write(bundled_ext.join("SKILL.md"), "# Bundled").unwrap();

        let config = DiscoveryConfig {
            home_dir: Some(home.path().to_path_buf()),
            bundled_dir: Some(bundled.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert_eq!(resolved.active.len(), 1);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Bundled);
    }

    #[test]
    fn test_discover_empty() {
        let home = tempdir().unwrap();

        let config = DiscoveryConfig {
            home_dir: Some(home.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert!(resolved.active.is_empty());
        assert!(resolved.overridden.is_empty());
    }

    #[test]
    fn test_discover_claude_extensions_dir() {
        let home = tempdir().unwrap();

        // Create plugin in .claude/extensions (Claude Code compatible)
        let claude_ext = home.path().join(".claude/extensions/claude-plugin");
        fs::create_dir_all(&claude_ext).unwrap();
        fs::write(claude_ext.join("SKILL.md"), "# Claude Compatible").unwrap();

        let config = DiscoveryConfig {
            home_dir: Some(home.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert_eq!(resolved.active.len(), 1);
        assert_eq!(resolved.active[0].id, "claude-plugin");
    }

    #[test]
    fn test_discovery_config_builder() {
        let config = DiscoveryConfig::with_workspace("/path/to/workspace")
            .with_bundled_dir("/path/to/bundled")
            .with_home_dir("/path/to/home");

        assert_eq!(
            config.workspace_dir,
            Some(PathBuf::from("/path/to/workspace"))
        );
        assert_eq!(
            config.bundled_dir,
            Some(PathBuf::from("/path/to/bundled"))
        );
        assert_eq!(config.home_dir, Some(PathBuf::from("/path/to/home")));
    }

    #[test]
    fn test_scan_path_helper() {
        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("test-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("SKILL.md"), "# Test").unwrap();

        let results = scan_path(dir.path(), PluginOrigin::Global);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
    }
}
