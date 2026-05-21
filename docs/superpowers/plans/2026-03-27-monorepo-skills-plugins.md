# Monorepo Skills & Plugins Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate official skills and plugins from separate GitHub repos into the main Aleph monorepo, embedded in the binary at compile time.

**Architecture:** `include_dir!` embeds `skills/` and `plugins/` directories at compile time. On startup, a new `bundled` module extracts them to `~/.aleph/` if the version has changed. A `manifest.json` tracks skill sources (official/github/local) with startup reconcile.

**Tech Stack:** Rust, `include_dir` crate, serde_json for manifest

**Spec:** `docs/superpowers/specs/2026-03-27-monorepo-skills-plugins-design.md`

---

### Task 1: Migrate Skills & Plugins Content Into Monorepo

**Files:**
- Create: `skills/` (directory — copy from `~/.aleph/skills-official/`)
- Create: `plugins/` (directory — copy from `~/.aleph/plugins/cache/aleph-official/`)

- [ ] **Step 1: Copy official skills into monorepo**

```bash
cp -r ~/.aleph/skills-official/ skills/
# Remove .git directory (we don't want nested git)
rm -rf skills/.git
# Remove any README/CLAUDE.md from the skills repo root (not needed in monorepo)
rm -f skills/README.md skills/CLAUDE.md
```

Verify: `ls skills/` should show ~31 skill directories (api-design, architecture, ci-cd, code-review, etc.)

- [ ] **Step 2: Copy official plugins into monorepo**

```bash
cp -r ~/.aleph/plugins/cache/aleph-official/ plugins/
# Remove .git directory
rm -rf plugins/.git
# Remove any README/CLAUDE.md from the plugins repo root
rm -f plugins/README.md plugins/CLAUDE.md
```

Verify: `ls plugins/` should show `marketplace.toml` and `plugins/` subdirectory.

- [ ] **Step 3: Flatten plugins directory if nested**

The marketplace cache has structure `plugins/plugins/<name>/`. If `marketplace.toml` has `plugin-root = "./plugins"`, the actual plugin dirs are in `plugins/plugins/`. Check and flatten if needed so that `marketplace.toml` and the plugin directories are at the same level:

```
plugins/
├── marketplace.toml
├── diagnostics/
├── llm-task/
└── ...
```

Update `marketplace.toml` source paths from `./plugins/<name>` to `./<name>` if flattened.

- [ ] **Step 4: Commit**

```bash
git add skills/ plugins/
git commit -m "chore: migrate official skills and plugins into monorepo"
```

---

### Task 2: Add `include_dir` Dependency and Create `bundled` Module

**Files:**
- Modify: `Cargo.toml` (add `include_dir` dependency)
- Modify: `src/lib.rs:81` (add `pub mod bundled;`)
- Create: `src/bundled/mod.rs`

- [ ] **Step 1: Add `include_dir` to `Cargo.toml` dependencies**

In the `[dependencies]` section of `Cargo.toml`, add:

```toml
include_dir = "0.7"
```

- [ ] **Step 2: Create `src/bundled/mod.rs`**

```rust
//! Bundled official skills and plugins, embedded at compile time.
//!
//! On startup, these are extracted to `~/.aleph/` if the bundled version
//! is newer than what's already installed.

mod extractor;
pub mod manifest;

use include_dir::{include_dir, Dir};

/// Official skills directory tree, embedded at compile time.
pub static BUNDLED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/../skills");

/// Official plugins (marketplace), embedded at compile time.
pub static BUNDLED_PLUGINS: Dir = include_dir!("$CARGO_MANIFEST_DIR/../plugins");

/// Version of the bundled content, tied to the server release.
pub const BUNDLED_VERSION: &str = env!("ALEPH_VERSION");

pub use extractor::extract_bundled_content;
```

- [ ] **Step 3: Add module declaration in `src/lib.rs`**

After line 81 (`pub mod skill;`), add:

```rust
pub mod bundled;
```

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p alephcore
```

Expected: Compilation succeeds (extractor and manifest modules will be created next, create empty placeholder files first if needed).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/bundled/
git commit -m "bundled: add include_dir dependency and bundled module scaffold"
```

---

### Task 3: Implement Manifest Module

**Files:**
- Create: `src/bundled/manifest.rs`

- [ ] **Step 1: Write manifest data structures and I/O**

```rust
//! Skills manifest — tracks installed skills, their sources, and bundled version.
//!
//! Location: `~/.aleph/skills/manifest.json`

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Version of the last successfully extracted bundled content.
    pub bundled_version: String,
    /// Per-skill metadata keyed by skill directory name.
    pub skills: BTreeMap<String, SkillEntry>,
}

/// Where a skill was installed from.
///
/// Named `SkillOrigin` to avoid collision with `domain::skill::SkillSource`
/// and `skills::registry::SkillSource`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillOrigin {
    /// Bundled with the binary, extracted on startup.
    Official,
    /// Installed from a GitHub URL.
    Github,
    /// Manually placed in the skills directory.
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Where this skill came from.
    pub source: SkillOrigin,
    /// Version when installed (for official skills, matches bundled_version).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Source URL (for github-installed skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// ISO date when installed (for non-official skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
}

impl SkillManifest {
    /// Load manifest from disk. Returns None if file doesn't exist or is corrupt.
    pub fn load(skills_dir: &Path) -> Option<Self> {
        let path = skills_dir.join("manifest.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(manifest) => Some(manifest),
                Err(e) => {
                    warn!(error = %e, "Corrupt manifest.json, will recreate");
                    None
                }
            },
            Err(_) => None,
        }
    }

    /// Save manifest to disk.
    pub fn save(&self, skills_dir: &Path) -> std::io::Result<()> {
        let path = skills_dir.join("manifest.json");
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, content)?;
        debug!(path = %path.display(), "Saved skills manifest");
        Ok(())
    }

    /// Create an empty manifest with the given version.
    pub fn new(version: &str) -> Self {
        Self {
            bundled_version: version.to_string(),
            skills: BTreeMap::new(),
        }
    }

    /// Reconcile manifest with actual directory contents.
    /// - Directories not in manifest → add as Local
    /// - Manifest entries without directories → remove
    pub fn reconcile(&mut self, skills_dir: &Path) {
        // Find directories on disk
        let on_disk: Vec<String> = match std::fs::read_dir(skills_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect(),
            Err(_) => return,
        };

        // Add missing directories as Local
        for name in &on_disk {
            if !self.skills.contains_key(name) {
                debug!(skill = %name, "Discovered untracked skill, marking as local");
                self.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Local,
                        version: None,
                        url: None,
                        installed_at: None,
                    },
                );
            }
        }

        // Remove manifest entries for deleted directories
        self.skills.retain(|name, _| on_disk.contains(name));
    }

    /// Check if a skill is official.
    pub fn is_official(&self, name: &str) -> bool {
        self.skills
            .get(name)
            .map(|e| e.source == SkillOrigin::Official)
            .unwrap_or(false)
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p alephcore
```

- [ ] **Step 3: Commit**

```bash
git add src/bundled/manifest.rs
git commit -m "bundled: implement skills manifest with reconcile"
```

---

### Task 4: Implement Extractor Module

**Files:**
- Create: `src/bundled/extractor.rs`

- [ ] **Step 1: Write the extractor logic**

```rust
//! Bundled content extractor — extracts embedded skills/plugins on startup.
//!
//! Extraction occurs when:
//! - manifest.json doesn't exist (first install or upgrade from old version)
//! - bundled_version differs from manifest's bundled_version

use super::manifest::{SkillEntry, SkillManifest, SkillOrigin};
use super::{BUNDLED_PLUGINS, BUNDLED_SKILLS, BUNDLED_VERSION};
use include_dir::Dir;
use std::path::Path;
use tracing::{debug, info, warn};

/// Main entry point — called during server startup.
///
/// Extracts bundled skills to `~/.aleph/skills/` and plugins to
/// `~/.aleph/plugins/cache/aleph-official/` if version has changed.
pub fn extract_bundled_content(aleph_home: &Path) {
    let skills_dir = aleph_home.join("skills");
    let plugins_cache = aleph_home.join("plugins").join("cache").join("aleph-official");

    // Ensure directories exist
    let _ = std::fs::create_dir_all(&skills_dir);
    let _ = std::fs::create_dir_all(&plugins_cache);

    // Load or create manifest
    let mut manifest = match SkillManifest::load(&skills_dir) {
        Some(m) => m,
        None => {
            // First run or upgrade from old version — reconcile existing skills first
            info!("No skills manifest found, performing initial reconcile");
            let mut m = SkillManifest::new("");
            m.reconcile(&skills_dir);
            m
        }
    };

    // Check if extraction is needed
    if manifest.bundled_version == BUNDLED_VERSION {
        debug!(version = BUNDLED_VERSION, "Bundled content is up to date");
        // Still reconcile to catch manually added skills
        manifest.reconcile(&skills_dir);
        if let Err(e) = manifest.save(&skills_dir) {
            warn!(error = %e, "Failed to save manifest after reconcile");
        }
        return;
    }

    info!(
        from = %manifest.bundled_version,
        to = BUNDLED_VERSION,
        "Extracting bundled content"
    );

    // Extract skills
    let skills_ok = extract_skills(&BUNDLED_SKILLS, &skills_dir, &mut manifest);

    // Extract plugins (marketplace cache)
    let plugins_ok = extract_plugins(&BUNDLED_PLUGINS, &plugins_cache);

    // Only update bundled_version if ALL extractions succeeded
    if skills_ok && plugins_ok {
        manifest.bundled_version = BUNDLED_VERSION.to_string();
        info!(version = BUNDLED_VERSION, "Bundled content extraction complete");
    } else {
        warn!("Partial extraction failure — will retry on next startup");
    }

    // Reconcile and save
    manifest.reconcile(&skills_dir);
    if let Err(e) = manifest.save(&skills_dir) {
        warn!(error = %e, "Failed to save manifest");
    }

    // Clean up legacy skills-official directory
    cleanup_legacy_dir(aleph_home);
}

/// Extract bundled skills to the skills directory.
/// Returns true if all extractions succeeded.
fn extract_skills(bundled: &Dir, skills_dir: &Path, manifest: &mut SkillManifest) -> bool {
    let mut all_ok = true;

    for dir in bundled.dirs() {
        let name = dir.path().to_string_lossy().to_string();

        // Skip if user has a non-official skill with the same name
        if let Some(entry) = manifest.skills.get(&name) {
            if entry.source != SkillOrigin::Official {
                debug!(skill = %name, source = ?entry.source, "Skipping user skill");
                continue;
            }
        }

        // Extract this skill
        let target = skills_dir.join(&name);
        match extract_dir_recursive(dir, &target) {
            Ok(()) => {
                manifest.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Official,
                        version: Some(BUNDLED_VERSION.to_string()),
                        url: None,
                        installed_at: None,
                    },
                );
                debug!(skill = %name, "Extracted bundled skill");
            }
            Err(e) => {
                warn!(skill = %name, error = %e, "Failed to extract skill");
                all_ok = false;
            }
        }
    }

    all_ok
}

/// Extract bundled plugins to the marketplace cache directory.
/// Overwrites the entire cache directory.
fn extract_plugins(bundled: &Dir, cache_dir: &Path) -> bool {
    // Remove existing cache and recreate
    if cache_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(cache_dir) {
            warn!(error = %e, "Failed to remove old plugin cache");
            return false;
        }
    }
    if let Err(e) = std::fs::create_dir_all(cache_dir) {
        warn!(error = %e, "Failed to create plugin cache directory");
        return false;
    }

    // Extract all files and directories
    match extract_dir_contents(bundled, cache_dir) {
        Ok(()) => {
            info!("Extracted bundled plugins to marketplace cache");
            true
        }
        Err(e) => {
            warn!(error = %e, "Failed to extract bundled plugins");
            false
        }
    }
}

/// Recursively extract an include_dir Dir to a filesystem path.
fn extract_dir_recursive(dir: &Dir, target: &Path) -> std::io::Result<()> {
    // Remove existing and recreate (for clean update)
    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
    std::fs::create_dir_all(target)?;

    extract_dir_contents(dir, target)
}

/// Extract contents of a Dir (files + subdirs) into target path.
fn extract_dir_contents(dir: &Dir, target: &Path) -> std::io::Result<()> {
    for file in dir.files() {
        let file_path = target.join(file.path().file_name().unwrap_or_default());
        std::fs::write(&file_path, file.contents())?;
    }

    for subdir in dir.dirs() {
        let subdir_name = subdir.path().file_name().unwrap_or_default();
        let subdir_target = target.join(subdir_name);
        std::fs::create_dir_all(&subdir_target)?;
        extract_dir_contents(subdir, &subdir_target)?;
    }

    Ok(())
}

/// Remove legacy `~/.aleph/skills-official/` directory if it exists.
fn cleanup_legacy_dir(aleph_home: &Path) {
    let legacy = aleph_home.join("skills-official");
    if legacy.exists() {
        info!("Removing legacy skills-official directory");
        if let Err(e) = std::fs::remove_dir_all(&legacy) {
            warn!(error = %e, "Failed to remove legacy skills-official directory");
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p alephcore
```

- [ ] **Step 3: Commit**

```bash
git add src/bundled/extractor.rs
git commit -m "bundled: implement startup extractor with version check and reconcile"
```

---

### Task 5: Wire Extractor Into Server Startup, Remove Updater

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs:178-181`
- Modify: `src/skills/mod.rs:37` (remove `pub mod updater;`)
- Delete: `src/skills/updater.rs`

- [ ] **Step 1: Replace updater calls in startup**

In `src/bin/aleph-server/commands/start/mod.rs`, find the `initialize_extension_manager` function (around line 174). Replace the updater calls:

```rust
// BEFORE (lines 180-181):
alephcore::skills::updater::migrate_skills_directory(&aleph_home).await;
alephcore::skills::updater::update_official_skills(&aleph_home.join("skills-official")).await;

// AFTER:
alephcore::bundled::extract_bundled_content(&aleph_home);
```

Note: `extract_bundled_content` is synchronous (filesystem I/O only, no network). The function signature of `initialize_extension_manager` stays async.

- [ ] **Step 2: Remove `pub mod updater;` from `src/skills/mod.rs`**

Delete line 37: `pub mod updater;`

- [ ] **Step 3: Delete `src/skills/updater.rs`**

```bash
rm src/skills/updater.rs
```

- [ ] **Step 4: Update installer.rs doc comment**

In `src/skills/installer.rs`, lines 7-8 reference the updater:

```rust
// BEFORE:
//! Note: Aleph official skills (rootazero/Aleph-skills) are managed by
//! `updater.rs`, not this installer. They auto-sync on startup via git.

// AFTER:
//! Note: Aleph official skills are bundled in the binary and extracted
//! on startup by `bundled::extractor`. This installer handles third-party skills.
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p alephcore
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "bundled: wire extractor into startup, remove git-based updater"
```

---

### Task 6: Remove `skills-official` Path References

**Files:**
- Modify: `src/utils/paths.rs:278-282`
- Modify: `src/builtin_tools/self_manage.rs:61`
- Modify: `src/extension/mod.rs:254-268`

- [ ] **Step 1: Remove `skills-official` from `src/utils/paths.rs`**

Delete or comment out lines 278-282:

```rust
// REMOVE these lines:
let global_official = home.join(".aleph").join("skills-official");
if global_official.is_dir() && !dirs.contains(&global_official) {
    info!(path = %global_official.display(), "Found global ~/.aleph/skills-official");
    dirs.push(global_official);
}
```

The `~/.aleph/skills/` path (which is already discovered in this function) now contains both official and user skills.

- [ ] **Step 2: Remove `skills-official` from `src/builtin_tools/self_manage.rs`**

In the `Default` impl (around line 56-64), remove the `skills-official` path:

```rust
// BEFORE:
Self::new(vec![
    aleph_home.join("skills-official"),
    aleph_home.join("skills"),
])

// AFTER:
Self::new(vec![
    aleph_home.join("skills"),
])
```

- [ ] **Step 3: Remove `skills-official` from `src/extension/mod.rs`**

In the SkillSystem initialization (around lines 253-268), remove the `skills-official` directory push:

```rust
// REMOVE these lines:
let official_dir = dirs::home_dir()
    .unwrap_or_else(|| PathBuf::from("/tmp"))
    .join(".aleph")
    .join("skills-official");
if official_dir.exists() {
    skill_dirs.push(official_dir);
}
```

The user skills dirs discovered via `self.discovery.discover_skill_dirs()` will now include official skills (since they're in `~/.aleph/skills/`).

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p alephcore
```

- [ ] **Step 5: Commit**

```bash
git add src/utils/paths.rs src/builtin_tools/self_manage.rs src/extension/mod.rs
git commit -m "bundled: remove all skills-official path references"
```

---

### Task 7: Update Source Classification to Use Manifest

**Files:**
- Modify: `src/skill/mod.rs:302-326` (`guess_source()`)
- Modify: `src/skills/registry.rs:71-81` (`SkillEcosystem`)

- [ ] **Step 1: Update `guess_source()` in `src/skill/mod.rs`**

The function currently uses `path_str.contains("skills-official")` to detect bundled skills. Now official skills live in `~/.aleph/skills/` alongside user skills. We need to consult the manifest:

```rust
fn guess_source(path: &Path) -> SkillSource {
    let path_str = path.to_string_lossy();

    if path_str.contains(".aleph/skills") {
        if let Some(home) = dirs::home_dir() {
            let home_skills = home.join(".aleph").join("skills");
            if path.starts_with(&home_skills) {
                // Under ~/.aleph/skills/ — check manifest to distinguish official from user
                if let Some(manifest) = crate::bundled::manifest::SkillManifest::load(&home_skills) {
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
```

**Notes:**
- The logic is identical to the original except: instead of checking `"skills-official"` for Bundled, we now check the manifest.
- `SkillSource::Workspace` is returned for project-level `.aleph/skills` (not under home dir) — same as original.
- Default `SkillSource::Bundled` for non-.aleph paths (e.g., `.claude/skills`, plugin skills) — same as original.
- `SkillManifest::load()` reads a small JSON file once per call. Called once per skill during startup discovery. If needed later, cache the manifest in a `OnceLock`.

- [ ] **Step 2: Update `SkillEcosystem` classification in `src/skills/registry.rs`**

In `load_skills_from_dir()` (around line 343-350), replace the path-based check:

```rust
// BEFORE (lines 344-350):
let ecosystem = if dir_str.contains("/.claude/") {
    SkillEcosystem::Claude
} else if dir_str.contains("skills-official") {
    SkillEcosystem::Official
} else {
    SkillEcosystem::Aleph
};

// AFTER:
let ecosystem = if dir_str.contains("/.claude/") {
    SkillEcosystem::Claude
} else {
    // Check manifest to determine if this is an official skill
    let is_official = crate::bundled::manifest::SkillManifest::load(skills_dir)
        .map(|m| m.is_official(&skill_id))
        .unwrap_or(false);
    if is_official {
        SkillEcosystem::Official
    } else {
        SkillEcosystem::Aleph
    }
};
```

Here `skills_dir` is the parameter already available in the function scope — it points to the skills directory being scanned (e.g., `~/.aleph/skills/`).

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p alephcore
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 5: Commit**

```bash
git add src/skill/mod.rs src/skills/registry.rs
git commit -m "bundled: use manifest for source classification instead of path heuristics"
```

---

### Task 8: Update Marketplace Builtin Source

**Files:**
- Modify: `src/extension/marketplace/types.rs:86-87`
- Modify: `src/extension/marketplace/mod.rs:110-122,248-253,292-301`

- [ ] **Step 1: Change builtin marketplace source constant**

In `src/extension/marketplace/types.rs` (line 87):

```rust
// BEFORE:
pub const BUILTIN_MARKETPLACE_SOURCE: &str = "rootazero/Aleph-plugins";

// AFTER:
/// Builtin marketplace is extracted from bundled content, not cloned from GitHub.
pub const BUILTIN_MARKETPLACE_SOURCE: &str = "bundled";
```

- [ ] **Step 2: Change `builtin_config()` source type**

In `src/extension/marketplace/mod.rs` (lines 248-253), change the builtin config to use `Local` type instead of `Github` (since its content is managed by the bundled extractor, not git):

```rust
// BEFORE:
fn builtin_config() -> MarketplaceConfig {
    MarketplaceConfig {
        source: BUILTIN_MARKETPLACE_SOURCE.to_string(),
        source_type: MarketplaceSourceType::Github,
    }
}

// AFTER:
fn builtin_config() -> MarketplaceConfig {
    MarketplaceConfig {
        source: BUILTIN_MARKETPLACE_SOURCE.to_string(),
        source_type: MarketplaceSourceType::Local,
    }
}
```

This ensures `update()` (line 110-122) will call `resolve_local_marketplace` instead of `sync_github_marketplace` for the builtin, and `resolve_cache_dir` (line 237-238) will resolve the local cache path correctly.

- [ ] **Step 3: Update the `resolve_cache_dir` for Local type**

The current `resolve_cache_dir` calls `resolve_local_marketplace(&config.source)` for Local types, which validates the path exists. For the builtin marketplace, the cache dir is at `~/.aleph/plugins/cache/aleph-official/`. Ensure the `update()` method for Local type returns the correct cache path. Since `resolve_local_marketplace` expects an actual directory path (not `"bundled"`), the `update()` for the builtin should return its known cache path:

In `update()` (line 110-122), add special handling:

```rust
pub fn update(&self, name: &str) -> Result<PathBuf, String> {
    let all = self.all_marketplaces();
    let config = all
        .get(name)
        .ok_or_else(|| format!("Unknown marketplace '{name}'"))?;

    // Builtin marketplace is managed by bundled extractor — just return cache path
    if name == BUILTIN_MARKETPLACE_NAME {
        let cache = self.cache_dir.join(name);
        return if cache.exists() {
            Ok(cache)
        } else {
            Err("Builtin marketplace cache not yet extracted".to_string())
        };
    }

    match config.source_type {
        MarketplaceSourceType::Github => {
            sync_github_marketplace(&config.source, &self.cache_dir, name)
        }
        MarketplaceSourceType::Local => resolve_local_marketplace(&config.source),
    }
}
```

- [ ] **Step 4: Update marketplace test**

In `src/extension/marketplace/mod.rs` (line 300), the test checks the builtin source value:

```rust
// BEFORE:
assert_eq!(all[BUILTIN_MARKETPLACE_NAME].source, BUILTIN_MARKETPLACE_SOURCE);

// This still passes since BUILTIN_MARKETPLACE_SOURCE is now "bundled"
// and the config uses it. No change needed — the test auto-adapts.
```

Verify the test passes. If the test also checks `source_type`, update accordingly.

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p alephcore
```

- [ ] **Step 6: Run marketplace tests**

```bash
cargo test -p alephcore --lib marketplace
```

- [ ] **Step 7: Commit**

```bash
git add src/extension/marketplace/
git commit -m "marketplace: builtin now bundled, skip git sync"
```

---

### Task 9: Integrate Manifest With Skill Installer and Deletion

**Files:**
- Modify: `src/skills/installer.rs`
- Modify: `src/skills/mod.rs` (the `delete_skill` and `install_skill_from_url` FFI functions)

- [ ] **Step 1: Update installer to write manifest on install**

In `src/skills/installer.rs`, after a successful install (in `install_from_github` and `install_from_zip`), write a manifest entry. Add a helper method:

```rust
use crate::bundled::manifest::{SkillEntry, SkillManifest, SkillOrigin};

impl SkillsInstaller {
    /// Update manifest after installing a skill.
    fn update_manifest_install(&self, skill_names: &[String], source_url: Option<&str>) {
        let mut manifest = SkillManifest::load(&self.skills_dir)
            .unwrap_or_else(|| SkillManifest::new(""));

        let now = chrono::Utc::now().format("%Y-%m-%d").to_string();
        for name in skill_names {
            manifest.skills.insert(
                name.clone(),
                SkillEntry {
                    source: if source_url.is_some() {
                        SkillOrigin::Github
                    } else {
                        SkillOrigin::Local
                    },
                    version: None,
                    url: source_url.map(|u| u.to_string()),
                    installed_at: Some(now.clone()),
                },
            );
        }

        if let Err(e) = manifest.save(&self.skills_dir) {
            warn!(error = %e, "Failed to update manifest after install");
        }
    }
}
```

Call `self.update_manifest_install(&installed_ids, Some(url))` at the end of `install_from_github` and `self.update_manifest_install(&installed_ids, None)` at the end of `install_from_zip`.

- [ ] **Step 2: Update deletion to sync manifest**

In the `delete_skill` method of `SkillsInstaller` (around line 67-82), add manifest removal after deleting the directory:

```rust
pub fn delete_skill(&self, id: &str) -> Result<()> {
    let skill_dir = self.skills_dir.join(id);
    // ... existing deletion logic ...

    // Remove from manifest
    if let Some(mut manifest) = SkillManifest::load(&self.skills_dir) {
        manifest.skills.remove(id);
        if let Err(e) = manifest.save(&self.skills_dir) {
            warn!(error = %e, "Failed to update manifest after deletion");
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p alephcore
```

- [ ] **Step 4: Commit**

```bash
git add src/skills/installer.rs src/skills/mod.rs
git commit -m "skills: integrate manifest with installer and deletion"
```

---

### Task 10: Final Verification and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Full compilation check**

```bash
cargo check -p alephcore
```

- [ ] **Step 2: Run all core tests**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 3: Search for any remaining `skills-official` references**

```bash
grep -r "skills-official" src/ --include="*.rs"
grep -r "skills.official" src/ --include="*.rs"
grep -r "Aleph-skills" src/ --include="*.rs"
grep -r "rootazero/Aleph-skills" core/ --include="*.rs" --include="*.toml"
```

Expected: No results. If any found, update them.

- [ ] **Step 4: Search for remaining references to the old plugin repo**

```bash
grep -r "rootazero/Aleph-plugins" src/ --include="*.rs"
```

Expected: No results (should have been replaced by `"bundled"` in types.rs).

- [ ] **Step 5: Clippy check**

```bash
cargo clippy -p alephcore -- -W warnings
```

Fix any warnings.

- [ ] **Step 6: Build release binary and test extraction**

```bash
cargo build --bin aleph-server --release
# The binary now contains embedded skills/plugins
# Test by checking binary size increase
ls -la target/release/aleph-server
```

- [ ] **Step 7: Final commit (if any cleanup needed)**

```bash
git add -A
git commit -m "bundled: final cleanup and verification"
```
