//! Directory scanner for discovering components
//!
//! Implements the multi-directory scanning strategy with upward traversal.

use super::paths::*;
use super::types::*;
use super::{DiscoveryConfig, DiscoveryResult};
use std::path::{Path, PathBuf};
use tracing::{debug, trace};

/// Directory scanner for discovering components across multiple locations
#[derive(Debug)]
pub struct DirectoryScanner {
    /// Aether home directory (~/.aleph/)
    aether_home: PathBuf,
    /// Claude home directory (~/.claude/)
    claude_home: Option<PathBuf>,
    /// Git root directory (if found)
    git_root: Option<PathBuf>,
    /// Working directory
    working_dir: PathBuf,
    /// Configuration
    config: DiscoveryConfig,
}

impl DirectoryScanner {
    /// Create a new directory scanner
    pub fn new(config: &DiscoveryConfig) -> DiscoveryResult<Self> {
        let aether_home = aether_home_dir()?;

        // Claude home is optional (only scan if it exists)
        let claude_home = if config.scan_claude_dirs {
            claude_home_dir().ok().filter(|p| p.exists())
        } else {
            None
        };

        // Find git root if scanning project dirs
        let git_root = if config.scan_project_dirs {
            find_git_root(&config.working_dir)
        } else {
            None
        };

        debug!(
            aether_home = ?aether_home,
            claude_home = ?claude_home,
            git_root = ?git_root,
            working_dir = ?config.working_dir,
            "DirectoryScanner initialized"
        );

        Ok(Self {
            aether_home,
            claude_home,
            git_root,
            working_dir: config.working_dir.clone(),
            config: config.clone(),
        })
    }

    /// Get the git root directory
    pub fn git_root(&self) -> Option<&Path> {
        self.git_root.as_deref()
    }

    /// Get all directories to scan, in priority order
    ///
    /// Priority order (lowest to highest):
    /// 1. Claude global (~/.claude/) - priority 0
    /// 2. Aether global (~/.aleph/) - priority 10
    /// 3. Project-level .claude/ directories - priority 20+
    pub fn get_all_directories(&self) -> DiscoveryResult<Vec<ScanDirectory>> {
        let mut dirs = Vec::new();

        // 1. Claude global (lowest priority, read-only)
        if let Some(ref claude_home) = self.claude_home {
            if claude_home.exists() {
                dirs.push(ScanDirectory::new(
                    claude_home.clone(),
                    DiscoverySource::ClaudeGlobal,
                    0,
                ));
            }
        }

        // 2. Aether global
        if self.aether_home.exists() {
            dirs.push(ScanDirectory::new(
                self.aether_home.clone(),
                DiscoverySource::AetherGlobal,
                10,
            ));
        }

        // 3. Project-level .claude/ directories (upward traversal)
        if self.config.scan_project_dirs {
            let stop = self.git_root.as_deref();
            let claude_dirs = find_dir_upward(
                CLAUDE_HOME_DIR,
                &self.working_dir,
                stop,
                self.config.max_upward_depth,
            );

            // Reverse to get proper priority (deeper = higher priority)
            for (i, dir) in claude_dirs.into_iter().rev().enumerate() {
                dirs.push(ScanDirectory::new(
                    dir,
                    DiscoverySource::Project,
                    20 + i as u32,
                ));
            }
        }

        trace!("Scan directories: {:?}", dirs);
        Ok(dirs)
    }

    /// Find configuration files with upward traversal
    ///
    /// Returns paths in priority order (global first, project last).
    pub fn find_upward(&self, filename: &str) -> DiscoveryResult<Vec<PathBuf>> {
        let mut configs = Vec::new();

        // 1. Global config (lowest priority)
        let global_config = self.aether_home.join(filename);
        if global_config.exists() {
            configs.push(global_config);
        }

        // 2. Project configs (upward traversal)
        if self.config.scan_project_dirs {
            let stop = self.git_root.as_deref();
            let project_configs = find_file_upward(
                filename,
                &self.working_dir,
                stop,
                self.config.max_upward_depth,
            );

            // Reverse so higher directories come first (lower priority)
            for config in project_configs.into_iter().rev() {
                configs.push(config);
            }
        }

        trace!("Found config files for '{}': {:?}", filename, configs);
        Ok(configs)
    }

    /// Discover a specific component type (skills, commands, agents, plugins)
    pub fn discover_component(&self, component_name: &str) -> DiscoveryResult<Vec<DiscoveredPath>> {
        let mut discovered = Vec::new();
        let scan_dirs = self.get_all_directories()?;

        for scan_dir in scan_dirs {
            if !scan_dir.exists() {
                continue;
            }

            let component_dir = scan_dir.path.join(component_name);
            if !component_dir.exists() || !component_dir.is_dir() {
                continue;
            }

            // Scan for subdirectories (each is a component)
            match std::fs::read_dir(&component_dir) {
                Ok(entries) => {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let path = entry.path();
                        if path.is_dir() {
                            // Skip hidden directories
                            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                if name.starts_with('.') {
                                    continue;
                                }
                            }

                            discovered.push(DiscoveredPath::new(
                                path,
                                scan_dir.source,
                                scan_dir.priority,
                            ));
                        } else if path.is_file() {
                            // Also include direct .md files (for commands/agents)
                            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                                if ext == "md" {
                                    discovered.push(DiscoveredPath::new(
                                        path,
                                        scan_dir.source,
                                        scan_dir.priority,
                                    ));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to read directory {:?}: {}", component_dir, e);
                }
            }
        }

        // Sort by priority (lower first, so higher priority items can override)
        discovered.sort_by_key(|d| d.priority);

        trace!(
            "Discovered {} {} components",
            discovered.len(),
            component_name
        );
        Ok(discovered)
    }

    /// Discover plugins (special handling for plugin structure)
    pub fn discover_plugins(&self) -> DiscoveryResult<Vec<DiscoveredPath>> {
        let mut discovered = Vec::new();

        // Only scan Aether plugins directory (not Claude)
        let plugins_dir = self.aether_home.join(PLUGINS_DIR);
        if !plugins_dir.exists() || !plugins_dir.is_dir() {
            return Ok(discovered);
        }

        match std::fs::read_dir(&plugins_dir) {
            Ok(entries) => {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }

                    // Skip hidden directories
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with('.') {
                            continue;
                        }
                    }

                    // Check for valid plugin structure (.claude-plugin/plugin.json)
                    let manifest_path = path.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE);
                    if manifest_path.exists() {
                        discovered.push(DiscoveredPath::new(
                            path,
                            DiscoverySource::AetherGlobal,
                            10,
                        ));
                    } else {
                        trace!("Skipping {:?}: no plugin manifest", path);
                    }
                }
            }
            Err(e) => {
                debug!("Failed to read plugins directory {:?}: {}", plugins_dir, e);
            }
        }

        trace!("Discovered {} plugins", discovered.len());
        Ok(discovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_structure(temp: &TempDir) -> PathBuf {
        let root = temp.path();

        // Create git root marker
        std::fs::create_dir(root.join(".git")).unwrap();

        // Create Aether-like structure
        let aether = root.join(".aether");
        std::fs::create_dir_all(aether.join("skills/my-skill")).unwrap();
        std::fs::create_dir_all(aether.join("commands/my-cmd")).unwrap();
        std::fs::create_dir_all(aether.join("plugins")).unwrap();

        // Create project-level .claude
        std::fs::create_dir_all(root.join("project/.claude/skills/project-skill")).unwrap();

        root.to_path_buf()
    }

    #[test]
    fn test_scanner_get_directories() {
        let temp = TempDir::new().unwrap();
        let root = create_test_structure(&temp);

        let config = DiscoveryConfig {
            working_dir: root.join("project"),
            scan_claude_dirs: true,
            scan_project_dirs: true,
            max_upward_depth: 10,
        };

        // Override aether home for testing
        let scanner = DirectoryScanner {
            aether_home: root.join(".aether"),
            claude_home: None,
            git_root: Some(root.clone()),
            working_dir: root.join("project"),
            config,
        };

        let dirs = scanner.get_all_directories().unwrap();

        // Should have Aether global + project .claude
        assert!(dirs.len() >= 1);
        assert!(dirs.iter().any(|d| d.source == DiscoverySource::AetherGlobal));
    }

    #[test]
    fn test_scanner_discover_skills() {
        let temp = TempDir::new().unwrap();
        let root = create_test_structure(&temp);

        let config = DiscoveryConfig {
            working_dir: root.join("project"),
            scan_claude_dirs: true,
            scan_project_dirs: true,
            max_upward_depth: 10,
        };

        let scanner = DirectoryScanner {
            aether_home: root.join(".aether"),
            claude_home: None,
            git_root: Some(root.clone()),
            working_dir: root.join("project"),
            config,
        };

        let skills = scanner.discover_component("skills").unwrap();

        // Should find my-skill and project-skill
        assert!(skills.len() >= 1);
        assert!(skills.iter().any(|s| s.name == "my-skill"));
    }

    #[test]
    fn test_scanner_find_config_upward() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create nested structure with configs
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".aether")).unwrap();
        std::fs::write(root.join(".aether/aether.jsonc"), "{}").unwrap();
        std::fs::create_dir_all(root.join("project/subdir")).unwrap();
        std::fs::write(root.join("project/aether.jsonc"), "{}").unwrap();

        let config = DiscoveryConfig {
            working_dir: root.join("project/subdir"),
            scan_claude_dirs: false,
            scan_project_dirs: true,
            max_upward_depth: 10,
        };

        let scanner = DirectoryScanner {
            aether_home: root.join(".aether"),
            claude_home: None,
            git_root: Some(root.to_path_buf()),
            working_dir: root.join("project/subdir"),
            config,
        };

        let configs = scanner.find_upward("aether.jsonc").unwrap();

        // Should find both configs
        assert_eq!(configs.len(), 2);
    }
}
