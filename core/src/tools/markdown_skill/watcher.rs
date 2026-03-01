//! File system watcher for hot-reloading Markdown skills
//!
//! Monitors SKILL.md files for changes and triggers reload callbacks.

use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, DebouncedEvent, Debouncer, FileIdMap};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::error::{AlephError, Result};

use super::loader::SkillLoader;
use super::tool_adapter::MarkdownCliTool;

/// Hot reload event
#[derive(Debug, Clone)]
pub enum SkillEvent {
    /// A skill was created
    Created { path: PathBuf },
    /// A skill was modified
    Modified { path: PathBuf },
    /// A skill was deleted
    Deleted { path: PathBuf },
}

/// Callback for skill reload events
pub type ReloadCallback = Arc<dyn Fn(Vec<MarkdownCliTool>) -> Result<()> + Send + Sync>;

/// Configuration for skill watcher
#[derive(Debug, Clone)]
pub struct SkillWatcherConfig {
    /// Debounce duration (default: 500ms)
    pub debounce_duration: Duration,
    /// Whether to emit initial events for existing files
    pub emit_initial_events: bool,
}

impl Default for SkillWatcherConfig {
    fn default() -> Self {
        Self {
            debounce_duration: Duration::from_millis(500),
            emit_initial_events: false,
        }
    }
}

/// File system watcher for Markdown skills
pub struct SkillWatcher {
    _watcher: Debouncer<RecommendedWatcher, FileIdMap>,
    event_rx: mpsc::Receiver<DebouncedEvent>,
}

impl SkillWatcher {
    /// Create a new skill watcher
    ///
    /// # Arguments
    /// * `skills_dir` - Directory to watch for SKILL.md files
    /// * `reload_callback` - Callback to invoke when skills need reloading
    /// * `config` - Watcher configuration
    pub fn new(
        skills_dir: impl AsRef<Path>,
        _reload_callback: ReloadCallback,
        config: SkillWatcherConfig,
    ) -> Result<Self> {
        let skills_dir = skills_dir.as_ref().to_path_buf();

        // Create channel for file system events
        let (event_tx, event_rx) = mpsc::channel(100);

        // Create debounced watcher
        let mut debouncer = new_debouncer(
            config.debounce_duration,
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        for event in events {
                            // Send event through channel
                            if let Err(e) = event_tx.blocking_send(event) {
                                error!(error = %e, "Failed to send file event");
                            }
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            error!(error = %error, "File watcher error");
                        }
                    }
                }
            },
        )
        .map_err(|e| AlephError::Other {
            message: format!("Failed to create file watcher: {}", e),
            suggestion: Some("Check file system permissions".to_string()),
        })?;

        // Start watching the skills directory
        debouncer
            .watcher()
            .watch(&skills_dir, RecursiveMode::Recursive)
            .map_err(|e| AlephError::Other {
                message: format!("Failed to watch directory: {}", e),
                suggestion: Some("Ensure the directory exists and is readable".to_string()),
            })?;

        info!(
            dir = %skills_dir.display(),
            debounce_ms = config.debounce_duration.as_millis(),
            "Started skill watcher"
        );

        Ok(Self {
            _watcher: debouncer,
            event_rx,
        })
    }

    /// Run the watcher event loop
    ///
    /// This is a blocking async function that processes file events
    /// and invokes the reload callback when skills change.
    pub async fn run(
        mut self,
        skills_dir: PathBuf,
        reload_callback: ReloadCallback,
    ) -> Result<()> {
        info!("Skill watcher event loop started");

        while let Some(event) = self.event_rx.recv().await {
            debug!(event = ?event, "Received file system event");

            // Process only SKILL.md file changes
            let skill_events = Self::filter_skill_events(&event);

            if skill_events.is_empty() {
                continue;
            }

            // Reload affected skills
            if let Err(e) = Self::handle_skill_events(&skills_dir, &skill_events, &reload_callback).await {
                error!(error = %e, "Failed to handle skill events");
            }
        }

        info!("Skill watcher event loop ended");
        Ok(())
    }

    /// Filter events to only include SKILL.md file changes
    fn filter_skill_events(event: &DebouncedEvent) -> Vec<SkillEvent> {
        let mut skill_events = Vec::new();

        for path in &event.paths {
            if !Self::is_skill_file(path) {
                continue;
            }

            let skill_event = match &event.event.kind {
                EventKind::Create(_) => Some(SkillEvent::Created {
                    path: path.clone(),
                }),
                EventKind::Modify(_) => Some(SkillEvent::Modified {
                    path: path.clone(),
                }),
                EventKind::Remove(_) => Some(SkillEvent::Deleted {
                    path: path.clone(),
                }),
                _ => None,
            };

            if let Some(event) = skill_event {
                skill_events.push(event);
            }
        }

        skill_events
    }

    /// Check if a path is a SKILL.md file
    fn is_skill_file(path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|name| name.eq_ignore_ascii_case("SKILL.md"))
            .unwrap_or(false)
    }

    /// Handle skill reload events
    async fn handle_skill_events(
        _skills_dir: &Path,
        events: &[SkillEvent],
        reload_callback: &ReloadCallback,
    ) -> Result<()> {
        // Collect unique skill directories that need reloading
        let mut skill_dirs = std::collections::HashSet::new();

        for event in events {
            match event {
                SkillEvent::Created { path } | SkillEvent::Modified { path } => {
                    if let Some(parent) = path.parent() {
                        skill_dirs.insert(parent.to_path_buf());
                    }
                }
                SkillEvent::Deleted { path } => {
                    info!(path = %path.display(), "Skill deleted");
                    // For deletion, we would need to track and unregister the tool
                    // This requires ToolServer API extension
                    warn!("Skill deletion hot-reload not yet fully implemented");
                }
            }
        }

        if skill_dirs.is_empty() {
            return Ok(());
        }

        // Reload all affected skills
        let mut reloaded_tools = Vec::new();

        for skill_dir in skill_dirs {
            info!(dir = %skill_dir.display(), "Reloading skill");

            let loader = SkillLoader::new(skill_dir.clone());
            let (tools, errors) = loader.load_all().await;

            for (path, error) in errors {
                warn!(
                    path = %path.display(),
                    error = %error,
                    "Failed to reload skill"
                );
            }

            reloaded_tools.extend(tools);
        }

        // Invoke callback with reloaded tools
        if !reloaded_tools.is_empty() {
            info!(count = reloaded_tools.len(), "Reloaded skills, invoking callback");
            reload_callback(reloaded_tools)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;
    use tempfile::TempDir;

    #[test]
    fn test_is_skill_file() {
        assert!(SkillWatcher::is_skill_file(Path::new("SKILL.md")));
        assert!(SkillWatcher::is_skill_file(Path::new("/path/to/SKILL.md")));
        assert!(SkillWatcher::is_skill_file(Path::new("skill.md")));
        assert!(!SkillWatcher::is_skill_file(Path::new("README.md")));
        assert!(!SkillWatcher::is_skill_file(Path::new("skill.txt")));
    }

    #[tokio::test]
    async fn test_watcher_creation() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let reload_count = Arc::new(Mutex::new(0));
        let reload_count_clone = reload_count.clone();

        let callback: ReloadCallback = Arc::new(move |tools| {
            *reload_count_clone.lock().unwrap() += tools.len();
            Ok(())
        });

        let result = SkillWatcher::new(&skills_dir, callback, Default::default());
        assert!(result.is_ok());
    }
}
