//! Skill configuration persistence — stores user preferences per skill.
//! Non-sensitive config (enabled/disabled, scope override, install preferences)
//! persists to ~/.aleph/data/skills.toml. API keys route to the Vault.

use crate::domain::skill::{PromptScope, SkillId};
use crate::skill::prompt::SkillPromptBudget;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Global install preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPreferences {
    #[serde(default)]
    pub prefer_brew: bool,
}
impl Default for InstallPreferences {
    fn default() -> Self {
        Self {
            prefer_brew: cfg!(target_os = "macos"),
        }
    }
}

/// Per-skill configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillEntryConfig {
    pub enabled: Option<bool>,
    pub scope_override: Option<PromptScope>,
}

/// Root config, persisted as TOML.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsConfig {
    #[serde(default)]
    pub install_preferences: InstallPreferences,
    #[serde(default)]
    pub entries: HashMap<String, SkillEntryConfig>,
    /// Budget bounding the injected `<available_skills>` prompt index. Lets a
    /// host with a large skill library trade detail for context (or lift the
    /// cap entirely). Omitted → built-in default (64 skills / 12k chars).
    #[serde(default)]
    pub prompt_budget: SkillPromptBudget,
}

/// Update request for a single skill's config.
#[derive(Debug)]
pub enum SkillConfigUpdate {
    SetEnabled(bool),
    SetScope(PromptScope),
}

impl SkillsConfig {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "skills config parse failed; using defaults");
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "skills config read failed; using defaults");
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let content = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp_path = path.with_extension("toml.tmp");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp_path, content)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    #[must_use]
    pub fn get_entry(&self, id: &SkillId) -> Option<&SkillEntryConfig> {
        self.entries.get(id.as_str())
    }

    pub fn apply_update(&mut self, id: &SkillId, update: SkillConfigUpdate) {
        let entry = self.entries.entry(id.as_str().to_string()).or_default();
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
        assert!(loaded.install_preferences.prefer_brew);
        let entry = loaded.entries.get("test:skill").unwrap();
        assert_eq!(entry.enabled, Some(false));
    }

    #[test]
    fn load_missing_file_returns_default() {
        let config = SkillsConfig::load(Path::new("/nonexistent/path.toml"));
        assert!(config.entries.is_empty());
    }

    #[test]
    fn prompt_budget_defaults_when_absent() {
        // A config TOML with no [prompt_budget] table deserializes to the
        // built-in default budget (64 skills / 12k chars).
        let config: SkillsConfig = toml::from_str("").unwrap();
        assert_eq!(config.prompt_budget, SkillPromptBudget::default());
    }

    #[test]
    fn prompt_budget_partial_table_fills_missing_field() {
        // Only max_skills set → max_chars falls back to the default via the
        // container-level #[serde(default)] on SkillPromptBudget.
        let config: SkillsConfig = toml::from_str("[prompt_budget]\nmax_skills = 8\n").unwrap();
        assert_eq!(config.prompt_budget.max_skills, 8);
        assert_eq!(
            config.prompt_budget.max_chars,
            SkillPromptBudget::default().max_chars
        );
    }

    #[test]
    fn prompt_budget_roundtrips_toml() {
        let config = SkillsConfig {
            prompt_budget: SkillPromptBudget {
                max_skills: 10,
                max_chars: 2000,
            },
            ..Default::default()
        };
        let tmp = NamedTempFile::new().unwrap();
        config.save(tmp.path()).unwrap();
        let loaded = SkillsConfig::load(tmp.path());
        assert_eq!(loaded.prompt_budget.max_skills, 10);
        assert_eq!(loaded.prompt_budget.max_chars, 2000);
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
