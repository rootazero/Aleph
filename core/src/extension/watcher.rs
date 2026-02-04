//! Extension file watcher for hot-reload support
//!
//! This module watches extension directories for changes and triggers
//! reload when skill, command, agent, or plugin files are modified.
//!
//! Watched directories:
//! - `~/.claude/` (skills/, commands/, agents/, plugins/)
//! - `~/.aleph/` (skills/, commands/, agents/, plugins/)
//! - `.claude/` (project-local)
//! - `.aether/` (project-local)
//!
//! Uses macOS FSEvents for efficient file system monitoring with debouncing.

use crate::error::{AlephError, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Debounce delay for extension file changes (ms)
const DEBOUNCE_DELAY_MS: u64 = 500;

/// File extensions to watch
const WATCHED_EXTENSIONS: &[&str] = &["md", "json", "toml", "yaml", "yml"];

/// Callback type for extension change notifications
pub type ExtensionChangeCallback = Arc<dyn Fn(ExtensionChangeEvent) + Send + Sync>;

/// Event describing what changed in the extension system
#[derive(Debug, Clone)]
pub struct ExtensionChangeEvent {
    /// Paths that changed
    pub changed_paths: Vec<PathBuf>,
    /// Type of change detected
    pub change_type: ExtensionChangeType,
}

/// Type of extension change
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionChangeType {
    /// Skill files changed
    Skill,
    /// Command files changed
    Command,
    /// Agent files changed
    Agent,
    /// Plugin manifest changed
    Plugin,
    /// Unknown/mixed change
    Unknown,
}

impl ExtensionChangeType {
    /// Determine change type from file path
    fn from_path(path: &PathBuf) -> Self {
        let path_str = path.to_string_lossy();

        if path_str.contains("/skills/") {
            Self::Skill
        } else if path_str.contains("/commands/") {
            Self::Command
        } else if path_str.contains("/agents/") {
            Self::Agent
        } else if path_str.contains("plugin.json") {
            Self::Plugin
        } else {
            Self::Unknown
        }
    }
}

/// Extension file watcher with hot-reload support
///
/// Monitors extension directories for changes and invokes a callback
/// when modifications are detected.
pub struct ExtensionWatcher {
    /// Directories to watch
    watch_dirs: Vec<PathBuf>,
    /// Callback invoked when extensions change
    callback: ExtensionChangeCallback,
    /// Debounced file watcher (None when stopped)
    debouncer: Arc<Mutex<Option<Debouncer<RecommendedWatcher, FileIdMap>>>>,
}

impl ExtensionWatcher {
    /// Create a new extension watcher with default directories
    ///
    /// Watches:
    /// - `~/.claude/` (global)
    /// - `~/.aleph/` (global)
    /// - `.claude/` (project-local, if exists)
    /// - `.aether/` (project-local, if exists)
    ///
    /// # Arguments
    /// * `project_root` - Optional project root for watching local directories
    /// * `callback` - Function to call when extensions change
    pub fn new<F>(project_root: Option<PathBuf>, callback: F) -> Self
    where
        F: Fn(ExtensionChangeEvent) + Send + Sync + 'static,
    {
        let mut watch_dirs = Vec::new();

        // Global directories
        if let Some(home) = dirs::home_dir() {
            let claude_global = home.join(".claude");
            let aether_global = home.join(".aether");

            if claude_global.exists() {
                watch_dirs.push(claude_global);
            }
            if aether_global.exists() {
                watch_dirs.push(aether_global);
            }
        }

        // Project-local directories
        if let Some(root) = project_root {
            let claude_local = root.join(".claude");
            let aether_local = root.join(".aether");

            if claude_local.exists() {
                watch_dirs.push(claude_local);
            }
            if aether_local.exists() {
                watch_dirs.push(aether_local);
            }
        }

        Self {
            watch_dirs,
            callback: Arc::new(callback),
            debouncer: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a watcher for specific directories (test/custom use)
    pub fn new_with_dirs<F>(watch_dirs: Vec<PathBuf>, callback: F) -> Self
    where
        F: Fn(ExtensionChangeEvent) + Send + Sync + 'static,
    {
        Self {
            watch_dirs,
            callback: Arc::new(callback),
            debouncer: Arc::new(Mutex::new(None)),
        }
    }

    /// Start watching extension directories
    ///
    /// Spawns a background thread that monitors the directories for changes.
    /// When changes are detected (debounced), the callback is invoked.
    pub fn start(&self) -> Result<()> {
        let mut debouncer_lock = self.debouncer.lock().unwrap_or_else(|e| e.into_inner());

        if debouncer_lock.is_some() {
            return Err(AlephError::config("Extension watcher already started"));
        }

        if self.watch_dirs.is_empty() {
            warn!("No extension directories to watch");
            return Ok(());
        }

        let callback = Arc::clone(&self.callback);

        // Create debounced watcher
        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_DELAY_MS),
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        // Collect all changed paths
                        let changed_paths: Vec<PathBuf> = events
                            .iter()
                            .flat_map(|e| e.paths.iter().cloned())
                            .filter(Self::should_watch_file)
                            .collect::<HashSet<_>>()
                            .into_iter()
                            .collect();

                        if changed_paths.is_empty() {
                            return;
                        }

                        // Determine change type from first path
                        let change_type = changed_paths
                            .first()
                            .map(ExtensionChangeType::from_path)
                            .unwrap_or(ExtensionChangeType::Unknown);

                        debug!(
                            ?changed_paths,
                            ?change_type,
                            "Extension files changed"
                        );

                        callback(ExtensionChangeEvent {
                            changed_paths,
                            change_type,
                        });
                    }
                    Err(errors) => {
                        for error in &errors {
                            error!(?error, "Extension file watcher error");
                        }
                    }
                }
            },
        )
        .map_err(|e| AlephError::config(format!("Failed to create extension watcher: {}", e)))?;

        // Watch each directory recursively
        for dir in &self.watch_dirs {
            if dir.exists() {
                debouncer
                    .watcher()
                    .watch(dir, RecursiveMode::Recursive)
                    .map_err(|e| {
                        AlephError::config(format!(
                            "Failed to watch directory {}: {}",
                            dir.display(),
                            e
                        ))
                    })?;
                info!(dir = %dir.display(), "Watching extension directory");
            }
        }

        *debouncer_lock = Some(debouncer);
        info!("Extension watcher started");

        Ok(())
    }

    /// Stop watching extension directories
    pub fn stop(&self) -> Result<()> {
        let mut debouncer_lock = self.debouncer.lock().unwrap_or_else(|e| e.into_inner());

        if debouncer_lock.is_none() {
            return Err(AlephError::config("Extension watcher not started"));
        }

        *debouncer_lock = None;
        info!("Extension watcher stopped");
        Ok(())
    }

    /// Check if the watcher is running
    pub fn is_running(&self) -> bool {
        self.debouncer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Check if a file should be watched based on extension
    fn should_watch_file(path: &PathBuf) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| WATCHED_EXTENSIONS.contains(&ext))
            .unwrap_or(false)
    }

    /// Add a directory to watch (while running)
    pub fn add_watch_dir(&self, dir: PathBuf) -> Result<()> {
        let debouncer_lock = self.debouncer.lock().unwrap_or_else(|e| e.into_inner());

        if debouncer_lock.is_some()
            && dir.exists() {
                // Note: We can't call watch on debouncer.watcher() because it's behind Arc<Mutex<>>
                // For now, restart is required to add new directories
                drop(debouncer_lock);
                warn!(
                    dir = %dir.display(),
                    "Adding watch directory requires restart"
                );
                return Err(AlephError::config(
                    "Cannot add directory to running watcher. Please restart the watcher.",
                ));
            }

        Ok(())
    }
}

impl Drop for ExtensionWatcher {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use tempfile::TempDir;

    #[test]
    fn test_watcher_creation() {
        let watcher = ExtensionWatcher::new(None, |_| {});
        assert!(!watcher.is_running());
    }

    #[test]
    fn test_watcher_with_custom_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().to_path_buf();

        let watcher = ExtensionWatcher::new_with_dirs(vec![path.clone()], |_| {});
        assert_eq!(watcher.watch_dirs.len(), 1);
        assert_eq!(watcher.watch_dirs[0], path);
    }

    #[test]
    fn test_watcher_start_stop() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().to_path_buf();

        let watcher = ExtensionWatcher::new_with_dirs(vec![path], |_| {});

        watcher.start().unwrap();
        assert!(watcher.is_running());

        watcher.stop().unwrap();
        assert!(!watcher.is_running());
    }

    #[test]
    fn test_watcher_empty_dirs() {
        let watcher = ExtensionWatcher::new_with_dirs(vec![], |_| {});

        // Should succeed but do nothing
        watcher.start().unwrap();
        assert!(!watcher.is_running()); // No debouncer created for empty dirs
    }

    #[test]
    fn test_change_type_detection() {
        assert_eq!(
            ExtensionChangeType::from_path(&PathBuf::from("/foo/skills/test.md")),
            ExtensionChangeType::Skill
        );
        assert_eq!(
            ExtensionChangeType::from_path(&PathBuf::from("/foo/commands/test.md")),
            ExtensionChangeType::Command
        );
        assert_eq!(
            ExtensionChangeType::from_path(&PathBuf::from("/foo/agents/test.md")),
            ExtensionChangeType::Agent
        );
        assert_eq!(
            ExtensionChangeType::from_path(&PathBuf::from("/foo/plugin.json")),
            ExtensionChangeType::Plugin
        );
        assert_eq!(
            ExtensionChangeType::from_path(&PathBuf::from("/foo/random.md")),
            ExtensionChangeType::Unknown
        );
    }

    #[test]
    fn test_should_watch_file() {
        assert!(ExtensionWatcher::should_watch_file(&PathBuf::from(
            "test.md"
        )));
        assert!(ExtensionWatcher::should_watch_file(&PathBuf::from(
            "plugin.json"
        )));
        assert!(ExtensionWatcher::should_watch_file(&PathBuf::from(
            "config.toml"
        )));
        assert!(ExtensionWatcher::should_watch_file(&PathBuf::from(
            "config.yaml"
        )));
        assert!(!ExtensionWatcher::should_watch_file(&PathBuf::from(
            "binary.exe"
        )));
        assert!(!ExtensionWatcher::should_watch_file(&PathBuf::from(
            "image.png"
        )));
    }

    #[test]
    fn test_watcher_file_change_detection() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        // Canonicalize for macOS symlink resolution
        let path = temp_dir.path().canonicalize().unwrap();

        let called = Arc::new(AtomicBool::new(false));
        let called_clone = Arc::clone(&called);

        let watcher = ExtensionWatcher::new_with_dirs(vec![path], move |event| {
            if event.change_type == ExtensionChangeType::Skill {
                called_clone.store(true, Ordering::SeqCst);
            }
        });

        watcher.start().unwrap();

        // Wait for watcher to initialize
        thread::sleep(Duration::from_millis(200));

        // Create a skill file
        let skill_file = skills_dir.join("test.md");
        fs::write(&skill_file, "# Test Skill").unwrap();

        // Wait for debounce + processing
        thread::sleep(Duration::from_millis(1200));

        // Callback should have been called
        assert!(called.load(Ordering::SeqCst));

        watcher.stop().unwrap();
    }
}
