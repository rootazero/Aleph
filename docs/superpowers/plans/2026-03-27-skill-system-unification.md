# Skill System Unification & Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify Aleph's four parallel skill systems into one domain-driven architecture, add install execution + API key management + status reporting, redesign Panel UI, and expose skill operations as LLM Tools.

**Architecture:** All skill data flows converge into `skill/` module via `SkillManifest`. `SkillSystem` facade gains config persistence (TOML), Vault integration (API keys), install execution (child process), and a `full_status()` method that powers both the new RPC endpoints and Panel UI. Three new LLM Tools complete the R9 (Everything is a Tool) loop.

**Tech Stack:** Rust (core), Leptos/WASM (Panel UI), TOML (config), tokio::process (install execution), serde_json (RPC)

**Spec:** `docs/superpowers/specs/2026-03-27-skill-system-unification-design.md`

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `src/skill/config.rs` | SkillsConfig persistence (TOML), Vault API key helpers |
| `src/builtin_tools/skill_status.rs` | LLM Tool: query skill status |
| `src/builtin_tools/skill_install.rs` | LLM Tool: install skill dependencies |
| `src/builtin_tools/skill_manage.rs` | LLM Tool: toggle/configure skills |
| `interfaces/webchat/src/views/settings/skills/mod.rs` | SkillsView main + tab bar |
| `interfaces/webchat/src/views/settings/skills/skill_list.rs` | List + source grouping |
| `interfaces/webchat/src/views/settings/skills/skill_card.rs` | Single skill row |
| `interfaces/webchat/src/views/settings/skills/skill_detail.rs` | Detail dialog |
| `interfaces/webchat/src/views/settings/skills/skill_install.rs` | Install interaction |
| `interfaces/webchat/src/views/settings/skills/add_skill.rs` | Add Skill dialog |

### Modified Files

| File | Changes |
|------|---------|
| `src/domain/skill.rs:362-515` | Add `primary_env`, `homepage`, `emoji` fields + accessors + mutators |
| `src/skill/manifest.rs:62-224` | Parse new frontmatter fields (`primary-env`, `homepage`, `emoji`) |
| `src/skill/status.rs:1-149` | Replace `SkillStatusReport` with `SkillStatusEntry` + `MissingRequirements` + `InstallOption` + `SkillStatusFilter` |
| `src/skill/installer.rs:1-53` | Add `InstallExecutor`, `select_best_install`, `InstallResult`, `InstallPreferences`, `NodeManager` |
| `src/skill/mod.rs:7-23,85-249` | Add `config` module, extend `Inner` with config/vault, add `register_external`/`full_status`/`update_config`/`install_dependency` |
| `src/gateway/handlers/skills.rs:1-191` | Rewrite: `skills.status`/`skills.update`/`skills.install_dep`/`skills.add`/`skills.remove` |
| `src/gateway/handlers/mod.rs:301-312` | Replace old handler registrations with new ones |
| `interfaces/webchat/src/views/settings/mod.rs` | Change `skills` from file module to directory module |

### Files to Delete (Phase 6)

| File | Reason |
|------|--------|
| `src/gateway/handlers/markdown_skills.rs` | Unified into `skills.*` |
| `interfaces/webchat/src/views/settings/skills.rs` | Replaced by `skills/` directory |

---

## Phase 1: Core Data Model Foundation

### Task 1: Extend SkillManifest with new fields

**Files:**
- Modify: `src/domain/skill.rs:362-515`
- Test: `src/domain/skill.rs:527-725` (existing test module)

- [ ] **Step 1: Write test for new fields**

In `src/domain/skill.rs` test module, add:

```rust
#[test]
fn manifest_new_metadata_fields() {
    let mut manifest = SkillManifest::new(
        "test:skill",
        "Test Skill",
        "A test skill",
        SkillContent::new("content"),
        SkillSource::Bundled,
    );

    // Defaults are None
    assert!(manifest.primary_env().is_none());
    assert!(manifest.homepage().is_none());
    assert!(manifest.emoji().is_none());

    // Set values
    manifest.set_primary_env("OPENAI_API_KEY".to_string());
    manifest.set_homepage("https://openai.com".to_string());
    manifest.set_emoji("🔍".to_string());

    assert_eq!(manifest.primary_env(), Some("OPENAI_API_KEY"));
    assert_eq!(manifest.homepage(), Some("https://openai.com"));
    assert_eq!(manifest.emoji(), Some("🔍"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib manifest_new_metadata_fields`
Expected: FAIL — fields/methods don't exist yet

- [ ] **Step 3: Add fields, accessors, and mutators to SkillManifest**

In `src/domain/skill.rs`, add three private fields after `source` (line 385):

```rust
    /// API Key environment variable name (e.g. "OPENAI_API_KEY").
    primary_env: Option<String>,
    /// External documentation / key acquisition URL.
    homepage: Option<String>,
    /// UI emoji icon.
    emoji: Option<String>,
```

In `SkillManifest::new()` (line 397-409), add defaults:

```rust
    primary_env: None,
    homepage: None,
    emoji: None,
```

Add accessors after `source()` (around line 462):

```rust
    /// API Key environment variable name.
    pub fn primary_env(&self) -> Option<&str> {
        self.primary_env.as_deref()
    }

    /// External documentation URL.
    pub fn homepage(&self) -> Option<&str> {
        self.homepage.as_deref()
    }

    /// UI emoji icon.
    pub fn emoji(&self) -> Option<&str> {
        self.emoji.as_deref()
    }
```

Add mutators after `set_invocation()` (around line 514):

```rust
    /// Set the API key environment variable name.
    pub fn set_primary_env(&mut self, env: String) {
        self.primary_env = Some(env);
    }

    /// Set the homepage URL.
    pub fn set_homepage(&mut self, url: String) {
        self.homepage = Some(url);
    }

    /// Set the emoji icon.
    pub fn set_emoji(&mut self, emoji: String) {
        self.emoji = Some(emoji);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib manifest_new_metadata_fields`
Expected: PASS

- [ ] **Step 5: Run full test suite to check for regressions**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests pass (new fields have `None` defaults — no breaking changes)

- [ ] **Step 6: Commit**

```bash
git add src/domain/skill.rs
git commit -m "skill: add primary_env, homepage, emoji fields to SkillManifest"
```

---

### Task 2: Parse new fields from SKILL.md frontmatter

**Files:**
- Modify: `src/skill/manifest.rs:62-224`
- Test: `src/skill/manifest.rs:265+` (existing test module)

- [ ] **Step 1: Write test for parsing new fields**

In `src/skill/manifest.rs` test module, add:

```rust
#[test]
fn parse_metadata_fields() {
    let content = r#"---
name: Web Search
description: Searches the web
primary-env: SERPAPI_KEY
homepage: https://serpapi.com
emoji: "🌐"
---
Search instructions."#;

    let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
    assert_eq!(manifest.primary_env(), Some("SERPAPI_KEY"));
    assert_eq!(manifest.homepage(), Some("https://serpapi.com"));
    assert_eq!(manifest.emoji(), Some("🌐"));
}

#[test]
fn parse_metadata_fields_absent() {
    let content = r#"---
name: Simple Skill
description: No metadata
---
Content."#;

    let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
    assert!(manifest.primary_env().is_none());
    assert!(manifest.homepage().is_none());
    assert!(manifest.emoji().is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib parse_metadata_fields`
Expected: FAIL — fields not parsed

- [ ] **Step 3: Add new fields to RawFrontmatter and parsing logic**

In `RawFrontmatter` (line 64-79), add:

```rust
    #[serde(default)]
    primary_env: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    emoji: Option<String>,
```

In `parse_skill_content()`, after install specs parsing (line 221), add:

```rust
    // Metadata fields
    if let Some(env) = raw.primary_env {
        manifest.set_primary_env(env);
    }
    if let Some(url) = raw.homepage {
        manifest.set_homepage(url);
    }
    if let Some(emoji) = raw.emoji {
        manifest.set_emoji(emoji);
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib parse_metadata_fields`
Expected: PASS (both tests)

- [ ] **Step 5: Commit**

```bash
git add src/skill/manifest.rs
git commit -m "skill: parse primary-env, homepage, emoji from SKILL.md frontmatter"
```

---

### Task 3: Create SkillsConfig with TOML persistence

**Files:**
- Create: `src/skill/config.rs`
- Modify: `src/skill/mod.rs:7-14` (add module declaration)

- [ ] **Step 1: Write tests**

Create `src/skill/config.rs` with test module:

```rust
//! Skill configuration persistence — stores user preferences per skill.
//!
//! Non-sensitive config (enabled/disabled, scope override, install preferences)
//! persists to `~/.aleph/data/skills.toml`. API keys route to the Vault.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::skill::{PromptScope, SkillId};

/// Node.js package manager preference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NodeManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Default for NodeManager {
    fn default() -> Self {
        Self::Npm
    }
}

/// Global install preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPreferences {
    #[serde(default)]
    pub prefer_brew: bool,
    #[serde(default)]
    pub node_manager: NodeManager,
}

impl Default for InstallPreferences {
    fn default() -> Self {
        Self {
            prefer_brew: cfg!(target_os = "macos"),
            node_manager: NodeManager::Npm,
        }
    }
}

/// Per-skill configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillEntryConfig {
    /// None = auto, Some(false) = user disabled, Some(true) = user enabled
    pub enabled: Option<bool>,
    /// Override the default prompt scope for this skill
    pub scope_override: Option<PromptScope>,
}

/// Root configuration structure, persisted as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    #[serde(default)]
    pub install_preferences: InstallPreferences,
    #[serde(default)]
    pub entries: HashMap<String, SkillEntryConfig>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            install_preferences: InstallPreferences::default(),
            entries: HashMap::new(),
        }
    }
}

/// Update request for a single skill's config.
pub enum SkillConfigUpdate {
    SetEnabled(bool),
    SetScope(PromptScope),
}

impl SkillsConfig {
    /// Load config from a TOML file, or return default if not found.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| toml::from_str(&content).ok())
            .unwrap_or_default()
    }

    /// Save config to a TOML file (atomic write).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        // Atomic write: write to temp file, then rename
        let tmp_path = path.with_extension("toml.tmp");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp_path, content)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Get per-skill config entry.
    pub fn get_entry(&self, id: &SkillId) -> Option<&SkillEntryConfig> {
        self.entries.get(id.as_str())
    }

    /// Apply an update to a specific skill's config.
    pub fn apply_update(&mut self, id: &SkillId, update: SkillConfigUpdate) {
        let entry = self
            .entries
            .entry(id.as_str().to_string())
            .or_default();
        match update {
            SkillConfigUpdate::SetEnabled(enabled) => {
                entry.enabled = Some(enabled);
            }
            SkillConfigUpdate::SetScope(scope) => {
                entry.scope_override = Some(scope);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn default_config() {
        let config = SkillsConfig::default();
        assert!(config.entries.is_empty());
        assert_eq!(config.install_preferences.node_manager, NodeManager::Npm);
    }

    #[test]
    fn roundtrip_toml() {
        let mut config = SkillsConfig::default();
        config.apply_update(
            &SkillId::new("test:skill"),
            SkillConfigUpdate::SetEnabled(false),
        );
        config.install_preferences.prefer_brew = true;

        let tmp = NamedTempFile::new().unwrap();
        config.save(tmp.path()).unwrap();

        let loaded = SkillsConfig::load(tmp.path());
        assert_eq!(loaded.install_preferences.prefer_brew, true);
        let entry = loaded.entries.get("test:skill").unwrap();
        assert_eq!(entry.enabled, Some(false));
    }

    #[test]
    fn load_missing_file_returns_default() {
        let config = SkillsConfig::load(Path::new("/nonexistent/path.toml"));
        assert!(config.entries.is_empty());
    }

    #[test]
    fn apply_scope_override() {
        let mut config = SkillsConfig::default();
        let id = SkillId::new("my:skill");
        config.apply_update(&id, SkillConfigUpdate::SetScope(PromptScope::Tool));

        let entry = config.entries.get("my:skill").unwrap();
        assert_eq!(entry.scope_override, Some(PromptScope::Tool));
    }
}
```

- [ ] **Step 2: Add module declaration**

In `src/skill/mod.rs`, add after line 14 (`pub mod status;`):

```rust
pub mod config;
```

And in the public exports section (after line 23):

```rust
pub use config::{InstallPreferences, NodeManager, SkillConfigUpdate, SkillEntryConfig, SkillsConfig};
```

- [ ] **Step 3: Ensure PromptScope derives needed traits**

Check that `PromptScope` in `src/domain/skill.rs` has `Serialize, Deserialize, PartialEq`. If not, add them to its derive. It likely needs at least `Serialize, Deserialize` for TOML roundtrip.

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib config`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/skill/config.rs src/skill/mod.rs src/domain/skill.rs
git commit -m "skill: add SkillsConfig with TOML persistence"
```

---

### Task 4: Replace SkillStatusReport with SkillStatusEntry

**Files:**
- Modify: `src/skill/status.rs:1-149` (full rewrite)
- Modify: `src/skill/mod.rs:23` (update re-export)

- [ ] **Step 1: Rewrite status.rs**

Replace entire `src/skill/status.rs` with:

```rust
//! Status reporting — provides a rich, serializable view of skill status
//! for the Panel UI, CLI, and LLM Tools.

use serde::{Serialize, Deserialize};

use crate::domain::skill::{InstallKind, PromptScope, SkillId, SkillManifest, SkillSource};
use crate::domain::Entity;
use crate::skill::config::{SkillEntryConfig, SkillsConfig};
use crate::skill::eligibility::{EligibilityResult, IneligibilityReason};
use crate::skill::installer::filter_install_specs_for_current_os;

/// A single missing-requirement entry for an install option.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallOption {
    pub id: String,
    pub kind: InstallKind,
    pub label: String,
    pub bins: Vec<String>,
}

/// What the skill is missing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissingRequirements {
    pub bins: Vec<String>,
    pub env: Vec<String>,
    pub config: Vec<String>,
}

/// Status filter for UI tabs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatusFilter {
    All,
    Ready,
    NeedsSetup,
    Disabled,
}

/// Full status entry for a single skill — powers UI, CLI, and LLM Tools.
#[derive(Debug, Clone, Serialize)]
pub struct SkillStatusEntry {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub emoji: Option<String>,
    pub source: SkillSource,
    pub homepage: Option<String>,

    pub eligible: bool,
    pub disabled: bool,
    pub missing: MissingRequirements,

    pub install_options: Vec<InstallOption>,

    pub primary_env: Option<String>,
    pub api_key_set: bool,

    pub scope: PromptScope,
    pub user_invocable: bool,
}

impl SkillStatusEntry {
    /// Build a status entry from a manifest + eligibility result + config + vault info.
    pub fn build(
        manifest: &SkillManifest,
        eligibility: &EligibilityResult,
        entry_config: Option<&SkillEntryConfig>,
        api_key_set: bool,
    ) -> Self {
        let disabled = entry_config
            .and_then(|c| c.enabled)
            .map(|e| !e)
            .unwrap_or(false);

        let scope = entry_config
            .and_then(|c| c.scope_override.clone())
            .unwrap_or_else(|| manifest.scope().clone());

        let mut missing = MissingRequirements::default();
        let eligible = match eligibility {
            EligibilityResult::Eligible => true,
            EligibilityResult::Ineligible(reasons) => {
                for reason in reasons {
                    match reason {
                        IneligibilityReason::MissingBinary(bin) => {
                            missing.bins.push(bin.clone());
                        }
                        IneligibilityReason::MissingAnyBinary(bins) => {
                            missing.bins.extend(bins.iter().cloned());
                        }
                        IneligibilityReason::MissingEnv(env) => {
                            missing.env.push(env.clone());
                        }
                        IneligibilityReason::MissingConfig(cfg) => {
                            missing.config.push(cfg.clone());
                        }
                        _ => {} // Disabled, OsNotSupported handled elsewhere
                    }
                }
                false
            }
        };

        // If primary_env is set but API key is missing, add to missing.env
        if let Some(env_name) = manifest.primary_env() {
            if !api_key_set && !missing.env.contains(&env_name.to_string()) {
                missing.env.push(env_name.to_string());
            }
        }

        // Build install options from platform-filtered specs
        let install_options = filter_install_specs_for_current_os(manifest.install_specs())
            .into_iter()
            .map(|spec| InstallOption {
                id: spec.id.clone(),
                kind: spec.kind.clone(),
                label: format!("Install {} ({})", spec.package, spec.kind.as_str()),
                bins: spec.bins.clone(),
            })
            .collect();

        Self {
            id: manifest.id().clone(),
            name: manifest.name().to_string(),
            description: manifest.description().to_string(),
            emoji: manifest.emoji().map(|s| s.to_string()),
            source: manifest.source().clone(),
            homepage: manifest.homepage().map(|s| s.to_string()),
            eligible,
            disabled,
            missing,
            install_options,
            primary_env: manifest.primary_env().map(|s| s.to_string()),
            api_key_set,
            scope,
            user_invocable: manifest.is_user_invocable(),
        }
    }

    /// Whether this entry matches a UI filter tab.
    pub fn matches_filter(&self, filter: SkillStatusFilter) -> bool {
        match filter {
            SkillStatusFilter::All => true,
            SkillStatusFilter::Ready => self.eligible && !self.disabled,
            SkillStatusFilter::NeedsSetup => !self.eligible && !self.disabled,
            SkillStatusFilter::Disabled => self.disabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{SkillContent, SkillManifest, SkillSource};

    fn make_manifest(name: &str) -> SkillManifest {
        SkillManifest::new(
            name,
            name,
            format!("{} description", name),
            SkillContent::new("content"),
            SkillSource::Bundled,
        )
    }

    #[test]
    fn build_eligible_entry() {
        let manifest = make_manifest("test:skill");
        let entry = SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Eligible,
            None,
            false,
        );

        assert!(entry.eligible);
        assert!(!entry.disabled);
        assert!(entry.missing.bins.is_empty());
    }

    #[test]
    fn build_ineligible_entry() {
        let manifest = make_manifest("test:skill");
        let reasons = vec![
            IneligibilityReason::MissingBinary("docker".to_string()),
        ];
        let entry = SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Ineligible(reasons),
            None,
            false,
        );

        assert!(!entry.eligible);
        assert_eq!(entry.missing.bins, vec!["docker"]);
    }

    #[test]
    fn disabled_by_config() {
        let manifest = make_manifest("test:skill");
        let config = SkillEntryConfig {
            enabled: Some(false),
            scope_override: None,
        };
        let entry = SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Eligible,
            Some(&config),
            false,
        );

        assert!(entry.disabled);
    }

    #[test]
    fn missing_api_key_added_to_env() {
        let mut manifest = make_manifest("test:skill");
        manifest.set_primary_env("OPENAI_API_KEY".to_string());

        let entry = SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Eligible,
            None,
            false, // api_key NOT set
        );

        assert!(entry.missing.env.contains(&"OPENAI_API_KEY".to_string()));
    }

    #[test]
    fn api_key_set_not_missing() {
        let mut manifest = make_manifest("test:skill");
        manifest.set_primary_env("OPENAI_API_KEY".to_string());

        let entry = SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Eligible,
            None,
            true, // api_key IS set
        );

        assert!(!entry.missing.env.contains(&"OPENAI_API_KEY".to_string()));
    }

    #[test]
    fn filter_matching() {
        let manifest = make_manifest("test:skill");
        let ready = SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Eligible,
            None,
            false,
        );
        let needs_setup = SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Ineligible(vec![
                IneligibilityReason::MissingBinary("x".into()),
            ]),
            None,
            false,
        );

        assert!(ready.matches_filter(SkillStatusFilter::Ready));
        assert!(!ready.matches_filter(SkillStatusFilter::NeedsSetup));
        assert!(needs_setup.matches_filter(SkillStatusFilter::NeedsSetup));
        assert!(!needs_setup.matches_filter(SkillStatusFilter::Ready));
    }
}
```

- [ ] **Step 2: Ensure InstallKind has needed traits and as_str method**

In `src/domain/skill.rs`, ensure `InstallKind` has `Clone, Serialize, Deserialize` derives and an `as_str()` method:

```rust
impl InstallKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Brew => "brew",
            Self::Apt => "apt",
            Self::Npm => "npm",
            Self::Uv => "uv",
            Self::Go => "go",
            Self::Download => "download",
        }
    }
}
```

- [ ] **Step 3: Update re-export in mod.rs**

In `src/skill/mod.rs` line 23, change:

```rust
pub use status::SkillStatusReport;
```

to:

```rust
pub use status::{InstallOption, MissingRequirements, SkillStatusEntry, SkillStatusFilter};
```

- [ ] **Step 4: Update callers of SkillStatusReport**

Search for `SkillStatusReport` usage in the codebase (primarily `SkillSystem::skill_status()` in `mod.rs:200-210`). Update return type. The `skill_status()` method will be replaced in Task 7 with `full_status()`, so for now just keep both.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib status`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/skill/status.rs src/skill/mod.rs src/domain/skill.rs
git commit -m "skill: replace SkillStatusReport with rich SkillStatusEntry"
```

---

### Task 5: Add InstallExecutor and preference selection

**Files:**
- Modify: `src/skill/installer.rs:1-53`

- [ ] **Step 1: Write tests for select_best_install and InstallExecutor**

Add to `src/skill/installer.rs` test module:

```rust
#[test]
fn select_best_install_prefers_brew_on_macos() {
    use crate::skill::config::InstallPreferences;

    let specs = vec![
        InstallSpec {
            id: "npm-pkg".into(),
            kind: InstallKind::Npm,
            package: "pkg".into(),
            bins: vec!["pkg".into()],
            os: None,
            url: None,
        },
        InstallSpec {
            id: "brew-pkg".into(),
            kind: InstallKind::Brew,
            package: "pkg".into(),
            bins: vec!["pkg".into()],
            os: Some(vec![Os::Darwin]),
            url: None,
        },
    ];

    let prefs = InstallPreferences {
        prefer_brew: true,
        ..Default::default()
    };

    let best = select_best_install(&specs, &prefs);
    assert!(best.is_some());
    assert_eq!(best.unwrap().id, "brew-pkg");
}

#[test]
fn select_best_install_no_brew_preference() {
    use crate::skill::config::InstallPreferences;

    let specs = vec![
        InstallSpec {
            id: "brew-pkg".into(),
            kind: InstallKind::Brew,
            package: "pkg".into(),
            bins: vec!["pkg".into()],
            os: None,
            url: None,
        },
        InstallSpec {
            id: "uv-pkg".into(),
            kind: InstallKind::Uv,
            package: "pkg".into(),
            bins: vec!["pkg".into()],
            os: None,
            url: None,
        },
    ];

    let prefs = InstallPreferences {
        prefer_brew: false,
        ..Default::default()
    };

    let best = select_best_install(&specs, &prefs);
    assert!(best.is_some());
    assert_eq!(best.unwrap().id, "uv-pkg");
}
```

- [ ] **Step 2: Implement select_best_install**

Add to `src/skill/installer.rs` before the test module:

```rust
use crate::skill::config::InstallPreferences;

/// Rank for install kind preference.
fn install_kind_rank(kind: &InstallKind, prefer_brew: bool) -> u8 {
    if prefer_brew {
        match kind {
            InstallKind::Brew => 0,
            InstallKind::Uv => 1,
            InstallKind::Npm => 2,
            InstallKind::Go => 3,
            InstallKind::Apt => 4,
            InstallKind::Download => 5,
        }
    } else {
        match kind {
            InstallKind::Uv => 0,
            InstallKind::Npm => 1,
            InstallKind::Brew => 2,
            InstallKind::Go => 3,
            InstallKind::Apt => 4,
            InstallKind::Download => 5,
        }
    }
}

/// Select the best install spec for the current platform and preferences.
pub fn select_best_install<'a>(
    specs: &'a [InstallSpec],
    prefs: &InstallPreferences,
) -> Option<&'a InstallSpec> {
    let mut candidates = filter_install_specs_for_current_os(specs);
    candidates.sort_by_key(|spec| install_kind_rank(&spec.kind, prefs.prefer_brew));
    candidates.into_iter().next()
}
```

- [ ] **Step 3: Add InstallResult struct**

Add to `src/skill/installer.rs`:

```rust
/// Result of a dependency installation execution.
#[derive(Debug, Clone, Serialize)]
pub struct InstallResult {
    pub success: bool,
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}
```

- [ ] **Step 4: Add InstallExecutor**

```rust
use std::time::Duration;
use tokio::process::Command;

/// Executes install commands with timeout and output capture.
pub struct InstallExecutor;

impl InstallExecutor {
    /// Run an install spec. Returns the result.
    pub async fn run(spec: &InstallSpec, prefs: &InstallPreferences) -> InstallResult {
        let cmd_str = match build_install_command(spec) {
            Some(cmd) => cmd,
            None => {
                return InstallResult {
                    success: false,
                    message: format!("Cannot build install command for {}", spec.package),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                };
            }
        };

        let result = tokio::time::timeout(
            Duration::from_secs(300),
            Command::new("sh")
                .arg("-c")
                .arg(&cmd_str)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let success = output.status.success();
                InstallResult {
                    success,
                    message: if success {
                        format!("Successfully installed {}", spec.package)
                    } else {
                        format!("Failed to install {}", spec.package)
                    },
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    exit_code: output.status.code(),
                }
            }
            Ok(Err(e)) => InstallResult {
                success: false,
                message: format!("Failed to execute install command: {}", e),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
            },
            Err(_) => InstallResult {
                success: false,
                message: "Installation timed out after 300 seconds".to_string(),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
            },
        }
    }
}
```

- [ ] **Step 5: Update mod.rs re-exports**

In `src/skill/mod.rs`, update the installer re-export:

```rust
pub use installer::{build_install_command, filter_install_specs_for_current_os, select_best_install, InstallExecutor, InstallResult};
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib installer`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/skill/installer.rs src/skill/mod.rs
git commit -m "skill: add InstallExecutor, select_best_install, InstallResult"
```

---

## Phase 2: SkillSystem Facade Extension

### Task 6: Add register_external and full_status to SkillSystem

**Files:**
- Modify: `src/skill/mod.rs:85-249`

- [ ] **Step 1: Write tests**

Add to `src/skill/mod.rs` test module:

```rust
#[tokio::test]
async fn register_external_skills() {
    let system = SkillSystem::new();
    let manifest = SkillManifest::new(
        "plugin:test",
        "Test Plugin Skill",
        "From a plugin",
        SkillContent::new("content"),
        SkillSource::Plugin,
    );

    system.register_external(vec![manifest]).await;

    let skills = system.list_skills().await;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name(), "Test Plugin Skill");
}

#[tokio::test]
async fn full_status_returns_entries() {
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
    assert!(entries[0].eligible); // No requirements = eligible
}
```

- [ ] **Step 2: Extend Inner struct**

In `src/skill/mod.rs`, add to `Inner` struct (line 89-95):

```rust
struct Inner {
    registry: RwLock<SkillRegistry>,
    snapshot: RwLock<SkillSnapshot>,
    skill_dirs: RwLock<Vec<PathBuf>>,
    version_counter: RwLock<u64>,
    eligibility: EligibilityService,
    config: RwLock<SkillsConfig>,        // NEW
    config_path: PathBuf,                // NEW
}
```

Update `SkillSystem::new()` to initialize with default config:

```rust
pub fn new() -> Self {
    let data_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".aleph")
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
```

- [ ] **Step 3: Add register_external method**

```rust
/// Register skills from external sources (plugins, markdown).
/// These are already converted to SkillManifest by their loaders.
pub async fn register_external(&self, manifests: Vec<SkillManifest>) {
    let mut registry = self.inner.registry.write().await;
    registry.register_all(manifests);
    drop(registry);
    self.rebuild_snapshot().await;
}
```

- [ ] **Step 4: Add full_status method**

```rust
/// Build full status entries for all skills.
/// Combines eligibility evaluation, user config, and vault state.
pub async fn full_status(&self) -> Vec<SkillStatusEntry> {
    let registry = self.inner.registry.read().await;
    let config = self.inner.config.read().await;

    registry
        .list_all()
        .into_iter()
        .map(|manifest| {
            let eligibility = self.inner.eligibility.evaluate(manifest);
            let entry_config = config.get_entry(manifest.id());
            // Note: api_key_set requires Vault access — for now default to false.
            // Vault integration wired in Task 7 (RPC handler calls Vault directly).
            let api_key_set = false;
            SkillStatusEntry::build(manifest, &eligibility, entry_config, api_key_set)
        })
        .collect()
}
```

- [ ] **Step 5: Add update_config method**

```rust
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
```

- [ ] **Step 6: Add install_dependency method**

```rust
/// Install a dependency for a skill.
pub async fn install_dependency(
    &self,
    id: &SkillId,
    spec_id: Option<&str>,
) -> InstallResult {
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
    let prefs = &config.install_preferences;

    let spec = if let Some(spec_id) = spec_id {
        manifest.install_specs().iter().find(|s| s.id == spec_id)
    } else {
        select_best_install(manifest.install_specs(), prefs)
    };

    let spec = match spec {
        Some(s) => s.clone(),
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

    drop(config);
    let result = InstallExecutor::run(&spec, &InstallPreferences::default()).await;

    // Rebuild snapshot to re-evaluate eligibility
    if result.success {
        self.rebuild_snapshot().await;
    }

    result
}
```

- [ ] **Step 7: Add remove_skill method**

```rust
/// Remove a skill from the registry. Only user-installed skills (Global/Workspace source)
/// can be removed. Returns true if found and removed, false if not found.
pub async fn remove_skill(&self, id: &SkillId) -> Result<bool, std::io::Error> {
    let mut registry = self.inner.registry.write().await;
    let manifest = registry.get(id).cloned();
    if let Some(m) = &manifest {
        // Only allow removing non-bundled skills
        if matches!(m.source(), SkillSource::Bundled) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Cannot remove bundled skills",
            ));
        }
        registry.remove(id);
        drop(registry);
        self.rebuild_snapshot().await;
        Ok(true)
    } else {
        Ok(false)
    }
}
```

Note: `SkillRegistry::remove()` doesn't exist yet — add a simple `pub fn remove(&mut self, id: &SkillId) -> bool { self.skills.remove(id).is_some() }` to `registry.rs`.

- [ ] **Step 8: Add add_from_url and add_from_base64_zip stubs**

```rust
/// Add a skill from a URL (git clone or download).
pub async fn add_from_url(&self, url: &str) -> Result<(), SkillSystemError> {
    // Download to ~/.aleph/skills/{name}/
    // Parse SKILL.md → SkillManifest
    // Register to registry
    // Rebuild snapshot
    todo!("Implement URL-based skill installation")
}

/// Add a skill from base64-encoded zip data.
pub async fn add_from_base64_zip(&self, data: &str) -> Result<(), SkillSystemError> {
    // Decode base64 → zip bytes
    // Extract to ~/.aleph/skills/{name}/
    // Parse SKILL.md → SkillManifest
    // Register to registry
    // Rebuild snapshot
    todo!("Implement zip-based skill installation")
}
```

Note: These can leverage existing logic from the legacy `skills::install_skill_from_url` and `skills::install_skills_from_zip` functions. The implementer should port that logic here rather than writing from scratch.

- [ ] **Step 9: Run tests**

Run: `cargo test -p alephcore --lib skill`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add src/skill/mod.rs src/skill/registry.rs
git commit -m "skill: extend SkillSystem with register_external, full_status, update_config, install_dependency, remove_skill"
```

---

## Phase 3: RPC Layer

### Task 7: Rewrite skills RPC handlers

**Files:**
- Modify: `src/gateway/handlers/skills.rs:1-191` (full rewrite)
- Modify: `src/gateway/handlers/mod.rs:301-312` (update registrations)

- [ ] **Step 1: Rewrite skills.rs**

Replace entire `src/gateway/handlers/skills.rs` with new handlers that call `SkillSystem`:

```rust
//! Skills RPC handlers — unified interface for skill management.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::gateway::handlers::{extract_context, JsonRpcResponse};
use crate::skill::{SkillStatusEntry, SkillStatusFilter};

/// skills.status — returns full status for all skills
pub async fn handle_status(
    _params: Value,
    ctx: &crate::gateway::GatewayContext,
) -> JsonRpcResponse {
    let entries = ctx.skill_system().full_status().await;
    JsonRpcResponse::success(json!({ "skills": entries }))
}

/// skills.update — update a skill's config (enabled, scope, api_key)
#[derive(Deserialize)]
struct UpdateParams {
    skill_id: String,
    enabled: Option<bool>,
    scope: Option<String>,
    api_key: Option<String>,
}

pub async fn handle_update(
    params: Value,
    ctx: &crate::gateway::GatewayContext,
) -> JsonRpcResponse {
    let p: UpdateParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(-32602, &format!("Invalid params: {}", e)),
    };

    let skill_id = crate::domain::skill::SkillId::new(&p.skill_id);

    // Handle enabled
    if let Some(enabled) = p.enabled {
        if let Err(e) = ctx
            .skill_system()
            .update_config(
                &skill_id,
                crate::skill::SkillConfigUpdate::SetEnabled(enabled),
            )
            .await
        {
            return JsonRpcResponse::error(-32000, &format!("Config update failed: {}", e));
        }
    }

    // Handle scope
    if let Some(scope_str) = &p.scope {
        let scope = match scope_str.as_str() {
            "system" => crate::domain::skill::PromptScope::System,
            "tool" => crate::domain::skill::PromptScope::Tool,
            "standalone" => crate::domain::skill::PromptScope::Standalone,
            "disabled" => crate::domain::skill::PromptScope::Disabled,
            _ => return JsonRpcResponse::error(-32602, "Invalid scope value"),
        };
        if let Err(e) = ctx
            .skill_system()
            .update_config(&skill_id, crate::skill::SkillConfigUpdate::SetScope(scope))
            .await
        {
            return JsonRpcResponse::error(-32000, &format!("Config update failed: {}", e));
        }
    }

    // Handle API key — route to Vault
    if let Some(api_key) = &p.api_key {
        let vault_key = format!("skill:{}", p.skill_id);
        if let Err(e) = ctx.shared_token_manager().store_secret(&vault_key, api_key) {
            return JsonRpcResponse::error(-32000, &format!("Vault store failed: {}", e));
        }
    }

    // Return updated status for this skill
    let entries = ctx.skill_system().full_status().await;
    match entries.into_iter().find(|e| e.id.as_str() == p.skill_id) {
        Some(entry) => JsonRpcResponse::success(json!({ "skill": entry })),
        None => JsonRpcResponse::error(-32000, "Skill not found after update"),
    }
}

/// skills.install_dep — install dependencies for a skill
#[derive(Deserialize)]
struct InstallDepParams {
    skill_id: String,
    spec_id: Option<String>,
}

pub async fn handle_install_dep(
    params: Value,
    ctx: &crate::gateway::GatewayContext,
) -> JsonRpcResponse {
    let p: InstallDepParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(-32602, &format!("Invalid params: {}", e)),
    };

    let skill_id = crate::domain::skill::SkillId::new(&p.skill_id);
    let result = ctx
        .skill_system()
        .install_dependency(&skill_id, p.spec_id.as_deref())
        .await;

    // Get updated status
    let entries = ctx.skill_system().full_status().await;
    let skill = entries.into_iter().find(|e| e.id.as_str() == p.skill_id);

    JsonRpcResponse::success(json!({
        "result": result,
        "skill": skill,
    }))
}

/// skills.add — add a new skill from URL or base64 zip
#[derive(Deserialize)]
struct AddParams {
    source: String, // URL or base64 zip data
}

pub async fn handle_add(
    params: Value,
    ctx: &crate::gateway::GatewayContext,
) -> JsonRpcResponse {
    let p: AddParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(-32602, &format!("Invalid params: {}", e)),
    };

    // Detect source type: URL (starts with http) vs base64 zip
    let result = if p.source.starts_with("http://") || p.source.starts_with("https://") {
        // Download and install from URL
        // Delegate to SkillSystem::add_from_url (to be added)
        ctx.skill_system().add_from_url(&p.source).await
    } else {
        // Assume base64 zip — decode, extract, install
        ctx.skill_system().add_from_base64_zip(&p.source).await
    };

    match result {
        Ok(_) => {
            let entries = ctx.skill_system().full_status().await;
            JsonRpcResponse::success(json!({ "skills": entries }))
        }
        Err(e) => JsonRpcResponse::error(-32000, &format!("Failed to add skill: {}", e)),
    }
}

/// skills.remove — delete a skill from registry and filesystem
#[derive(Deserialize)]
struct RemoveParams {
    skill_id: String,
}

pub async fn handle_remove(
    params: Value,
    ctx: &crate::gateway::GatewayContext,
) -> JsonRpcResponse {
    let p: RemoveParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(-32602, &format!("Invalid params: {}", e)),
    };

    let skill_id = crate::domain::skill::SkillId::new(&p.skill_id);
    match ctx.skill_system().remove_skill(&skill_id).await {
        Ok(removed) => JsonRpcResponse::success(json!({ "ok": removed })),
        Err(e) => JsonRpcResponse::error(-32000, &format!("Failed to remove: {}", e)),
    }
}
```

Note: The exact `GatewayContext` method names (`skill_system()`, `shared_token_manager()`) must match the actual API. The implementer should check `src/gateway/mod.rs` for the exact accessor names and adapt.

- [ ] **Step 2: Update handler registrations in mod.rs**

In `src/gateway/handlers/mod.rs`, replace the old skill handler registrations (lines ~301-312):

Remove:
```rust
"markdown_skills.install" => ...
"markdown_skills.load" => ...
"markdown_skills.reload" => ...
"markdown_skills.list" => ...
"markdown_skills.unload" => ...
"skills.list" => ...
"skills.install" => ...
"skills.installFromZip" => ...
"skills.delete" => ...
```

Add:
```rust
"skills.status" => skills::handle_status,
"skills.update" => skills::handle_update,
"skills.install_dep" => skills::handle_install_dep,
"skills.add" => skills::handle_add,
"skills.remove" => skills::handle_remove,
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS (or fix compilation errors from import mismatches)

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/skills.rs src/gateway/handlers/mod.rs
git commit -m "gateway: rewrite skills RPC handlers with unified SkillSystem API"
```

---

## Phase 4: LLM Tools

### Task 8: Create skill_status LLM Tool

**Files:**
- Create: `src/builtin_tools/skill_status.rs`
- Modify: `src/builtin_tools/mod.rs` (register tool)

- [ ] **Step 1: Implement skill_status tool**

Create `src/builtin_tools/skill_status.rs`:

```rust
//! skill_status — LLM Tool for querying skill system status.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::skill::{SkillStatusEntry, SkillStatusFilter, SkillSystem};

#[derive(Deserialize)]
pub struct SkillStatusArgs {
    /// Filter: "all", "ready", "needs_setup", "disabled"
    pub filter: Option<String>,
}

pub struct SkillStatusTool {
    skill_system: SkillSystem,
}

impl SkillStatusTool {
    pub fn new(skill_system: SkillSystem) -> Self {
        Self { skill_system }
    }

    pub async fn call(&self, args: Value) -> Value {
        let filter = args
            .get("filter")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "ready" => SkillStatusFilter::Ready,
                "needs_setup" => SkillStatusFilter::NeedsSetup,
                "disabled" => SkillStatusFilter::Disabled,
                _ => SkillStatusFilter::All,
            })
            .unwrap_or(SkillStatusFilter::All);

        let entries = self.skill_system.full_status().await;
        let filtered: Vec<&SkillStatusEntry> = entries
            .iter()
            .filter(|e| e.matches_filter(filter))
            .collect();

        json!({
            "total": entries.len(),
            "filtered": filtered.len(),
            "skills": filtered,
        })
    }
}
```

The tool schema (for AlephTool registration) should include name `"skill_status"`, description `"Query skill system status. Returns skills filtered by readiness."`, and parameters `{ filter: optional string enum ["all", "ready", "needs_setup", "disabled"] }`.

- [ ] **Step 2: Register in builtin_tools/mod.rs**

Add `pub mod skill_status;` to the module declarations and register the tool in the tool initialization function. Follow the same pattern as `vault_store.rs`.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/skill_status.rs src/builtin_tools/mod.rs
git commit -m "tools: add skill_status LLM Tool for querying skill readiness"
```

---

### Task 9: Create skill_install LLM Tool

**Files:**
- Create: `src/builtin_tools/skill_install.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Implement skill_install tool**

Create `src/builtin_tools/skill_install.rs`:

```rust
//! skill_install — LLM Tool for installing skill dependencies.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::skill::SkillId;
use crate::skill::SkillSystem;

#[derive(Deserialize)]
pub struct SkillInstallArgs {
    pub skill_id: String,
    pub spec_id: Option<String>,
}

pub struct SkillInstallTool {
    skill_system: SkillSystem,
}

impl SkillInstallTool {
    pub fn new(skill_system: SkillSystem) -> Self {
        Self { skill_system }
    }

    pub async fn call(&self, args: Value) -> Value {
        let parsed: SkillInstallArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return json!({ "error": format!("Invalid args: {}", e) }),
        };

        let skill_id = SkillId::new(&parsed.skill_id);
        let result = self
            .skill_system
            .install_dependency(&skill_id, parsed.spec_id.as_deref())
            .await;

        json!({
            "success": result.success,
            "message": result.message,
            "stdout": result.stdout,
            "stderr": result.stderr,
        })
    }
}
```

Tool schema: name `"skill_install"`, description `"Install missing dependencies for a skill. Specify skill_id, optionally spec_id for a specific installer."`, parameters `{ skill_id: required string, spec_id: optional string }`.

- [ ] **Step 2: Register in mod.rs**

Add `pub mod skill_install;` and register.

- [ ] **Step 3: Commit**

```bash
git add src/builtin_tools/skill_install.rs src/builtin_tools/mod.rs
git commit -m "tools: add skill_install LLM Tool for dependency installation"
```

---

### Task 10: Create skill_manage LLM Tool

**Files:**
- Create: `src/builtin_tools/skill_manage.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Implement skill_manage tool**

Create `src/builtin_tools/skill_manage.rs`:

```rust
//! skill_manage — LLM Tool for toggling and configuring skills.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::skill::{PromptScope, SkillId};
use crate::skill::{SkillConfigUpdate, SkillSystem};

#[derive(Deserialize)]
pub struct SkillManageArgs {
    pub skill_id: String,
    pub enabled: Option<bool>,
    pub scope: Option<String>,
}

pub struct SkillManageTool {
    skill_system: SkillSystem,
}

impl SkillManageTool {
    pub fn new(skill_system: SkillSystem) -> Self {
        Self { skill_system }
    }

    pub async fn call(&self, args: Value) -> Value {
        let parsed: SkillManageArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return json!({ "error": format!("Invalid args: {}", e) }),
        };

        let skill_id = SkillId::new(&parsed.skill_id);

        if let Some(enabled) = parsed.enabled {
            if let Err(e) = self
                .skill_system
                .update_config(&skill_id, SkillConfigUpdate::SetEnabled(enabled))
                .await
            {
                return json!({ "error": format!("Failed to update enabled: {}", e) });
            }
        }

        if let Some(scope_str) = &parsed.scope {
            let scope = match scope_str.as_str() {
                "system" => PromptScope::System,
                "tool" => PromptScope::Tool,
                "standalone" => PromptScope::Standalone,
                "disabled" => PromptScope::Disabled,
                _ => return json!({ "error": "Invalid scope" }),
            };
            if let Err(e) = self
                .skill_system
                .update_config(&skill_id, SkillConfigUpdate::SetScope(scope))
                .await
            {
                return json!({ "error": format!("Failed to update scope: {}", e) });
            }
        }

        json!({
            "success": true,
            "skill_id": parsed.skill_id,
            "message": "Skill configuration updated",
        })
    }
}
```

Tool schema: name `"skill_manage"`, description `"Toggle or configure a skill. Set enabled/disabled or change prompt scope."`, parameters `{ skill_id: required string, enabled: optional bool, scope: optional string enum ["system", "tool", "standalone", "disabled"] }`.

- [ ] **Step 2: Register in mod.rs**

Add `pub mod skill_manage;` and register.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/skill_manage.rs src/builtin_tools/mod.rs
git commit -m "tools: add skill_manage LLM Tool for toggling and configuring skills"
```

---

## Phase 5: Panel UI Redesign

### Task 11: Scaffold skills/ directory module

**Files:**
- Create: `interfaces/webchat/src/views/settings/skills/mod.rs`
- Modify: `interfaces/webchat/src/views/settings/mod.rs`

- [ ] **Step 1: Create skills/mod.rs with SkillsView**

Create directory and main component. This component replaces the old monolithic `skills.rs`. It contains the tab bar and delegates to sub-components.

```rust
//! Skills management view — tab bar + list + detail dialog.

mod skill_list;
mod skill_card;
mod skill_detail;
mod skill_install;
mod add_skill;

use leptos::*;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

// Re-export SkillStatusEntry for child components
pub use super::super::types::SkillStatusEntry;

/// Status filter tabs
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SkillTab {
    All,
    Ready,
    NeedsSetup,
    Disabled,
}

/// Main skills settings view
#[component]
pub fn SkillsView() -> impl IntoView {
    let state = use_context::<AppState>().unwrap();
    let (skills, set_skills) = create_signal(Vec::<SkillStatusEntry>::new());
    let (active_tab, set_active_tab) = create_signal(SkillTab::All);
    let (loading, set_loading) = create_signal(false);
    let (selected_skill, set_selected_skill) = create_signal(None::<String>);

    // Load skills on mount
    let load = move || {
        let state = state.clone();
        set_loading.set(true);
        spawn_local(async move {
            if let Ok(response) = state.rpc_call("skills.status", serde_json::json!({})).await {
                if let Some(arr) = response.get("skills").and_then(|v| v.as_array()) {
                    let entries: Vec<SkillStatusEntry> = arr
                        .iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect();
                    set_skills.set(entries);
                }
            }
            set_loading.set(false);
        });
    };
    load();

    // Compute tab counts
    let tab_counts = move || {
        let all = skills.get();
        let ready = all.iter().filter(|s| s.eligible && !s.disabled).count();
        let needs_setup = all.iter().filter(|s| !s.eligible && !s.disabled).count();
        let disabled = all.iter().filter(|s| s.disabled).count();
        (all.len(), ready, needs_setup, disabled)
    };

    // Filtered skills by active tab
    let filtered_skills = move || {
        let tab = active_tab.get();
        skills.get().into_iter().filter(move |s| {
            match tab {
                SkillTab::All => true,
                SkillTab::Ready => s.eligible && !s.disabled,
                SkillTab::NeedsSetup => !s.eligible && !s.disabled,
                SkillTab::Disabled => s.disabled,
            }
        }).collect::<Vec<_>>()
    };

    view! {
        // Tab bar + refresh button + skill list + detail dialog
        // Implementation follows Leptos patterns established in the codebase
        // Child components: skill_list::SkillList, skill_detail::SkillDetail
    }
}
```

Note: The exact Leptos patterns (signal usage, component structure, CSS classes) must match those used elsewhere in `interfaces/webchat/src/views/settings/`. The implementer should read adjacent settings views (e.g., providers, agents) to follow the established style.

- [ ] **Step 2: Update settings/mod.rs**

Change the skills module from a file to a directory:

```rust
// Was: pub mod skills;  (pointing to skills.rs)
// Now: pub mod skills;  (pointing to skills/mod.rs — Rust resolves automatically)
```

Delete the old `interfaces/webchat/src/views/settings/skills.rs` file.

- [ ] **Step 3: Compile check**

Run: `cd interfaces/webchat && cargo check`

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/skills/
git rm interfaces/webchat/src/views/settings/skills.rs
git add interfaces/webchat/src/views/settings/mod.rs
git commit -m "panel: scaffold skills/ directory module with SkillsView + tab bar"
```

---

### Task 12: Implement skill_list and skill_card components

**Files:**
- Create: `interfaces/webchat/src/views/settings/skills/skill_list.rs`
- Create: `interfaces/webchat/src/views/settings/skills/skill_card.rs`

- [ ] **Step 1: Create skill_list.rs**

Groups skills by source (Bundled/Global/Plugin/Workspace) and renders sections:

```rust
//! Skill list with source-based grouping.

use leptos::*;
use std::collections::BTreeMap;

use super::SkillStatusEntry;
use super::skill_card::SkillCard;

#[component]
pub fn SkillList(
    skills: Vec<SkillStatusEntry>,
    on_select: Callback<String>,
) -> impl IntoView {
    // Group by source
    let mut groups: BTreeMap<String, Vec<SkillStatusEntry>> = BTreeMap::new();
    for skill in skills {
        let source = format!("{:?}", skill.source);
        groups.entry(source).or_default().push(skill);
    }

    view! {
        // Render each group as a section with header and skill cards
        // For each skill: SkillCard component with on_select callback
    }
}
```

- [ ] **Step 2: Create skill_card.rs**

Single row: emoji + name + status badge + toggle:

```rust
//! Single skill card row.

use leptos::*;
use super::SkillStatusEntry;

#[component]
pub fn SkillCard(
    skill: SkillStatusEntry,
    on_select: Callback<String>,
) -> impl IntoView {
    let status_class = if skill.disabled {
        "status-disabled"
    } else if skill.eligible {
        "status-ready"
    } else {
        "status-needs-setup"
    };

    let status_text = if skill.disabled {
        "Disabled"
    } else if skill.eligible {
        "Ready"
    } else {
        "Needs Setup"
    };

    view! {
        // Row: [emoji] [name (clickable)] [status badge] [toggle]
        // Click name → on_select(skill.id)
        // Toggle → RPC skills.update { enabled }
    }
}
```

- [ ] **Step 3: Compile check**

Run: `cd interfaces/webchat && cargo check`

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/skills/skill_list.rs
git add interfaces/webchat/src/views/settings/skills/skill_card.rs
git commit -m "panel: add skill_list (source grouping) and skill_card (row) components"
```

---

### Task 13: Implement skill_detail dialog

**Files:**
- Create: `interfaces/webchat/src/views/settings/skills/skill_detail.rs`

- [ ] **Step 1: Create skill_detail.rs**

Dialog with: requirements checklist, install button, API key input, settings, info:

```rust
//! Skill detail dialog — full management interface for a single skill.

use leptos::*;
use super::SkillStatusEntry;
use super::skill_install::SkillInstallButton;

#[component]
pub fn SkillDetail(
    skill: SkillStatusEntry,
    on_close: Callback<()>,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let (api_key, set_api_key) = create_signal(String::new());
    let (message, set_message) = create_signal(None::<String>);

    view! {
        // Modal dialog overlay
        // Header: name + close button
        // Status badges: [Ready/Needs Setup/Disabled] [Source] [Scope]
        // Description text
        //
        // Requirements section:
        //   For each required binary: ✅ or ⚠️ + install button
        //   For each missing env: ⚠️ marker
        //
        // API Key section (if primary_env is Some):
        //   Password input + Save button
        //   Link to homepage
        //
        // Settings section:
        //   Enabled toggle
        //   Scope dropdown (System/Tool/Standalone/Disabled)
        //
        // Info section:
        //   Source, ID, Homepage link
    }
}
```

- [ ] **Step 2: Compile check and commit**

```bash
git add interfaces/webchat/src/views/settings/skills/skill_detail.rs
git commit -m "panel: add skill_detail dialog with requirements, API key, settings"
```

---

### Task 14: Implement skill_install and add_skill components

**Files:**
- Create: `interfaces/webchat/src/views/settings/skills/skill_install.rs`
- Create: `interfaces/webchat/src/views/settings/skills/add_skill.rs`

- [ ] **Step 1: Create skill_install.rs**

Install button with loading state and result feedback:

```rust
//! Install button with progress and result feedback.

use leptos::*;

#[component]
pub fn SkillInstallButton(
    skill_id: String,
    spec_id: String,
    label: String,
    on_success: Callback<()>,
) -> impl IntoView {
    let (installing, set_installing) = create_signal(false);
    let (result_msg, set_result_msg) = create_signal(None::<(bool, String)>);

    // Click handler: call skills.install_dep RPC
    // Show spinner while installing
    // On success: green message + call on_success
    // On failure: red message + stderr excerpt

    view! {
        // [Install X (brew)] button / spinner / result message
    }
}
```

- [ ] **Step 2: Create add_skill.rs**

Dialog for adding new skills from URL:

```rust
//! Add Skill dialog — install from URL or zip.

use leptos::*;

#[component]
pub fn AddSkillDialog(
    on_close: Callback<()>,
    on_success: Callback<()>,
) -> impl IntoView {
    let (source, set_source) = create_signal(String::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);

    // Submit: call skills.add RPC with source URL
    // On success: close dialog + trigger refresh
    // On error: show error message

    view! {
        // Modal: URL input + Submit button + error display
    }
}
```

- [ ] **Step 3: Compile check and commit**

```bash
git add interfaces/webchat/src/views/settings/skills/skill_install.rs
git add interfaces/webchat/src/views/settings/skills/add_skill.rs
git commit -m "panel: add skill_install button and add_skill dialog components"
```

---

## Phase 6: Migration and Cleanup

### Task 15: Migrate legacy skill callers

**Files:**
- Modify: Multiple files that import from `crate::skills::` (legacy module)

**Important:** The list below is partial — the actual codebase has ~18 files importing from `crate::skills::`. Step 1 (grep) is authoritative. Known callers include but are not limited to:
1. `src/gateway/handlers/skills.rs` — Already rewritten in Task 7
2. `src/extension/types/mod.rs` — Remove `pub use skills::*` re-export
3. `src/extension/registry/types.rs` — Update skill type references
4. `src/lib.rs` — Update re-exports
5. `src/capability/strategies/skills.rs` — Update `SkillsRegistry` → `SkillRegistry`
6. `src/capability/mod.rs` — Same
7. `src/dispatcher/registry/registration.rs` — Update `SkillInfo` usage
8. `src/dispatcher/registry/mod.rs` — Same
9. `src/dispatcher/registry/state.rs` — Same
10. `src/dispatcher/tool_index/coordinator.rs` — Update `SkillRegistryEvent`, `SkillsRegistry`
11. `src/dispatcher/tool_index/tests.rs` — Update test imports
12. `src/init_unified/coordinator.rs` — Update `SkillsRegistry`
13. `src/builtin_tools/skill_reader.rs` — Update skill lookup
14. `src/builtin_tools/clawhub.rs` — Update skill install references
15. `src/skills/installer.rs`, `registry.rs`, `health.rs`, `cli_wrapper.rs` — These ARE the legacy module, deleted in Task 17

- [ ] **Step 1: Find all callers**

Run: `grep -r "crate::skills::" src/ --include="*.rs" -l`

This will give the definitive list. For each file:
- Replace `crate::skills::SkillInfo` → use the domain type or new `SkillStatusEntry`
- Replace `crate::skills::SkillsRegistry` → `crate::skill::SkillRegistry` or `crate::skill::SkillSystem`
- Replace `crate::skills::SkillRegistryEvent` → adapt to new event mechanism or remove

- [ ] **Step 2: Update each caller one by one**

For each file, read current usage, map to the new API, and update. Compile between files.

- [ ] **Step 3: Compile check after all migrations**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor: migrate all legacy skills:: callers to skill:: module"
```

---

### Task 16: Thin extension/skill_ops.rs to delegation

**Files:**
- Modify: `src/extension/skill_ops.rs:1-222`

- [ ] **Step 1: Update ExtensionManager methods to delegate to SkillSystem**

The `ExtensionManager` needs a reference to `SkillSystem`. Add it as a field or parameter. Then delegate:

```rust
// get_all_skills → skill_system.list_skills()
// get_skill → skill_system.get_skill()
// execute_skill → skill_system methods
```

Keep the API signatures compatible so callers don't break. Internal implementation changes only.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git add src/extension/skill_ops.rs
git commit -m "refactor: thin skill_ops.rs to delegate to SkillSystem"
```

---

### Task 17: Delete legacy modules and clean up

**Files to delete:**
- `src/gateway/handlers/markdown_skills.rs`
- Remove `markdown_skills` module declaration from `src/gateway/handlers/mod.rs`
- Remove `markdown_skills.*` handler registrations from handler registry

**Files to clean:**
- `src/extension/types/skills.rs` — Remove `ExtensionSkill` type alias if all callers migrated
- `src/tools/markdown_skill/` — Evaluate what can be deleted vs. what still serves runtime loading

- [ ] **Step 1: Delete markdown_skills RPC handler**

```bash
git rm src/gateway/handlers/markdown_skills.rs
```

Remove its module declaration and handler registrations from `mod.rs`.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Fix any compilation errors from missing references.

- [ ] **Step 3: Evaluate legacy skills/ module**

Check if any code still references `src/skills/`. If all callers have been migrated (Task 15), the module can be deleted:

```bash
grep -r "crate::skills" src/ --include="*.rs" | grep -v "crate::skill::"
```

If no results, delete the legacy module.

- [ ] **Step 4: Final compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "cleanup: delete legacy skills module, markdown_skills handler, dead code"
```

---

## Phase 7: Integration Testing

### Task 18: End-to-end verification

- [ ] **Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 2: Run clippy**

```bash
just clippy
```

Fix any warnings.

- [ ] **Step 3: Manual verification checklist**

Start the server and verify:
1. `skills.status` RPC returns entries with eligibility + install options
2. `skills.update` toggles enabled/disabled and persists to `~/.aleph/data/skills.toml`
3. Panel UI shows tab bar with correct counts
4. Clicking a skill opens detail dialog
5. Install button works for a simple dependency

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "skill: complete skill system unification — unified model, status, install, UI, tools"
```
