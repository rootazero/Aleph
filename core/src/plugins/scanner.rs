//! Plugin directory scanner
//!
//! Discovers plugins from the plugins directory.

use std::path::{Path, PathBuf};

use crate::plugins::error::{PluginError, PluginResult};

/// Plugin scanner for discovering plugins
#[derive(Debug)]
pub struct PluginScanner {
    /// Base plugins directory
    plugins_dir: PathBuf,
    /// Additional development plugin paths
    dev_paths: Vec<PathBuf>,
}

impl PluginScanner {
    /// Create a new plugin scanner
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self {
            plugins_dir,
            dev_paths: Vec::new(),
        }
    }

    /// Add development plugin paths
    pub fn with_dev_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.dev_paths = paths;
        self
    }

    /// Get the plugins directory
    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// Scan for all available plugins
    ///
    /// Returns a list of plugin directory paths that contain valid plugin structure.
    pub fn scan(&self) -> PluginResult<Vec<PathBuf>> {
        let mut plugins = Vec::new();

        // Scan main plugins directory
        if self.plugins_dir.exists() {
            let found = self.scan_directory(&self.plugins_dir)?;
            plugins.extend(found);
        } else {
            tracing::debug!("Plugins directory does not exist: {:?}", self.plugins_dir);
        }

        // Scan development paths
        for dev_path in &self.dev_paths {
            if dev_path.exists() {
                if is_valid_plugin_dir(dev_path) {
                    tracing::info!("Found dev plugin: {:?}", dev_path);
                    plugins.push(dev_path.clone());
                } else {
                    // Try scanning as a directory of plugins
                    let found = self.scan_directory(dev_path)?;
                    plugins.extend(found);
                }
            } else {
                tracing::warn!("Dev plugin path does not exist: {:?}", dev_path);
            }
        }

        Ok(plugins)
    }

    /// Scan a single plugin by path
    ///
    /// Returns the path if it's a valid plugin, or an error if not.
    pub fn scan_single(&self, path: &Path) -> PluginResult<PathBuf> {
        if !path.exists() {
            return Err(PluginError::DirectoryNotFound(path.to_path_buf()));
        }

        if !is_valid_plugin_dir(path) {
            return Err(PluginError::InvalidStructure {
                path: path.to_path_buf(),
                reason: "Missing .claude-plugin/plugin.json".to_string(),
            });
        }

        Ok(path.to_path_buf())
    }

    /// Scan a directory for plugins
    fn scan_directory(&self, dir: &Path) -> PluginResult<Vec<PathBuf>> {
        let mut plugins = Vec::new();

        let entries = std::fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            // Skip hidden directories
            if path
                .file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }

            if is_valid_plugin_dir(&path) {
                tracing::debug!("Found plugin: {:?}", path);
                plugins.push(path);
            } else {
                tracing::trace!(
                    "Skipping non-plugin directory: {:?} (missing .claude-plugin/plugin.json)",
                    path
                );
            }
        }

        Ok(plugins)
    }
}

/// Check if a directory is a valid Claude Code plugin
///
/// A valid plugin directory must contain `.claude-plugin/plugin.json`.
pub fn is_valid_plugin_dir(path: &Path) -> bool {
    let manifest_path = path.join(".claude-plugin").join("plugin.json");
    manifest_path.exists() && manifest_path.is_file()
}

/// Get the default plugins directory
pub fn default_plugins_dir() -> PathBuf {
    crate::utils::paths::get_config_dir()
        .map(|p| p.join("plugins"))
        .unwrap_or_else(|_| PathBuf::from(".").join("aether").join("plugins"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_plugin(dir: &Path, name: &str) -> PathBuf {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_dir.join(".claude-plugin").join("plugin.json"),
            format!(r#"{{"name": "{}"}}"#, name),
        )
        .unwrap();
        plugin_dir
    }

    #[test]
    fn test_scan_empty_directory() {
        let temp = TempDir::new().unwrap();
        let scanner = PluginScanner::new(temp.path().to_path_buf());
        let plugins = scanner.scan().unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_scan_finds_plugins() {
        let temp = TempDir::new().unwrap();

        create_test_plugin(temp.path(), "plugin-a");
        create_test_plugin(temp.path(), "plugin-b");

        // Create a non-plugin directory
        fs::create_dir(temp.path().join("not-a-plugin")).unwrap();

        let scanner = PluginScanner::new(temp.path().to_path_buf());
        let plugins = scanner.scan().unwrap();

        assert_eq!(plugins.len(), 2);
    }

    #[test]
    fn test_scan_skips_hidden_directories() {
        let temp = TempDir::new().unwrap();

        create_test_plugin(temp.path(), "visible-plugin");

        // Create hidden plugin (should be skipped)
        let hidden = temp.path().join(".hidden-plugin");
        fs::create_dir_all(hidden.join(".claude-plugin")).unwrap();
        fs::write(
            hidden.join(".claude-plugin").join("plugin.json"),
            r#"{"name": "hidden"}"#,
        )
        .unwrap();

        let scanner = PluginScanner::new(temp.path().to_path_buf());
        let plugins = scanner.scan().unwrap();

        assert_eq!(plugins.len(), 1);
    }

    #[test]
    fn test_scan_single_valid() {
        let temp = TempDir::new().unwrap();
        let plugin_path = create_test_plugin(temp.path(), "my-plugin");

        let scanner = PluginScanner::new(temp.path().to_path_buf());
        let result = scanner.scan_single(&plugin_path);

        assert!(result.is_ok());
    }

    #[test]
    fn test_scan_single_invalid() {
        let temp = TempDir::new().unwrap();
        let not_plugin = temp.path().join("not-plugin");
        fs::create_dir(&not_plugin).unwrap();

        let scanner = PluginScanner::new(temp.path().to_path_buf());
        let result = scanner.scan_single(&not_plugin);

        assert!(matches!(result, Err(PluginError::InvalidStructure { .. })));
    }

    #[test]
    fn test_is_valid_plugin_dir() {
        let temp = TempDir::new().unwrap();

        // Valid plugin
        let valid = create_test_plugin(temp.path(), "valid");
        assert!(is_valid_plugin_dir(&valid));

        // Missing plugin.json
        let missing = temp.path().join("missing");
        fs::create_dir_all(missing.join(".claude-plugin")).unwrap();
        assert!(!is_valid_plugin_dir(&missing));

        // No .claude-plugin directory
        let no_dir = temp.path().join("no-dir");
        fs::create_dir(&no_dir).unwrap();
        assert!(!is_valid_plugin_dir(&no_dir));
    }
}
