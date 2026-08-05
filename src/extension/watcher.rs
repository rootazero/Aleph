//! Extension file watcher for hot-reload support
//!
//! This module watches extension directories for changes and triggers
//! reload when skill, command, agent, or plugin files are modified.
//!
//! Watched directories:
//! - `~/.claude/` (global, Claude Code compatible)
//! - `~/.aleph/` (global)
//! - `~/.aleph/projects/<id>/` (project-level, if `project_id` provided)
//!
//! Uses macOS `FSEvents` for efficient file system monitoring with debouncing.

use crate::error::{AlephError, Result};
use crate::sync_primitives::{Arc, Mutex};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
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
    /// User-level hooks config (`~/.aleph/hooks.json` or
    /// `<cwd>/.aleph/hooks.{json,local.json}`) changed. Routed through the
    /// lightweight `sync_user_hooks` path so a hooks-only edit doesn't
    /// trigger a full plugin re-discovery.
    HooksConfig,
    /// Unknown/mixed change
    Unknown,
}

impl ExtensionChangeType {
    /// Determine change type from file path
    pub(crate) fn from_path(path: &Path) -> Self {
        let path_str = path.to_string_lossy();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let parent_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        // hooks.json / hooks.local.json living under a `.aleph` or `.claude`
        // directory is the user-level hook config layer (Claude Code parity).
        // Match directory names on path components rather than slash-delimited
        // substrings: on Windows `to_string_lossy()` yields backslashes, so
        // `contains("/skills/")` never matches and every edit falls through to
        // `Unknown` (forcing a full re-discovery).
        let has_dir = |name: &str| {
            path.components()
                .any(|c| c.as_os_str().to_str() == Some(name))
        };
        if matches!(file_name, "hooks.json" | "hooks.local.json")
            && (parent_name == ".aleph" || parent_name == ".claude")
        {
            Self::HooksConfig
        } else if has_dir("skills") {
            Self::Skill
        } else if has_dir("commands") {
            Self::Command
        } else if has_dir("agents") {
            Self::Agent
        } else if path_str.contains("plugin.json")
            || path_str.contains("plugin.toml")
            || path_str.contains("aleph.plugin.toml")
        {
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
    /// - `~/.claude/` (global, Claude Code compatible)
    /// - `~/.aleph/` (global)
    /// - `~/.aleph/projects/<id>/` (project-level, if `project_id` provided)
    ///
    /// # Arguments
    /// * `project_id` - Optional project name for watching project-level directories
    /// * `callback` - Function to call when extensions change
    pub fn new<F>(project_id: Option<&str>, callback: F) -> Self
    where
        F: Fn(ExtensionChangeEvent) + Send + Sync + 'static,
    {
        let mut watch_dirs = Vec::new();

        // Global directories. `.claude` is Claude Code's own directory and
        // stays on the real home; `.aleph` is OUR state and must follow
        // `ALEPH_HOME`, or the watcher watches a tree nobody writes to.
        if let Some(home) = dirs::home_dir() {
            let claude_global = home.join(".claude");
            let aleph_global =
                crate::utils::paths::get_config_dir().unwrap_or_else(|_| home.join(".aleph"));

            if claude_global.exists() {
                watch_dirs.push(claude_global);
            }
            if aleph_global.exists() {
                watch_dirs.push(aleph_global);
            }
        }

        // Agent config directory (~/.aleph/agents/<id>/)
        if let Some(id) = project_id {
            if let Ok(project_dir) = crate::utils::paths::get_agent_config_dir(id) {
                if project_dir.exists() {
                    watch_dirs.push(project_dir);
                }
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

        // Runtime data directories are written constantly during normal chat
        // turns (sessions, memory notes, runtime ledger, logs). They are not
        // extension sources, so watching them triggers expensive full reloads
        // that contend with the hot path. Exclude them from the debounced event
        // set.
        let runtime_data_dirs: Vec<PathBuf> = [
            crate::utils::paths::get_data_dir().ok(),
            crate::utils::paths::get_runtimes_dir().ok(),
            crate::utils::paths::get_note_memory_dir()
                .ok()
                .and_then(|p| p.parent().map(PathBuf::from)),
            crate::logging::file_appender::get_log_directory().ok(),
        ]
        .into_iter()
        .flatten()
        .collect();

        // Create debounced watcher
        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_DELAY_MS),
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        // Collect all changed paths, skipping runtime data
                        // directories that are not extension sources.
                        let changed_paths: HashSet<PathBuf> = events
                            .iter()
                            .flat_map(|e| e.paths.iter().cloned())
                            .filter(|p| Self::should_watch_file(p))
                            .filter(|p| !runtime_data_dirs.iter().any(|d| p.starts_with(d)))
                            .collect();

                        if changed_paths.is_empty() {
                            return;
                        }

                        // Determine change type across ALL paths. A debounced
                        // batch can mix kinds (e.g. a skill edit plus a plugin
                        // manifest change); classifying by the first path alone
                        // is nondeterministic (HashSet order) and could route a
                        // mixed batch down the Skill no-op path, silently
                        // skipping the required full reload. Homogeneous
                        // batches keep their cheap routing; mixed batches
                        // degrade to Unknown (full reload — safe superset).
                        let mut types = changed_paths
                            .iter()
                            .map(|p| ExtensionChangeType::from_path(p));
                        let first = types.next().unwrap_or(ExtensionChangeType::Unknown);
                        let change_type = if types.all(|t| t == first) {
                            first
                        } else {
                            ExtensionChangeType::Unknown
                        };

                        debug!(?changed_paths, ?change_type, "Extension files changed");

                        let changed_paths: Vec<PathBuf> = changed_paths.into_iter().collect();

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
        .map_err(|e| AlephError::config(format!("Failed to create extension watcher: {e}")))?;

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
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.debouncer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Check if a file should be watched based on extension
    fn should_watch_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| WATCHED_EXTENSIONS.contains(&ext))
    }

    /// Add a directory to watch (while running)
    pub fn add_watch_dir(&self, dir: PathBuf) -> Result<()> {
        let debouncer_lock = self.debouncer.lock().unwrap_or_else(|e| e.into_inner());

        if debouncer_lock.is_some() && dir.exists() {
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

/// Default suppression window for Aleph's own writes (Claude Code parity:
/// `changeDetector.ts` uses 5s — long enough to outlast typical
/// fs event delivery latency, short enough to recover if a mark is leaked).
const DEFAULT_INTERNAL_WRITE_TTL: Duration = Duration::from_secs(5);

/// Tracks file paths that Aleph itself just wrote, so the file watcher can
/// skip them and avoid a feedback loop (write -> reload -> write -> ...).
///
/// Writers call [`InternalWriteTracker::mark`] immediately after a successful
/// write; the watcher callback consults [`InternalWriteTracker::was_recent`]
/// and drops matching events.
///
/// Stored entries expire after `ttl` (default 5s). Pruning is lazy: each
/// `was_recent` / `mark` call evicts expired entries — there is no background
/// thread.
#[derive(Debug)]
pub struct InternalWriteTracker {
    recent: Mutex<HashMap<PathBuf, Instant>>,
    ttl: Duration,
}

impl Default for InternalWriteTracker {
    fn default() -> Self {
        Self::with_ttl(DEFAULT_INTERNAL_WRITE_TTL)
    }
}

impl InternalWriteTracker {
    /// Construct a tracker with a custom TTL. Tests use a short TTL to
    /// avoid sleeping for seconds.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            recent: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Record that Aleph just wrote `path`. Subsequent watcher events
    /// targeting the same canonicalised path within `ttl` are suppressed.
    pub fn mark(&self, path: &Path) {
        let key = canonicalize_best_effort(path);
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune_locked(&mut guard, self.ttl);
        guard.insert(key, Instant::now());
    }

    /// Returns true if `path` was marked within the TTL window.
    pub fn was_recent(&self, path: &Path) -> bool {
        let key = canonicalize_best_effort(path);
        let mut guard = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune_locked(&mut guard, self.ttl);
        guard.contains_key(&key)
    }

    fn prune_locked(map: &mut HashMap<PathBuf, Instant>, ttl: Duration) {
        let now = Instant::now();
        map.retain(|_, ts| now.duration_since(*ts) < ttl);
    }
}

/// Canonicalise `path` so that marks recorded before a file exists still
/// match watcher events fired after creation.
///
/// `Path::canonicalize` requires the file to exist, but writers typically
/// call [`InternalWriteTracker::mark`] *before* the write completes. We
/// therefore fall back to `parent.canonicalize().join(file_name)`, which
/// works whenever the parent directory exists — true for every file write
/// inside `~/.aleph/` or a project `.aleph/` dir. If even the parent is
/// missing we accept the raw path; the suppression window will then only
/// match if the watcher event echoes the same uncanonicalised form
/// (rare).
fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match parent.canonicalize() {
            Ok(canon_parent) => canon_parent.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the (non-Windows) file-change-detection test uses these.
    #[cfg(not(windows))]
    use crate::sync_primitives::{AtomicBool, Ordering};
    use std::fs;
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
    fn test_change_type_detects_hooks_config() {
        for parent in [".aleph", ".claude"] {
            for file in ["hooks.json", "hooks.local.json"] {
                let p = PathBuf::from(format!("/home/u/{parent}/{file}"));
                assert_eq!(
                    ExtensionChangeType::from_path(&p),
                    ExtensionChangeType::HooksConfig,
                    "expected HooksConfig for {}",
                    p.display()
                );
            }
        }
    }

    #[test]
    fn test_change_type_ignores_unrelated_hooks_json() {
        // A `hooks.json` under a plugin dir (not `.aleph`/`.claude`) must NOT
        // be classified as user-level hooks config — those go through the
        // plugin manifest path instead.
        assert_eq!(
            ExtensionChangeType::from_path(&PathBuf::from("/p/plugin-x/hooks/hooks.json")),
            ExtensionChangeType::Unknown
        );
    }

    #[test]
    fn test_internal_write_tracker_suppresses_in_window() {
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("foo.json");
        fs::write(&p, b"{}").unwrap();
        let tracker = InternalWriteTracker::default();
        assert!(!tracker.was_recent(&p));
        tracker.mark(&p);
        assert!(tracker.was_recent(&p));
    }

    #[test]
    fn test_internal_write_tracker_expires_after_ttl() {
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("foo.json");
        fs::write(&p, b"{}").unwrap();
        let tracker = InternalWriteTracker::with_ttl(Duration::from_millis(50));
        tracker.mark(&p);
        assert!(tracker.was_recent(&p));
        thread::sleep(Duration::from_millis(80));
        assert!(!tracker.was_recent(&p));
    }

    #[test]
    fn test_internal_write_tracker_missing_file_falls_back_to_raw_path() {
        // `canonicalize` fails for nonexistent paths — we still want mark/query
        // to round-trip on the raw path so writers can mark a path before the
        // write completes.
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("not-yet-created.json");
        let tracker = InternalWriteTracker::default();
        tracker.mark(&p);
        assert!(tracker.was_recent(&p));
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
    #[cfg(not(windows))] // notify-crate watch latency/semantics differ on Windows (flaky timing)
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
