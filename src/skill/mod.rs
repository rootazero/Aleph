//! Skill System v2 — Domain-Driven Skill Management
//!
//! This module provides the runtime infrastructure for skill registration,
//! eligibility evaluation, SKILL.md parsing, prompt injection, and a
//! unified `SkillSystem` facade for the rest of the application.

pub mod commands;
pub mod compat;
pub mod config;
pub mod eligibility;
pub mod events;
pub mod installer;
pub mod manifest;
pub mod prompt;
pub mod registry;
pub mod snapshot;
pub mod status;

pub use commands::{list_available_commands, resolve_command, SkillCommandSpec};
pub use compat::SkillInfo;
pub use config::{
    InstallPreferences, NodeManager, SkillConfigUpdate, SkillEntryConfig, SkillsConfig,
};
pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
pub use events::SkillSystemEvent;
pub use installer::{
    build_install_command, filter_install_specs_for_current_os, select_best_install,
    InstallExecutor, InstallResult,
};
pub use manifest::{parse_skill_content, parse_skill_file, SkillParseError};
pub use prompt::build_skills_prompt_xml;
pub use registry::SkillRegistry;
pub use snapshot::SkillSnapshot;
pub use status::{InstallOption, MissingRequirements, SkillStatusEntry, SkillStatusFilter};

use crate::sync_primitives::Arc;
use std::path::{Path, PathBuf};

use tokio::sync::RwLock;

use crate::domain::skill::{SkillId, SkillManifest, SkillSource};
use crate::domain::Entity;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur in the skill system.
#[derive(Debug)]
pub enum SkillSystemError {
    /// Error parsing a skill file.
    Parse(SkillParseError),
    /// I/O error.
    Io(std::io::Error),
}

impl std::fmt::Display for SkillSystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "skill parse error: {}", e),
            Self::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for SkillSystemError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(e) => Some(e),
            Self::Io(e) => Some(e),
        }
    }
}

impl From<SkillParseError> for SkillSystemError {
    fn from(e: SkillParseError) -> Self {
        Self::Parse(e)
    }
}

impl From<std::io::Error> for SkillSystemError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// SkillSystem facade
// ---------------------------------------------------------------------------

/// The main entry point for the skill system.
///
/// `SkillSystem` is cheaply cloneable (via `Arc`) and provides async-safe
/// access to the skill registry, eligibility evaluation, snapshots, and
/// slash command resolution.
#[derive(Clone)]
pub struct SkillSystem {
    inner: Arc<Inner>,
}

struct Inner {
    registry: RwLock<SkillRegistry>,
    snapshot: RwLock<SkillSnapshot>,
    skill_dirs: RwLock<Vec<PathBuf>>,
    version_counter: RwLock<u64>,
    eligibility: EligibilityService,
    config: RwLock<SkillsConfig>,
    config_path: PathBuf,
    event_tx: tokio::sync::broadcast::Sender<SkillSystemEvent>,
}

impl SkillSystem {
    /// Create a new, empty skill system.
    pub fn new() -> Self {
        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("data");
        let config_path = data_dir.join("skills.toml");
        let config = SkillsConfig::load(&config_path);
        let (event_tx, _) = tokio::sync::broadcast::channel(64);

        Self {
            inner: Arc::new(Inner {
                registry: RwLock::new(SkillRegistry::new()),
                snapshot: RwLock::new(SkillSnapshot::empty()),
                skill_dirs: RwLock::new(Vec::new()),
                version_counter: RwLock::new(0),
                eligibility: EligibilityService::new(),
                config: RwLock::new(config),
                config_path,
                event_tx,
            }),
        }
    }

    /// Initialize the skill system by scanning the given directories.
    ///
    /// Each directory is scanned for SKILL.md files. The source is guessed
    /// from the path. After scanning, a snapshot is built.
    pub async fn init(&self, dirs: Vec<PathBuf>) -> Result<(), SkillSystemError> {
        {
            let mut skill_dirs = self.inner.skill_dirs.write().await;
            *skill_dirs = dirs;
        }
        self.rescan_dirs().await;
        Ok(())
    }

    /// Rebuild the snapshot from the current registry state.
    ///
    /// Re-scans all directories, increments the version counter, and builds a new snapshot.
    pub async fn rebuild(&self) -> Result<(), SkillSystemError> {
        self.rescan_dirs().await;
        Ok(())
    }

    /// Reload a single skill file into the registry and rebuild the snapshot.
    pub async fn reload_file(&self, path: impl AsRef<Path>) -> Result<(), SkillSystemError> {
        let path = path.as_ref();
        let source = guess_source(path);
        let manifest = parse_skill_file(path, source)?;

        let mut registry = self.inner.registry.write().await;
        registry.register(manifest);
        drop(registry);

        self.rebuild_snapshot().await;

        Ok(())
    }

    /// Get a clone of the current snapshot.
    pub async fn current_snapshot(&self) -> SkillSnapshot {
        self.inner.snapshot.read().await.clone()
    }

    /// Get a skill manifest by ID.
    pub async fn get_skill(&self, id: &SkillId) -> Option<SkillManifest> {
        self.inner.registry.read().await.get(id).cloned()
    }

    /// List all registered skill manifests.
    pub async fn list_skills(&self) -> Vec<SkillManifest> {
        self.inner
            .registry
            .read()
            .await
            .list_all()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Build status entries for all registered skills.
    pub async fn skill_status(&self) -> Vec<SkillStatusEntry> {
        let registry = self.inner.registry.read().await;
        let mut entries: Vec<SkillStatusEntry> = registry
            .list_all()
            .into_iter()
            .map(|m| {
                let result = self.inner.eligibility.evaluate(m);
                SkillStatusEntry::build(m, &result, None, false)
            })
            .collect();
        entries.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        entries
    }

    /// Resolve a slash command name to a skill command spec.
    pub async fn resolve_command(&self, name: &str) -> Option<SkillCommandSpec> {
        let registry = self.inner.registry.read().await;
        commands::resolve_command(name, &registry)
    }

    /// Register skills from external sources (plugins, markdown).
    pub async fn register_external(&self, manifests: Vec<SkillManifest>) {
        let events: Vec<SkillSystemEvent> = manifests
            .iter()
            .map(|m| SkillSystemEvent::loaded(m.id().as_str(), m.name()))
            .collect();

        let mut registry = self.inner.registry.write().await;
        registry.register_all(manifests);
        drop(registry);

        for event in events {
            self.emit_event(event);
        }

        self.rebuild_snapshot().await;
    }

    /// Build full status entries for all skills, incorporating user config.
    pub async fn full_status(&self) -> Vec<SkillStatusEntry> {
        let registry = self.inner.registry.read().await;
        let config = self.inner.config.read().await;

        let mut entries: Vec<SkillStatusEntry> = registry
            .list_all()
            .into_iter()
            .map(|manifest| {
                let eligibility = self.inner.eligibility.evaluate(manifest);
                let entry_config = config.get_entry(manifest.id());
                // Vault integration wired in RPC layer
                let api_key_set = false;
                SkillStatusEntry::build(manifest, &eligibility, entry_config, api_key_set)
            })
            .collect();
        entries.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        entries
    }

    /// Update a skill's configuration and persist to disk.
    pub async fn update_config(
        &self,
        id: &SkillId,
        update: SkillConfigUpdate,
    ) -> Result<(), std::io::Error> {
        let mut config = self.inner.config.write().await;
        config.apply_update(id, update);
        config.save(&self.inner.config_path)?;
        drop(config);
        self.rebuild_snapshot().await;
        Ok(())
    }

    /// Install a dependency for a skill.
    pub async fn install_dependency(&self, id: &SkillId, spec_id: Option<&str>) -> InstallResult {
        let registry = self.inner.registry.read().await;
        let manifest = match registry.get(id) {
            Some(m) => m.clone(),
            None => {
                return InstallResult {
                    success: false,
                    message: format!("Skill not found: {}", id.as_str()),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                };
            }
        };
        drop(registry);

        let config = self.inner.config.read().await;
        let prefs = config.install_preferences.clone();
        drop(config);

        let spec = if let Some(spec_id) = spec_id {
            manifest
                .install_specs()
                .iter()
                .find(|s| s.id == spec_id)
                .cloned()
        } else {
            select_best_install(manifest.install_specs(), &prefs).cloned()
        };

        let spec = match spec {
            Some(s) => s,
            None => {
                return InstallResult {
                    success: false,
                    message: "No matching install spec found".to_string(),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                };
            }
        };

        let result = InstallExecutor::run(&spec, &prefs).await;
        if result.success {
            self.rebuild_snapshot().await;
        }
        result
    }

    /// Remove a skill from the registry. Bundled skills cannot be removed.
    pub async fn remove_skill(&self, id: &SkillId) -> Result<bool, std::io::Error> {
        let mut registry = self.inner.registry.write().await;
        if let Some(m) = registry.get(id) {
            if matches!(m.source(), SkillSource::Bundled) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Cannot remove bundled skills",
                ));
            }
        }
        let removed = registry.remove(id);
        drop(registry);
        if removed {
            self.emit_event(SkillSystemEvent::removed(id.as_str()));
            self.rebuild_snapshot().await;
        }
        Ok(removed)
    }

    /// Subscribe to skill system events.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<SkillSystemEvent> {
        self.inner.event_tx.subscribe()
    }

    // --- Private helpers ---

    /// Scan all registered directories, repopulate the registry, and rebuild the snapshot.
    async fn rescan_dirs(&self) {
        let dirs = self.inner.skill_dirs.read().await.clone();

        let mut registry = self.inner.registry.write().await;
        registry.clear();

        for dir in &dirs {
            if dir.exists() {
                let source = guess_source(dir);
                let manifests = scan_directory(dir, source);
                registry.register_all(manifests);
            }
        }

        drop(registry);
        self.rebuild_snapshot().await;
    }

    /// Emit a skill system event to all subscribers.
    fn emit_event(&self, event: SkillSystemEvent) {
        let _ = self.inner.event_tx.send(event);
    }

    /// Increment the version counter and build a new snapshot.
    async fn rebuild_snapshot(&self) {
        let mut version = self.inner.version_counter.write().await;
        *version += 1;
        let current_version = *version;
        drop(version);

        let registry = self.inner.registry.read().await;
        let new_snapshot =
            SkillSnapshot::build(&registry, &self.inner.eligibility, current_version);
        let skill_ids: Vec<String> = registry
            .list_all()
            .iter()
            .map(|m| m.id().as_str().to_string())
            .collect();
        let count = skill_ids.len();
        drop(registry);

        let mut snapshot = self.inner.snapshot.write().await;
        *snapshot = new_snapshot;
        drop(snapshot);

        self.emit_event(SkillSystemEvent::all_reloaded(count, skill_ids));
    }
}

impl Default for SkillSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SkillSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillSystem")
            .field("arc_strong_count", &Arc::strong_count(&self.inner))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Scan a directory for SKILL.md files and parse them.
///
/// Non-parseable files are silently skipped.
fn scan_directory(dir: &Path, source: SkillSource) -> Vec<SkillManifest> {
    let mut manifests = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return manifests,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_file() && is_skill_file(&path) {
            match parse_skill_file(&path, source.clone()) {
                Ok(manifest) => manifests.push(manifest),
                Err(e) => {
                    tracing::warn!("failed to parse skill file {:?}: {}", path, e);
                }
            }
        }

        // Recurse into subdirectories
        if path.is_dir() {
            let sub = scan_directory(&path, source.clone());
            manifests.extend(sub);
        }
    }

    manifests
}

/// Check if a file looks like a SKILL.md file.
fn is_skill_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("SKILL.md"))
        .unwrap_or(false)
}

/// Return the standard skill directories used when no project context is available.
///
/// Scans the canonical user-level locations:
/// - `~/.aleph/skills/` — Aleph native global skills
/// - `~/.claude/skills/` — Claude Code compatibility
///
/// Only directories that actually exist are returned.
pub fn default_skill_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(home) = dirs::home_dir() {
        let aleph_skills = home.join(".aleph").join("skills");
        if aleph_skills.exists() {
            dirs.push(aleph_skills);
        }

        let claude_skills = home.join(".claude").join("skills");
        if claude_skills.exists() {
            dirs.push(claude_skills);
        }
    }

    dirs
}

/// Guess the `SkillSource` from a file path.
///
/// - Under `~/.aleph/skills/` with manifest marking official → Bundled
/// - Under `~/.aleph/skills/` otherwise → Global
/// - Contains `.aleph/skills` but not under home → Workspace
/// - Otherwise → Bundled (e.g. Claude Code compatibility paths)
fn guess_source(path: &Path) -> SkillSource {
    use std::sync::OnceLock;

    // Cache the bundled manifest to avoid re-reading from disk on every call.
    static CACHED_MANIFEST: OnceLock<Option<crate::bundled::manifest::SkillManifest>> =
        OnceLock::new();

    let path_str = path.to_string_lossy();

    if path_str.contains(".aleph/skills") {
        if let Some(home) = dirs::home_dir() {
            let home_skills = home.join(".aleph").join("skills");
            if path.starts_with(&home_skills) {
                // Under ~/.aleph/skills/ — check manifest to distinguish official from user
                let manifest = CACHED_MANIFEST
                    .get_or_init(|| crate::bundled::manifest::SkillManifest::load(&home_skills));
                if let Some(manifest) = manifest {
                    if let Ok(relative) = path.strip_prefix(&home_skills) {
                        if let Some(skill_name) = relative.components().next() {
                            let name = skill_name.as_os_str().to_string_lossy();
                            if manifest.is_official(&name) {
                                return SkillSource::Bundled;
                            }
                        }
                    }
                }
                return SkillSource::Global;
            }
        } else {
            tracing::warn!("dirs::home_dir() returned None, defaulting to Global source");
            return SkillSource::Global;
        }
        // Path contains .aleph/skills but NOT under home → project-level workspace skill
        return SkillSource::Workspace;
    }

    // Claude Code compatibility paths (.claude/skills) or plugin skills
    SkillSource::Bundled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::SkillSource;

    #[test]
    fn clone_shares_state() {
        let sys1 = SkillSystem::new();
        let sys2 = sys1.clone();

        // Both point to the same Arc
        assert!(Arc::ptr_eq(&sys1.inner, &sys2.inner));
    }

    #[tokio::test]
    async fn init_with_temp_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        let content = r#"---
name: Test Skill
description: A test skill for unit tests
---
You are a test expert."#;
        std::fs::write(&skill_file, content).unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let snapshot = system.current_snapshot().await;
        assert!(snapshot.version > 0);
        assert!(!snapshot.eligible.is_empty());

        let skills = system.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name(), "Test Skill");
    }

    #[tokio::test]
    async fn rebuild_increments_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        let content = r#"---
name: Version Test
description: Tests version increments
---
Content."#;
        std::fs::write(&skill_file, content).unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let v1 = system.current_snapshot().await.version;

        system.rebuild().await.unwrap();
        let v2 = system.current_snapshot().await.version;

        system.rebuild().await.unwrap();
        let v3 = system.current_snapshot().await.version;

        assert!(v2 > v1);
        assert!(v3 > v2);
    }

    #[tokio::test]
    async fn list_skills() {
        let dir = tempfile::TempDir::new().unwrap();

        // Create two skill subdirectories with SKILL.md files
        let sub1 = dir.path().join("skill1");
        std::fs::create_dir(&sub1).unwrap();
        std::fs::write(
            sub1.join("SKILL.md"),
            r#"---
name: Skill One
description: First skill
---
Content one."#,
        )
        .unwrap();

        let sub2 = dir.path().join("skill2");
        std::fs::create_dir(&sub2).unwrap();
        std::fs::write(
            sub2.join("SKILL.md"),
            r#"---
name: Skill Two
description: Second skill
---
Content two."#,
        )
        .unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let skills = system.list_skills().await;
        assert_eq!(skills.len(), 2);

        let names: Vec<&str> = skills.iter().map(|s| s.name()).collect();
        assert!(names.contains(&"Skill One"));
        assert!(names.contains(&"Skill Two"));
    }

    #[tokio::test]
    async fn resolve_command_through_facade() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        std::fs::write(
            &skill_file,
            r#"---
name: Git Commit
description: Helps with git commits
---
Git expert."#,
        )
        .unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        // The ID will be "git-commit" (derived from name by parser)
        let result = system.resolve_command("git-commit").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "Git Commit");
    }

    #[tokio::test]
    async fn skill_status_reports() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        std::fs::write(
            &skill_file,
            r#"---
name: Status Test
description: Tests status reporting
---
Content."#,
        )
        .unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let statuses = system.skill_status().await;
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].eligible);
    }

    #[test]
    fn guess_source_non_aleph_path_is_bundled() {
        // Paths outside .aleph/skills (e.g. system-installed) default to Bundled
        let path = PathBuf::from("/usr/local/share/aleph/skills/self/SKILL.md");
        assert_eq!(guess_source(&path), SkillSource::Bundled);
    }

    #[test]
    fn guess_source_workspace() {
        let path = PathBuf::from("/some/project/.aleph/skills/git/SKILL.md");
        assert_eq!(guess_source(&path), SkillSource::Workspace);
    }

    #[test]
    fn guess_source_bundled_fallback() {
        let path = PathBuf::from("/usr/share/aleph/skills/git/SKILL.md");
        assert_eq!(guess_source(&path), SkillSource::Bundled);
    }

    #[test]
    fn is_skill_file_detection() {
        assert!(is_skill_file(Path::new("/some/dir/SKILL.md")));
        assert!(is_skill_file(Path::new("/some/dir/skill.md")));
        assert!(!is_skill_file(Path::new("/some/dir/README.md")));
        assert!(!is_skill_file(Path::new("/some/dir/")));
    }

    #[tokio::test]
    async fn register_external_skills() {
        use crate::domain::skill::{PluginId, SkillContent};
        let system = SkillSystem::new();
        let manifest = SkillManifest::new(
            "plugin:test",
            "Test Plugin Skill",
            "From a plugin",
            SkillContent::new("content"),
            SkillSource::Plugin(PluginId::new("test-plugin")),
        );
        system.register_external(vec![manifest]).await;
        let skills = system.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name(), "Test Plugin Skill");
    }

    #[tokio::test]
    async fn full_status_returns_entries() {
        use crate::domain::skill::SkillContent;
        let system = SkillSystem::new();
        let manifest = SkillManifest::new(
            "test:skill",
            "Test Skill",
            "A test",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        system.register_external(vec![manifest]).await;
        let entries = system.full_status().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Test Skill");
        assert!(entries[0].eligible);
    }

    #[tokio::test]
    async fn remove_skill_from_registry() {
        use crate::domain::skill::SkillContent;
        let system = SkillSystem::new();
        let manifest = SkillManifest::new(
            "test:removable",
            "Removable",
            "desc",
            SkillContent::new("c"),
            SkillSource::Global,
        );
        system.register_external(vec![manifest]).await;
        assert_eq!(system.list_skills().await.len(), 1);

        let removed = system
            .remove_skill(&SkillId::new("test:removable"))
            .await
            .unwrap();
        assert!(removed);
        assert_eq!(system.list_skills().await.len(), 0);
    }

    #[tokio::test]
    async fn remove_skill_rejects_bundled() {
        use crate::domain::skill::SkillContent;
        let system = SkillSystem::new();
        let manifest = SkillManifest::new(
            "test:bundled",
            "Bundled Skill",
            "desc",
            SkillContent::new("c"),
            SkillSource::Bundled,
        );
        system.register_external(vec![manifest]).await;

        let result = system.remove_skill(&SkillId::new("test:bundled")).await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        // Skill should still be there
        assert_eq!(system.list_skills().await.len(), 1);
    }

    #[tokio::test]
    async fn subscribe_receives_events() {
        use crate::domain::skill::SkillContent;
        let system = SkillSystem::new();
        let mut rx = system.subscribe();

        let manifest = SkillManifest::new(
            "test:event",
            "Event Test",
            "desc",
            SkillContent::new("c"),
            SkillSource::Global,
        );
        system.register_external(vec![manifest]).await;

        // Should receive an event
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(event.is_ok());
    }
}
