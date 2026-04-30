//! Skill configuration persistence — stores user preferences per skill.
//! Non-sensitive config (enabled/disabled, scope override, install preferences)
//! persists to ~/.aleph/data/skills.toml. API keys route to the Vault.

use crate::domain::skill::{PromptScope, SkillId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Node.js package manager preference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NodeManager {
    #[default]
    Npm,
    Pnpm,
    Yarn,
    Bun,
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
}

/// Update request for a single skill's config.
#[derive(Debug)]
pub enum SkillConfigUpdate {
    SetEnabled(bool),
    SetScope(PromptScope),
}

impl SkillsConfig {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| toml::from_str(&content).ok())
            .unwrap_or_default()
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
