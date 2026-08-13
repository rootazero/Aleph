//! Skill System v2 — Domain-Driven Skill Management
//!
//! This module provides the runtime infrastructure for skill registration,
//! eligibility evaluation, SKILL.md parsing, prompt injection, and a
//! unified `SkillSystem` facade for the rest of the application.

pub mod compat;
pub mod config;
pub mod cooccurrence;
pub mod eligibility;
pub mod guard;
pub mod installer;
pub mod manifest;
pub mod preprocess;
pub mod prompt;
pub mod registry;
mod shared;
pub mod snapshot;
pub mod status;
pub mod usage;

pub use compat::SkillInfo;
pub use config::{InstallPreferences, SkillConfigUpdate, SkillEntryConfig, SkillsConfig};
pub use cooccurrence::{cluster_chains, CoOccurrenceLog, RecentUse};
pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
pub use guard::{
    install_allowed, merge_verdicts, scan_content, scan_skill_directory, ScanVerdict, ThreatLevel,
    TrustLevel,
};
pub use installer::{
    build_install_command, filter_install_specs_for_current_os, select_best_install,
    InstallExecutor, InstallResult,
};
pub use manifest::{automation_notice, parse_skill_content, parse_skill_file, SkillParseError};
pub use preprocess::{preprocess_skill_content, SkillPreprocessContext};
pub use prompt::{build_skills_prompt_xml, SkillPromptBudget};
pub use registry::SkillRegistry;
pub use shared::{ensure_shared_skill_system_initialized, shared_skill_system};
pub use snapshot::SkillSnapshot;
pub use status::{InstallOption, MissingRequirements, SkillStatusEntry, SkillStatusFilter};
pub use usage::{record_use_in_dir, SkillState, UsageStats, UsageStore};

use crate::sync_primitives::Arc;
use std::path::{Path, PathBuf};

use tokio::sync::RwLock;

use crate::domain::skill::{SkillId, SkillManifest, SkillSource};
use crate::domain::Entity;

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
}

impl SkillSystem {
    /// Create a new, empty skill system.
    #[must_use]
    pub fn new() -> Self {
        // `ALEPH_HOME`-aware: skills.toml is Aleph state, and `skill_install`
        // / `self_manage` address it through the same resolver.
        let data_dir = crate::utils::paths::get_config_dir()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "cannot resolve Aleph config dir; falling back to current directory for skill data");
                PathBuf::from(".aleph")
            })
            .join("data");
        let config_path = data_dir.join("skills.toml");
        let config = SkillsConfig::load(&config_path);

        Self {
            inner: Arc::new(Inner {
                registry: RwLock::new(SkillRegistry::new()),
                snapshot: RwLock::new(SkillSnapshot::empty()),
                skill_dirs: RwLock::new(Vec::new()),
                version_counter: RwLock::new(0),
                eligibility: EligibilityService::new(),
                config: RwLock::new(config),
                config_path,
            }),
        }
    }

    /// Initialize the skill system by scanning the given directories.
    ///
    /// Each directory is scanned for SKILL.md files. The source is guessed
    /// from the path. After scanning, a snapshot is built.
    pub async fn init(&self, dirs: Vec<PathBuf>) {
        {
            let mut skill_dirs = self.inner.skill_dirs.write().await;
            *skill_dirs = dirs;
        }
        self.rescan_dirs().await;
    }

    /// Reload a single skill file into the registry and rebuild the snapshot.
    pub async fn reload_file(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), SkillParseError> {
        let path = path.as_ref();
        let source = guess_source(path);
        let manifest = parse_skill_file(path, source)?;

        let mut registry = self.inner.registry.write().await;
        registry.replace(manifest);
        drop(registry);

        self.rebuild_snapshot().await;

        Ok(())
    }

    /// Ensure `dir` participates in directory rescans (idempotent).
    ///
    /// Used by the authoring path so a freshly created `~/.aleph/skills/`
    /// is picked up by subsequent `rebuild()` calls even when it did not
    /// exist at `init()` time.
    pub async fn ensure_dir_registered(&self, dir: PathBuf) {
        let mut dirs = self.inner.skill_dirs.write().await;
        if !dirs.iter().any(|d| d == &dir) {
            dirs.push(dir);
        }
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

    /// Whether the skill's primary API-key env var is present in the process
    /// environment.
    ///
    /// Mirrors the `required_env` source-of-truth used by
    /// [`crate::skill::eligibility::EligibilityService`] (which checks
    /// `std::env::var`), so the status surface stays consistent with
    /// eligibility instead of always reporting "key missing". Skills with no
    /// `primary_env` declared have no key to set and return `false` (the
    /// status builder only consults this flag when `primary_env` is `Some`).
    fn api_key_present(manifest: &SkillManifest) -> bool {
        manifest
            .primary_env()
            .is_some_and(|env| std::env::var(env).is_ok())
    }

    /// Test-only helper: seed pre-built manifests straight into the registry.
    ///
    /// Production plugin/markdown skills are NOT registered this way — they flow
    /// through the dir-based path (`extension/mod.rs::load_all` ->
    /// `skill_system.init` skill_dirs + `publish_plugin_skill_dirs` ->
    /// `get_all_skills_dirs`), which `rescan_dirs` rebuilds from scratch (and
    /// would therefore wipe any manifest inserted here). This helper exists only
    /// to seed unit tests that exercise `full_status` / `remove_skill`
    /// without touching the filesystem; it is compiled out of production builds.
    #[cfg(test)]
    pub(crate) async fn insert_manifests_for_test(&self, manifests: Vec<SkillManifest>) {
        let mut registry = self.inner.registry.write().await;
        for m in manifests {
            registry.register(m);
        }
        drop(registry);

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
                let api_key_set = Self::api_key_present(manifest);
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

    /// Find the registered skills dir that owns this skill id.
    ///
    /// Prefers a dir with the flat `<dir>/<id>/SKILL.md` layout; falls back
    /// to any dir whose `.usage.json` sidecar already tracks the id (nested
    /// plugin layouts). Returns `None` for malformed ids (traversal guard)
    /// or when no registered dir knows the skill.
    async fn owning_dir(&self, id: &SkillId) -> Option<PathBuf> {
        let id_str = id.as_str();
        if id_str.contains("..") || id_str.contains('/') || id_str.contains('\\') {
            tracing::warn!(skill_id = %id_str, "owning_dir: rejecting malformed skill id");
            return None;
        }
        let dirs = self.inner.skill_dirs.read().await.clone();
        let owned_id = id_str.to_string();
        tokio::task::spawn_blocking(move || {
            for dir in &dirs {
                if dir.join(&owned_id).join("SKILL.md").exists() {
                    return Some(dir.clone());
                }
            }
            for dir in &dirs {
                if UsageStore::new(dir).get(&owned_id).is_some() {
                    return Some(dir.clone());
                }
            }
            None
        })
        .await
        .unwrap_or(None)
    }

    /// Find the on-disk directory whose `SKILL.md` parses to `id`, without
    /// assuming the directory basename equals the derived id. The flat layout
    /// stores skills under `<root>/<install-slug>/SKILL.md` while the id comes
    /// from the `name:` frontmatter, so the two can differ. Scans only the
    /// immediate subdirectories of each registered root.
    async fn skill_dir_for_id(&self, id: &SkillId) -> Option<PathBuf> {
        let id_str = id.as_str();
        if id_str.contains("..") || id_str.contains('/') || id_str.contains('\\') {
            return None;
        }
        let dirs = self.inner.skill_dirs.read().await.clone();
        let owned_id = id_str.to_string();
        tokio::task::spawn_blocking(move || {
            for root in &dirs {
                let entries = match std::fs::read_dir(root) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for entry in entries.flatten() {
                    let sub = entry.path();
                    if !sub.is_dir() {
                        continue;
                    }
                    let md = sub.join("SKILL.md");
                    if !md.exists() {
                        continue;
                    }
                    if let Ok(m) = parse_skill_file(&md, guess_source(&sub)) {
                        if m.id().as_str() == owned_id {
                            return Some(sub);
                        }
                    }
                }
            }
            None
        })
        .await
        .unwrap_or(None)
    }

    /// Locate the on-disk `SKILL.md` for a skill in the flat
    /// `<skills_dir>/<id>/SKILL.md` layout. Returns `None` for nested
    /// (plugin) layouts and malformed ids.
    pub async fn locate_skill_file(&self, id: &SkillId) -> Option<PathBuf> {
        let dir = self.owning_dir(id).await?;
        let candidate = dir.join(id.as_str()).join("SKILL.md");
        candidate.exists().then_some(candidate)
    }

    /// Per-skill activity telemetry from the owning dir's `.usage.json`.
    pub async fn usage_for(&self, id: &SkillId) -> Option<UsageStats> {
        let dir = self.owning_dir(id).await?;
        UsageStore::new(dir).get(id.as_str())
    }

    /// Pin or unpin a skill. Pinned skills are exempt from lifecycle
    /// auto-transitions (see `memory::dreaming::stages::SkillLifecycleStage`)
    /// and refuse `delete` until unpinned. Best-effort no-op when the skill
    /// cannot be located.
    pub async fn set_pinned(&self, id: &SkillId, pinned: bool) {
        if let Some(dir) = self.owning_dir(id).await {
            UsageStore::new(dir).set_pinned(id.as_str(), pinned);
        }
    }

    /// Set the lifecycle state of a skill (`active` / `stale` / `archived`).
    ///
    /// Archived skills stay in the registry and status surfaces but are
    /// excluded from the injected `<available_skills>` prompt index, so the
    /// snapshot is rebuilt after the flip.
    pub async fn set_skill_state(&self, id: &SkillId, state: SkillState) {
        if let Some(dir) = self.owning_dir(id).await {
            UsageStore::new(dir).set_state(id.as_str(), state);
            self.rebuild_snapshot().await;
        }
    }

    /// Record that a skill was actually used, for callers that hold only the
    /// id (the `/<skill>` slash path). Resolves the owning dir, then defers to
    /// [`usage::record_use_in_dir`] — the single definition of what a use is.
    ///
    /// Best-effort: silently no-ops when no registered dir owns the id, which
    /// is the correct answer for plugin *commands* (registered as pseudo-skills
    /// under a `plugin:command` id with no `SKILL.md` of their own).
    pub async fn record_use(&self, id: &SkillId) {
        if let Some(dir) = self.owning_dir(id).await {
            let owned = id.as_str().to_string();
            let _ =
                tokio::task::spawn_blocking(move || usage::record_use_in_dir(dir, &owned)).await;
        }
    }

    /// Record an LLM-driven mutation to a skill (install / enable / scope
    /// change). Bumps `patch_count` on the sidecar belonging to whichever
    /// registered dir owns this skill's `SKILL.md`. Best-effort — silently
    /// no-ops if the skill cannot be located on disk.
    pub async fn record_patch(&self, id: &SkillId) {
        if let Some(dir) = self.owning_dir(id).await {
            UsageStore::new(dir).record_patch(id.as_str());
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
        let install_specs = {
            let registry = self.inner.registry.read().await;
            match registry.get(id) {
                Some(m) => m.install_specs().to_vec(),
                None => {
                    return InstallResult {
                        success: false,
                        message: format!("Skill not found: {}", id.as_str()),
                        stdout: String::new(),
                        stderr: String::new(),
                        exit_code: None,
                    };
                }
            }
        };

        let config = self.inner.config.read().await;
        let prefs = config.install_preferences.clone();
        drop(config);

        let spec = if let Some(spec_id) = spec_id {
            install_specs.iter().find(|s| s.id == spec_id).cloned()
        } else {
            select_best_install(&install_specs, &prefs).cloned()
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
    ///
    /// For Global / Workspace skills in the flat `<dir>/<id>/SKILL.md`
    /// layout the backing directory is also deleted — without this, the
    /// next directory rescan would resurrect a skill the user removed.
    pub async fn remove_skill(&self, id: &SkillId) -> Result<bool, std::io::Error> {
        let mut registry = self.inner.registry.write().await;
        let mut deletable_on_disk = false;
        if let Some(m) = registry.get(id) {
            if matches!(m.source(), SkillSource::Bundled) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Cannot remove bundled skills",
                ));
            }
            deletable_on_disk = matches!(m.source(), SkillSource::Global | SkillSource::Workspace);
        }
        let removed = registry.remove(id);
        drop(registry);
        if removed {
            if deletable_on_disk {
                // Resolve the backing directory by matching the parsed id, not by
                // assuming `dirname == id` — otherwise a skill whose install-slug
                // directory differs from its frontmatter-derived id survives on
                // disk and the next rescan resurrects it.
                if let Some(skill_dir) = self.skill_dir_for_id(id).await {
                    if let Err(e) = std::fs::remove_dir_all(&skill_dir) {
                        tracing::warn!(
                            skill_id = %id.as_str(),
                            path = %skill_dir.display(),
                            error = %e,
                            "remove_skill: failed to delete skill directory"
                        );
                    }
                }
            }
            let dirs = self.inner.skill_dirs.read().await.clone();
            for dir in &dirs {
                UsageStore::new(dir).forget(id.as_str());
            }
            self.rebuild_snapshot().await;
        }
        Ok(removed)
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

        // Snapshot the user's per-skill overrides (enable/disable, scope) and the
        // configured prompt budget so the rebuilt snapshot reflects them in
        // eligibility, prompt injection, and the budget carried to the layer.
        // Read under a single short-lived lock to avoid holding config +
        // registry locks simultaneously.
        let (skill_entries, prompt_budget) = {
            let cfg = self.inner.config.read().await;
            (cfg.entries.clone(), cfg.prompt_budget)
        };

        // Archived skills (LLM-curated via `skill_manage` or future dreaming
        // stages) stay listed in status surfaces but must not occupy prompt
        // budget — collect their ids so the snapshot can exclude them from
        // the injected index.
        let archived: std::collections::HashSet<String> = self
            .collect_usage_snapshot()
            .await
            .into_iter()
            .filter(|(_, stats)| stats.state == SkillState::Archived)
            .map(|(id, _)| id)
            .collect();

        let registry = self.inner.registry.read().await;
        let mut new_snapshot = SkillSnapshot::build(
            &registry,
            &self.inner.eligibility,
            current_version,
            &config_value,
            &skill_entries,
            &archived,
        );
        // Carry the configured budget to consumers (the prompt layer renders
        // the authoritative index with it; `build` only computes the
        // default-budget preview).
        new_snapshot.prompt_budget = prompt_budget;
        drop(registry);

        let mut snapshot = self.inner.snapshot.write().await;
        *snapshot = new_snapshot;
        drop(snapshot);
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
        .is_some_and(|n| n.eq_ignore_ascii_case("SKILL.md"))
}

/// Return the standard skill directories used when no project context is available.
///
/// Scans the canonical user-level locations:
/// - `~/.aleph/skills/` — Aleph native global skills
/// - `~/.claude/skills/` — Claude Code compatibility
///
/// Only directories that actually exist are returned.
#[must_use]
pub fn default_skill_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(aleph_skills) = crate::utils::paths::get_skills_dir() {
        if aleph_skills.exists() {
            dirs.push(aleph_skills);
        }
    }

    if let Ok(home) = crate::utils::paths::get_home_dir() {
        let claude_skills = home.join(".claude").join("skills");
        if claude_skills.exists() {
            dirs.push(claude_skills);
        }
    }

    dirs
}

/// Extract the owning plugin id from a plugin-bundled skill path.
///
/// Plugin skills live at `<…>/plugins/<plugin_id>/skills/<skill>/SKILL.md`, and
/// bundled ones nest a marketplace layer
/// (`<…>/plugins/cache/<market>/<plugin_id>/skills/…`). Rather than assume the
/// component right after `plugins`, take the component immediately *before* the
/// `skills` segment — robust to both layouts. Returns `None` for non-plugin
/// paths (no `plugins` ancestor before a `skills` segment).
fn plugin_id_from_path(path: &Path) -> Option<String> {
    let comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let mut saw_plugins = false;
    for (i, c) in comps.iter().enumerate() {
        if c == "plugins" || c == "plugins.local" {
            saw_plugins = true;
        } else if c == "skills" && saw_plugins && i >= 1 {
            return comps.get(i - 1).cloned().filter(|id| !id.is_empty());
        }
    }
    None
}

/// Guess the `SkillSource` from a file path.
///
/// - Under `<…>/plugins/<id>/skills/` (any root) → Plugin(id)
/// - Under `~/.aleph/skills/` with manifest marking official → Bundled
/// - Under `~/.aleph/skills/` otherwise → Global
/// - Contains `.aleph/skills` but not under home → Workspace
/// - Otherwise → Bundled (e.g. Claude Code compatibility paths)
fn guess_source(path: &Path) -> SkillSource {
    use std::sync::OnceLock;

    // Plugin-bundled skills take precedence: they live under a `plugins/<id>/
    // skills` tree (never matching the `.aleph/skills` check below) and must be
    // classed `Plugin` so they outrank Global/Bundled in the prompt index and
    // `skill_read` collision resolution. Before this branch every plugin skill
    // fell through to `Bundled`, mis-ranking it as lowest priority.
    if let Some(plugin_id) = plugin_id_from_path(path) {
        return SkillSource::Plugin(crate::domain::skill::PluginId::new(plugin_id));
    }

    // Cache the bundled manifest to avoid re-reading from disk on every call.
    static CACHED_MANIFEST: OnceLock<Option<crate::bundled::manifest::InstallRegistry>> =
        OnceLock::new();

    let path_str = path.to_string_lossy();

    if let Ok(global_skills) = crate::utils::paths::get_skills_dir() {
        let resolved_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let resolved_global =
            std::fs::canonicalize(&global_skills).unwrap_or_else(|_| global_skills.clone());
        if resolved_path.starts_with(&resolved_global) {
            let manifest = CACHED_MANIFEST
                .get_or_init(|| crate::bundled::manifest::InstallRegistry::load(&global_skills));
            if let Some(manifest) = manifest {
                if let Ok(relative) = resolved_path.strip_prefix(&resolved_global) {
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
    }

    if path_str.contains(".aleph/skills") {
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
    fn aleph_home_is_authoritative_for_default_skill_discovery() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let home = temp_dir.path().join("home");
        let aleph_home = temp_dir.path().join("aleph-home");
        let global_aleph = aleph_home.join("skills");
        let legacy_aleph = home.join(".aleph").join("skills");
        let global_claude = home.join(".claude").join("skills");
        std::fs::create_dir_all(&global_aleph).unwrap();
        std::fs::create_dir_all(&legacy_aleph).unwrap();
        std::fs::create_dir_all(&global_claude).unwrap();

        let dirs = {
            let _env =
                crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&aleph_home, &home);
            default_skill_dirs()
        };

        assert!(dirs.contains(&global_aleph));
        assert!(dirs.contains(&global_claude));
        assert!(!dirs.contains(&legacy_aleph));
    }

    #[test]
    fn api_key_present_reflects_env() {
        use crate::domain::skill::SkillContent;
        // Unique var name so concurrent tests never collide; removed at end.
        let env_name = "ALEPH_SKILL_APIKEY_TEST_XZ42";

        let mut with_key = SkillManifest::new(
            "test:needs-key",
            "Needs Key",
            "Requires an API key",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        with_key.set_primary_env(env_name.to_string());

        // No env var → not set (the previously hardcoded `false` bug case).
        std::env::remove_var(env_name);
        assert!(!SkillSystem::api_key_present(&with_key));

        // Env var present → reported as set.
        std::env::set_var(env_name, "secret");
        assert!(SkillSystem::api_key_present(&with_key));
        std::env::remove_var(env_name);

        // No primary_env declared → no key to set.
        let no_key = SkillManifest::new(
            "test:no-key",
            "No Key",
            "Needs nothing",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        assert!(!SkillSystem::api_key_present(&no_key));
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
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        let content = r#"---
name: Test Skill
description: A test skill for unit tests
---
You are a test expert."#;
        tokio::fs::write(&skill_file, content).await.unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await;

        let snapshot = system.current_snapshot().await;
        assert!(snapshot.version > 0);
        assert!(!snapshot.eligible.is_empty());

        let skills = system.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name(), "Test Skill");
    }

    #[tokio::test]
    async fn list_skills() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let dir = tempfile::TempDir::new().unwrap();

        // Create two skill subdirectories with SKILL.md files
        let sub1 = dir.path().join("skill1");
        tokio::fs::create_dir(&sub1).await.unwrap();
        tokio::fs::write(
            sub1.join("SKILL.md"),
            r#"---
name: Skill One
description: First skill
---
Content one."#,
        )
        .await
        .unwrap();

        let sub2 = dir.path().join("skill2");
        tokio::fs::create_dir(&sub2).await.unwrap();
        tokio::fs::write(
            sub2.join("SKILL.md"),
            r#"---
name: Skill Two
description: Second skill
---
Content two."#,
        )
        .await
        .unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await;

        let skills = system.list_skills().await;
        assert_eq!(skills.len(), 2);

        let names: Vec<&str> = skills.iter().map(|s| s.name()).collect();
        assert!(names.contains(&"Skill One"));
        assert!(names.contains(&"Skill Two"));
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
    fn guess_source_plugin_dir() {
        use crate::domain::skill::PluginId;
        // `<home>/.aleph/plugins/<plugin>/skills/<id>/SKILL.md` → Plugin(<plugin>).
        let path = PathBuf::from("/home/u/.aleph/plugins/my-plugin/skills/foo/SKILL.md");
        assert_eq!(
            guess_source(&path),
            SkillSource::Plugin(PluginId::new("my-plugin"))
        );
        // The base dir (what rescan passes) resolves identically.
        let base = PathBuf::from("/home/u/.aleph/plugins/my-plugin/skills");
        assert_eq!(
            guess_source(&base),
            SkillSource::Plugin(PluginId::new("my-plugin"))
        );
    }

    #[test]
    fn guess_source_plugin_bundled_cache_nesting() {
        use crate::domain::skill::PluginId;
        // Bundled/marketplace nesting `plugins/cache/<market>/<plugin>/skills`
        // must still pick the plugin (component before `skills`), not `cache`.
        let path = PathBuf::from(
            "/home/u/.aleph/plugins/cache/aleph-official/writer/skills/draft/SKILL.md",
        );
        assert_eq!(
            guess_source(&path),
            SkillSource::Plugin(PluginId::new("writer"))
        );
    }

    #[test]
    fn plugin_id_from_path_non_plugin_is_none() {
        // A normal skills path has no `plugins` ancestor → not a plugin skill.
        assert_eq!(
            plugin_id_from_path(Path::new("/home/u/.aleph/skills/foo/SKILL.md")),
            None
        );
        assert_eq!(plugin_id_from_path(Path::new("/tmp/whatever")), None);
    }

    #[test]
    fn is_skill_file_detection() {
        assert!(is_skill_file(Path::new("/some/dir/SKILL.md")));
        assert!(is_skill_file(Path::new("/some/dir/skill.md")));
        assert!(!is_skill_file(Path::new("/some/dir/README.md")));
        assert!(!is_skill_file(Path::new("/some/dir/")));
    }

    #[test]
    #[cfg(unix)]
    fn guess_source_canonicalizes_symlinked_global_skills() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real_aleph = tmp.path().join(".aleph");
        let real_skills = real_aleph.join("skills");
        std::fs::create_dir_all(&real_skills).unwrap();
        std::fs::write(
            real_skills.join("SKILL.md"),
            b"---\nname: Demo\ndescription: d\n---\nbody",
        )
        .unwrap();

        let alt_root = tmp.path().join("alt");
        std::fs::create_dir_all(&alt_root).unwrap();
        let link = alt_root.join("link_to_skills");
        std::os::unix::fs::symlink(&real_skills, &link).unwrap();
        let via_link = link.join("SKILL.md");

        let _env =
            crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&real_aleph, tmp.path());

        let via_link_result = guess_source(&via_link);
        let direct_result = guess_source(&real_skills.join("SKILL.md"));
        assert_eq!(via_link_result, direct_result);
        assert!(
            matches!(via_link_result, SkillSource::Global | SkillSource::Bundled),
            "symlinked skills dir must classify identically to literal, got {via_link_result:?}"
        );
    }

    #[tokio::test]
    async fn full_status_returns_entries() {
        use crate::domain::skill::SkillContent;
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let system = SkillSystem::new();
        let manifest = SkillManifest::new(
            "test:skill",
            "Test Skill",
            "A test",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        system.insert_manifests_for_test(vec![manifest]).await;
        let entries = system.full_status().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Test Skill");
        assert!(entries[0].eligible);
    }

    #[tokio::test]
    async fn remove_skill_from_registry() {
        use crate::domain::skill::SkillContent;
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let system = SkillSystem::new();
        let manifest = SkillManifest::new(
            "test:removable",
            "Removable",
            "desc",
            SkillContent::new("c"),
            SkillSource::Global,
        );
        system.insert_manifests_for_test(vec![manifest]).await;
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
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let system = SkillSystem::new();
        let manifest = SkillManifest::new(
            "test:bundled",
            "Bundled Skill",
            "desc",
            SkillContent::new("c"),
            SkillSource::Bundled,
        );
        system.insert_manifests_for_test(vec![manifest]).await;

        let result = system.remove_skill(&SkillId::new("test:bundled")).await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        // Skill should still be there
        assert_eq!(system.list_skills().await.len(), 1);
    }
}
