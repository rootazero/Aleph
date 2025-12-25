/// Config file watcher for hot-reload support
///
/// This module watches the config file (~/.config/aether/config.toml) for external changes
/// and triggers a callback when modifications are detected. Uses macOS FSEvents for efficient
/// file system monitoring with debouncing to avoid duplicate events.
///
/// Architecture:
/// 1. Watcher runs in background thread using `notify` crate
/// 2. File changes are debounced (500ms) to avoid rapid-fire events
/// 3. On change, callback is invoked with updated Config
/// 4. Watcher automatically handles file deletion/recreation
///
/// # Example
///
/// ```no_run
/// use aethecore::config::watcher::ConfigWatcher;
/// use std::sync::Arc;
///
/// let watcher = ConfigWatcher::new(|config| {
///     println!("Config changed! New default provider: {:?}", config.general.default_provider);
/// });
///
/// watcher.start()?;
/// // Watcher runs in background...
/// watcher.stop()?;
/// ```
use crate::config::Config;
use crate::error::{AetherError, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Callback type for config change notifications
///
/// This callback is invoked when the config file changes.
/// It receives the updated Config object or an error if loading failed.
pub type ConfigChangeCallback = Arc<dyn Fn(Result<Config>) + Send + Sync>;

/// Config file watcher with hot-reload support
///
/// Monitors the config file for changes and invokes a callback
/// when modifications are detected. Uses FSEvents on macOS for
/// efficient file system monitoring.
///
/// # Thread Safety
///
/// The watcher runs in a background thread and is Send + Sync safe.
/// The callback is invoked on the watcher's background thread, so
/// implementations should dispatch to the appropriate thread if needed.
pub struct ConfigWatcher {
    /// Path to the config file being watched
    config_path: PathBuf,
    /// Callback invoked when config changes
    callback: ConfigChangeCallback,
    /// Debounced file watcher (None when stopped)
    debouncer: Arc<Mutex<Option<Debouncer<RecommendedWatcher, FileIdMap>>>>,
}

impl ConfigWatcher {
    /// Create a new config watcher
    ///
    /// # Arguments
    /// * `callback` - Function to call when config changes
    ///
    /// # Example
    ///
    /// ```no_run
    /// let watcher = ConfigWatcher::new(|result| {
    ///     match result {
    ///         Ok(config) => println!("Config reloaded"),
    ///         Err(e) => eprintln!("Config reload failed: {}", e),
    ///     }
    /// });
    /// ```
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(Result<Config>) + Send + Sync + 'static,
    {
        Self {
            config_path: Config::default_path(),
            callback: Arc::new(callback),
            debouncer: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new config watcher for a custom path (test only)
    ///
    /// # Arguments
    /// * `config_path` - Path to config file to watch
    /// * `callback` - Function to call when config changes
    #[cfg(test)]
    pub fn new_with_path<F>(config_path: PathBuf, callback: F) -> Self
    where
        F: Fn(Result<Config>) + Send + Sync + 'static,
    {
        Self {
            config_path,
            callback: Arc::new(callback),
            debouncer: Arc::new(Mutex::new(None)),
        }
    }

    /// Start watching the config file
    ///
    /// Spawns a background thread that monitors the config file for changes.
    /// When changes are detected (debounced to 500ms), the callback is invoked.
    ///
    /// # Errors
    ///
    /// * `AetherError::ConfigError` - Failed to start file watcher
    ///
    /// # Example
    ///
    /// ```no_run
    /// let watcher = ConfigWatcher::new(|_| {});
    /// watcher.start()?;
    /// // Config file is now being watched...
    /// ```
    pub fn start(&self) -> Result<()> {
        let mut debouncer_lock = self.debouncer.lock().unwrap();

        // Check if already started
        if debouncer_lock.is_some() {
            return Err(AetherError::config("Watcher already started"));
        }

        // Clone callback for move into closure
        let callback = Arc::clone(&self.callback);
        let config_path = self.config_path.clone();

        // Create debounced watcher with 500ms delay
        let mut debouncer = new_debouncer(
            Duration::from_millis(500),
            None,
            move |result: DebounceEventResult| match result {
                Ok(events) => {
                    // Check if any event affects our config file
                    for event in events {
                        if event.paths.iter().any(|p| p == &config_path) {
                            log::info!("Config file changed, reloading...");
                            let config_result = Config::load_from_file(&config_path);
                            callback(config_result);
                            break;
                        }
                    }
                }
                Err(errors) => {
                    for error in &errors {
                        log::error!("File watcher error: {:?}", error);
                    }
                    callback(Err(AetherError::config(format!(
                        "File watcher error: {:?}",
                        errors
                    ))));
                }
            },
        )
        .map_err(|e| AetherError::config(format!("Failed to create file watcher: {}", e)))?;

        // Watch the config file (or its parent directory if file doesn't exist)
        let watch_path = if self.config_path.exists() {
            self.config_path.clone()
        } else if let Some(parent) = self.config_path.parent() {
            parent.to_path_buf()
        } else {
            return Err(AetherError::config("Invalid config path"));
        };

        debouncer
            .watcher()
            .watch(&watch_path, RecursiveMode::NonRecursive)
            .map_err(|e| AetherError::config(format!("Failed to watch config file: {}", e)))?;

        log::info!("Started watching config file: {}", self.config_path.display());

        // Store debouncer to keep it alive
        *debouncer_lock = Some(debouncer);

        Ok(())
    }

    /// Stop watching the config file
    ///
    /// Stops the background watcher thread and releases resources.
    ///
    /// # Example
    ///
    /// ```no_run
    /// let watcher = ConfigWatcher::new(|_| {});
    /// watcher.start()?;
    /// // ... later ...
    /// watcher.stop()?;
    /// ```
    pub fn stop(&self) -> Result<()> {
        let mut debouncer_lock = self.debouncer.lock().unwrap();

        if debouncer_lock.is_none() {
            return Err(AetherError::config("Watcher not started"));
        }

        // Drop the debouncer to stop watching
        *debouncer_lock = None;

        log::info!("Stopped watching config file");
        Ok(())
    }

    /// Check if the watcher is currently running (test only)
    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.debouncer.lock().unwrap().is_some()
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        // Ensure watcher is stopped when dropped
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    #[test]
    fn test_watcher_creation() {
        let watcher = ConfigWatcher::new(|_| {});
        assert!(!watcher.is_running());
    }

    #[test]
    fn test_watcher_start_stop() {
        let watcher = ConfigWatcher::new(|_| {});

        // Start watcher
        watcher.start().unwrap();
        assert!(watcher.is_running());

        // Stop watcher
        watcher.stop().unwrap();
        assert!(!watcher.is_running());
    }

    #[test]
    fn test_watcher_double_start() {
        let watcher = ConfigWatcher::new(|_| {});

        watcher.start().unwrap();

        // Second start should fail
        let result = watcher.start();
        assert!(result.is_err());

        watcher.stop().unwrap();
    }

    #[test]
    fn test_watcher_file_change_detection() {
        // Create temporary config file
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        // Write initial config
        let config = Config::default();
        config.save_to_file(&path).unwrap();

        // Create watcher with callback
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = Arc::clone(&called);

        let watcher = ConfigWatcher::new_with_path(path.clone(), move |_result| {
            called_clone.store(true, Ordering::SeqCst);
        });

        watcher.start().unwrap();

        // Wait for watcher to initialize
        thread::sleep(Duration::from_millis(100));

        // Modify config file
        let mut config = Config::default();
        config.default_hotkey = "Command+Shift+A".to_string();
        config.save_to_file(&path).unwrap();

        // Wait for debounce + processing (500ms + margin)
        thread::sleep(Duration::from_millis(800));

        // Callback should have been called
        assert!(called.load(Ordering::SeqCst));

        watcher.stop().unwrap();
    }

    #[test]
    fn test_watcher_with_custom_path() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        let watcher = ConfigWatcher::new_with_path(path.clone(), |_| {});
        assert_eq!(watcher.config_path, path);
    }

    #[test]
    fn test_watcher_drop_cleanup() {
        let watcher = ConfigWatcher::new(|_| {});
        watcher.start().unwrap();
        assert!(watcher.is_running());

        // Drop watcher
        drop(watcher);

        // Watcher should be stopped (can't verify directly after drop,
        // but this tests that drop doesn't panic)
    }
}
