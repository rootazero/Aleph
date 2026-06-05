//! Skill System v2 — Domain-Driven Skill Management
//!
//! This module provides the runtime infrastructure for skill registration,
//! eligibility evaluation, SKILL.md parsing, prompt injection, and a
//! unified `SkillSystem` facade for the rest of the application.

pub mod commands;
pub mod compat;
pub mod config;
pub mod cooccurrence;
pub mod eligibility;
pub mod events;
pub mod guard;
pub mod installer;
pub mod manifest;
pub mod preprocess;
pub mod prompt;
pub mod recaller;
pub mod registry;
mod shared;
pub mod snapshot;
pub mod status;
pub mod tools;
pub mod usage;

pub use commands::{list_available_commands, resolve_command, SkillCommandSpec};
pub use compat::SkillInfo;
pub use config::{
    InstallPreferences, NodeManager, SkillConfigUpdate, SkillEntryConfig, SkillsConfig,
};
pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
pub use events::SkillSystemEvent;
pub use guard::{
    install_allowed, merge_verdicts, scan_content, scan_skill_directory, ScanVerdict, ThreatLevel,
    TrustLevel,
};
pub use installer::{
    build_install_command, filter_install_specs_for_current_os, select_best_install,
    InstallExecutor, InstallResult,
};
pub use manifest::{parse_skill_content, parse_skill_file, SkillParseError};
pub use preprocess::{preprocess_skill_content, SkillPreprocessContext};
pub use prompt::build_skills_prompt_xml;
pub use registry::SkillRegistry;
pub use shared::shared_skill_system;
pub use snapshot::SkillSnapshot;
pub use status::{InstallOption, MissingRequirements, SkillStatusEntry, SkillStatusFilter};
pub use usage::{SkillState, UsageStats, UsageStore};
pub use cooccurrence::{cluster_chains, CoOccurrenceLog, RecentUse};

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
            .unwrap_or_else(|| {
                tracing::warn!("dirs::home_dir() returned None; falling back to current directory for skill data");
                PathBuf::from(".")
            })
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
        let config_value = crate::config::Config::load()
            .ok()
            .and_then(|c| serde_json::to_value(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let registry = self.inner.registry.read().await;
        let mut entries: Vec<SkillStatusEntry> = registry
            .list_all()
            .into_iter()
            .map(|m| {
                let result = self.inner.eligibility.evaluate(m, &config_value);
                SkillStatusEntry::build(m, &result, None, false, None)
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
        let mut registry = self.inner.registry.write().await;

        // Only emit events for manifests that were actually accepted by the registry
        // (higher priority sources replace lower ones; equal priority rejects newcomers).
        let events: Vec<SkillSystemEvent> = manifests
            .into_iter()
            .filter_map(|m| {
                let id = m.id().as_str().to_string();
                let name = m.name().to_string();
                if registry.register(m) {
                    Some(SkillSystemEvent::loaded(id, name))
                } else {
                    None
                }
            })
            .collect();

        drop(registry);

        for event in events {
            self.emit_event(event);
        }

        self.rebuild_snapshot().await;
    }

    /// Build full status entries for all skills, incorporating user config.
    ///
    /// Also merges per-skill activity telemetry from every registered
    /// `<skills_dir>/.usage.json` sidecar so consumers (Panel UI / LLM /
    /// CLI) can see usage counts and lifecycle state at a glance.
    pub async fn full_status(&self) -> Vec<SkillStatusEntry> {
        let config_value = crate::config::Config::load()
            .ok()
            .and_then(|c| serde_json::to_value(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let registry = self.inner.registry.read().await;
        let config = self.inner.config.read().await;
        let usage_index = self.collect_usage_snapshot().await;

        let mut entries: Vec<SkillStatusEntry> = registry
            .list_all()
            .into_iter()
            .map(|manifest| {
                let eligibility = self.inner.eligibility.evaluate(manifest, &config_value);
                let entry_config = config.get_entry(manifest.id());
                // Vault integration wired in RPC layer
                let api_key_set = false;
                let usage = usage_index.get(manifest.id().as_str()).cloned();
                SkillStatusEntry::build(manifest, &eligibility, entry_config, api_key_set, usage)
            })
            .collect();
        entries.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        entries
    }

    /// Read every registered skill dir's `.usage.json` and merge into a
    /// single `id → UsageStats` map. Later dirs overwrite earlier on the
    /// rare same-id collision (the registry would normally have already
    /// deduped at load time).
    async fn collect_usage_snapshot(&self) -> std::collections::HashMap<String, UsageStats> {
        let dirs = self.inner.skill_dirs.read().await.clone();
        let mut merged: std::collections::HashMap<String, UsageStats> = Default::default();
        for dir in &dirs {
            let store = UsageStore::new(dir);
            for (id, stats) in store.snapshot() {
                merged.insert(id, stats);
            }
        }
        merged
    }

    /// Record an LLM-driven mutation to a skill (install / enable / scope
    /// change). Bumps `patch_count` on the sidecar belonging to whichever
    /// registered dir owns this skill's `SKILL.md`. Best-effort — silently
    /// no-ops if the skill cannot be located on disk.
    pub async fn record_patch(&self, id: &SkillId) {
        let id_str = id.as_str();
        // Reject any path-separator or parent-dir references to prevent traversal.
        if id_str.contains("..") || id_str.contains('/') || id_str.contains('\\') {
            tracing::warn!(skill_id = %id_str, "record_patch: rejecting malformed skill id");
            return;
        }
        let dirs = self.inner.skill_dirs.read().await.clone();
        for dir in &dirs {
            let candidate = dir.join(id.as_str()).join("SKILL.md");
            if candidate.exists() {
                UsageStore::new(dir).record_patch(id.as_str());
                return;
            }
        }
        // Skill lives in a nested layout (e.g. plugin-installed under
        // `<dir>/<plugin>/<id>/SKILL.md`). Fall back to bumping any sidecar
        // that already has a row for this id so we update the right one
        // without creating fresh orphans in unrelated dirs.
        for dir in &dirs {
            let store = UsageStore::new(dir);
            if store.get(id.as_str()).is_some() {
                store.record_patch(id.as_str());
                return;
            }
        }
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
    /// On successful removal, also drops the skill's row from every
    /// registered `.usage.json` so the sidecar does not accumulate orphan
    /// telemetry over time. `forget` is idempotent so a missing row is
    /// silently ignored.
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
            let dirs = self.inner.skill_dirs.read().await.clone();
            for dir in &dirs {
                UsageStore::new(dir).forget(id.as_str());
            }
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

    /// Scan all registered directories, atomically replace the registry, and rebuild the snapshot.
    async fn rescan_dirs(&self) {
        let dirs = self.inner.skill_dirs.read().await.clone();

        // Build a fresh registry so we can swap atomically — never expose an empty registry.
        let mut new_registry = SkillRegistry::new();
        for dir in &dirs {
            if dir.exists() {
                let source = guess_source(dir);
                let manifests = scan_directory(dir, source);
                new_registry.register_all(manifests);
            }
        }

        let mut registry = self.inner.registry.write().await;
        *registry = new_registry;
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

        // Load config once; fall back to empty object on failure (defensive).
        let config_value = crate::config::Config::load()
            .ok()
            .and_then(|c| serde_json::to_value(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        // Snapshot the user's per-skill overrides (enable/disable, scope) so the
        // rebuilt snapshot reflects them in eligibility and prompt injection.
        // Cloned under a short-lived lock to avoid holding config + registry
        // locks simultaneously.
        let skill_entries = self.inner.config.read().await.entries.clone();

        let registry = self.inner.registry.read().await;
        let new_snapshot = SkillSnapshot::build(
            &registry,
            &self.inner.eligibility,
            current_version,
            &config_value,
            &skill_entries,
        );
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

        if let Ok(file_type) = entry.file_type() {
            if file_type.is_symlink() {
                continue;
            }
        }

        if path.is_file() && is_skill_file(&path) {
            match parse_skill_file(&path, source.clone()) {
                Ok(manifest) => manifests.push(manifest),
                Err(e) => {
                    tracing::warn!("failed to parse skill file {:?}: {}", path, e);
                }
            }
        }

        // Recurse into subdirectories (skip symlinks to avoid infinite loops)
        if let Ok(file_type) = entry.file_type() {
            if file_type.is_dir() && !file_type.is_symlink() {
                let sub = scan_directory(&path, source.clone());
                manifests.extend(sub);
            }
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
    static CACHED_MANIFEST: OnceLock<Option<crate::bundled::manifest::InstallRegistry>> =
        OnceLock::new();

    let path_str = path.to_string_lossy();

    if path_str.contains(".aleph/skills") {
        if let Some(home) = dirs::home_dir() {
            let home_skills = home.join(".aleph").join("skills");
            if path.starts_with(&home_skills) {
                // Under ~/.aleph/skills/ — check manifest to distinguish official from user
                let manifest = CACHED_MANIFEST
                    .get_or_init(|| crate::bundled::manifest::InstallRegistry::load(&home_skills));
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

// ---------------------------------------------------------------------------
// Self-growth: learned-skill validation
// ---------------------------------------------------------------------------

/// Valid skill categories.
pub const SKILL_CATEGORIES: &[&str] = &[
    "coding",
    "debugging",
    "workflow",
    "knowledge",
    "communication",
];

/// Validate a skill category.
pub fn is_valid_category(category: &str) -> bool {
    SKILL_CATEGORIES.contains(&category)
}

/// Validate a skill name (kebab-case, non-empty, ASCII alphanumeric + hyphens).
pub fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::SkillSource;

    #[test]
    fn valid_skill_names() {
        assert!(is_valid_skill_name("rust-lifetime-debugging"));
        assert!(is_valid_skill_name("git-rebase"));
        assert!(is_valid_skill_name("a"));
    }

    #[test]
    fn invalid_skill_names() {
        assert!(!is_valid_skill_name(""));
        assert!(!is_valid_skill_name("-leading"));
        assert!(!is_valid_skill_name("trailing-"));
        assert!(!is_valid_skill_name("has spaces"));
        assert!(!is_valid_skill_name("UpperCase"));
    }

    #[test]
    fn valid_categories() {
        assert!(is_valid_category("coding"));
        assert!(is_valid_category("debugging"));
        assert!(!is_valid_category("invalid"));
    }

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
