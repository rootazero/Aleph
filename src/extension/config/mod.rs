//! Extension configuration system
//!
//! Handles aleph.jsonc configuration with multi-level merging.
//! Now also supports aleph.toml as the preferred format.

pub mod loader;
pub mod migrate;
mod types;

pub use loader::{find_config_file, load_config_file, load_extension_config};
pub use migrate::{migrate_to_toml, needs_migration, MigrationResult};
pub use types::*;

use crate::discovery::{DiscoveryManager, ALEPH_CONFIG_FILE, ALEPH_CONFIG_FILE_ALT};
use crate::extension::ExtensionError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Configuration manager for aleph.jsonc
#[derive(Debug)]
pub struct ConfigManager {
    /// Merged configuration
    config: AlephConfig,
    /// Source files that contributed to the config
    sources: Vec<PathBuf>,
}

impl ConfigManager {
    /// Create a new config manager, loading and merging configurations
    pub async fn new(discovery: &DiscoveryManager) -> Result<Self, ExtensionError> {
        let mut manager = Self {
            config: AlephConfig::default(),
            sources: Vec::new(),
        };

        manager.load_all(discovery).await?;
        Ok(manager)
    }

    /// Load and merge all configuration files
    async fn load_all(&mut self, discovery: &DiscoveryManager) -> Result<(), ExtensionError> {
        // Find all config files
        let config_files = discovery.find_config_files(ALEPH_CONFIG_FILE)?;
        let alt_files = discovery.find_config_files(ALEPH_CONFIG_FILE_ALT)?;
        let toml_files = discovery.find_config_files("aleph.toml")?;

        let mut all_files: Vec<_> = config_files
            .into_iter()
            .chain(alt_files)
            .chain(toml_files)
            .collect();

        let global_config_dir = discovery.aleph_home().ok();
        all_files.sort_by(|a, b| {
            let ext_rank = |p: &Path| match p.extension().and_then(|e| e.to_str()) {
                Some("toml") => 0,
                Some("jsonc") => 1,
                _ => 2,
            };
            let layer_rank = |p: &Path| {
                if p.parent() == global_config_dir.as_deref() {
                    0
                } else {
                    1
                }
            };
            let depth = |p: &Path| p.parent().map_or(0, |parent| parent.components().count());
            layer_rank(a)
                .cmp(&layer_rank(b))
                .then_with(|| depth(a).cmp(&depth(b)))
                .then_with(|| a.parent().cmp(&b.parent()))
                .then_with(|| ext_rank(a).cmp(&ext_rank(b)))
                .then_with(|| a.cmp(b))
        });
        all_files.dedup_by(|a, b| a.parent() == b.parent() && a.extension() != b.extension());

        debug!("Found {} config files to merge", all_files.len());

        for file in all_files {
            match self.load_and_merge(&file).await {
                Ok(()) => {
                    self.sources.push(file.clone());
                    info!("Loaded config from: {:?}", file);
                }
                Err(e) => {
                    warn!("Failed to load config from {:?}: {}", file, e);
                }
            }
        }

        // Check for inline config from environment
        if let Ok(content) = std::env::var("ALEPH_CONFIG_CONTENT") {
            match self.merge_json_str(&content) {
                Ok(()) => {
                    info!("Loaded inline config from ALEPH_CONFIG_CONTENT");
                }
                Err(e) => {
                    warn!("Failed to parse ALEPH_CONFIG_CONTENT: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Load a config file and merge it
    async fn load_and_merge(&mut self, path: &Path) -> Result<(), ExtensionError> {
        let content = tokio::fs::read_to_string(path).await?;

        let parsed = match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => toml::from_str(&content)
                .map_err(|e| ExtensionError::config_parse(path, format!("TOML parse error: {e}")))?,
            _ => parse_jsonc(&content, path)?,
        };

        self.merge(parsed);

        Ok(())
    }

    /// Merge a JSON string
    fn merge_json_str(&mut self, content: &str) -> Result<(), ExtensionError> {
        let parsed: AlephConfig = serde_json::from_str(content)
            .map_err(|e| ExtensionError::ConfigMerge(format!("JSON parse error: {e}")))?;

        self.merge(parsed);
        Ok(())
    }

    /// Merge another config into this one
    fn merge(&mut self, other: AlephConfig) {
        if let Some(plugins) = other.plugin {
            let existing = self.config.plugin.get_or_insert_with(Vec::new);
            for plugin in plugins {
                if !existing.contains(&plugin) {
                    existing.push(plugin);
                }
            }
        }

        if let Some(instructions) = other.instructions {
            let existing = self.config.instructions.get_or_insert_with(Vec::new);
            for inst in instructions {
                if !existing.contains(&inst) {
                    existing.push(inst);
                }
            }
        }

        if let Some(agents) = other.agent {
            let existing = self.config.agent.get_or_insert_with(HashMap::new);
            for (name, agent) in agents {
                existing.insert(name, agent);
            }
        }

        if let Some(mcp) = other.mcp {
            let existing = self.config.mcp.get_or_insert_with(HashMap::new);
            for (name, server) in mcp {
                existing.insert(name, server);
            }
        }

        if let Some(permission) = other.permission {
            let existing = self.config.permission.get_or_insert_with(HashMap::new);
            for (tool, rule) in permission {
                existing.insert(tool, rule);
            }
        }

        if let Some(provider) = other.provider {
            let existing = self.config.provider.get_or_insert_with(HashMap::new);
            for (name, config) in provider {
                existing.insert(name, config);
            }
        }

        if other.schema.is_some() {
            self.config.schema = other.schema;
        }
        if other.model.is_some() {
            self.config.model = other.model;
        }
        if other.small_model.is_some() {
            self.config.small_model = other.small_model;
        }
        if other.default_agent.is_some() {
            self.config.default_agent = other.default_agent;
        }
        if other.disabled_providers.is_some() {
            self.config.disabled_providers = other.disabled_providers;
        }
        if other.enabled_providers.is_some() {
            self.config.enabled_providers = other.enabled_providers;
        }
        if other.compaction.is_some() {
            self.config.compaction = other.compaction;
        }
        if other.experimental.is_some() {
            self.config.experimental = other.experimental;
        }
    }

    /// Get the merged configuration
    #[must_use]
    pub const fn get_config(&self) -> &AlephConfig {
        &self.config
    }

    /// Get the list of source files
    #[must_use]
    pub fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    /// Get a specific agent config
    #[must_use]
    pub fn get_agent(&self, name: &str) -> Option<&AgentConfigOverride> {
        self.config.agent.as_ref()?.get(name)
    }

    /// Get plugins list
    #[must_use]
    pub fn get_plugins(&self) -> &[String] {
        self.config.plugin.as_deref().unwrap_or(&[])
    }

    /// Get MCP servers
    #[must_use]
    pub const fn get_mcp_servers(&self) -> Option<&HashMap<String, McpConfig>> {
        self.config.mcp.as_ref()
    }
}

/// Parse JSONC (JSON with comments)
fn parse_jsonc(content: &str, path: &Path) -> Result<AlephConfig, ExtensionError> {
    // Strip comments via the shared, escape-aware implementation (the previous
    // naive line/quote-count stripper mis-tracked `\"` inside strings and could
    // corrupt valid config).
    let stripped = loader::strip_json_comments(content);

    // Handle trailing commas (common in JSONC)
    // Use regex to handle commas followed by whitespace before ] or }
    let trailing_comma_re = regex::Regex::new(r",(\s*[\]}])").map_err(|e| {
        ExtensionError::config_parse(path, format!("Invalid trailing comma regex: {e}"))
    })?;
    let result = trailing_comma_re.replace_all(&stripped, "$1").to_string();

    serde_json::from_str(&result)
        .map_err(|e| ExtensionError::config_parse(path, format!("JSONC parse error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jsonc_comments() {
        let content = r#"{
  // This is a comment
  "model": "anthropic/claude-4",
  "plugin": [
    "plugin-a" // inline comment
  ]
}"#;

        let config = parse_jsonc(content, Path::new("/test")).unwrap();
        assert_eq!(config.model, Some("anthropic/claude-4".to_string()));
    }

    #[test]
    fn test_parse_jsonc_trailing_comma() {
        let content = r#"{
  "plugin": [
    "plugin-a",
    "plugin-b",
  ],
}"#;

        let config = parse_jsonc(content, Path::new("/test")).unwrap();
        let plugins = config.plugin.unwrap();
        assert_eq!(plugins.len(), 2);
    }

    #[test]
    fn test_config_merge() {
        let mut manager = ConfigManager {
            config: AlephConfig::default(),
            sources: Vec::new(),
        };

        // First config
        let config1 = AlephConfig {
            plugin: Some(vec!["plugin-a".to_string()]),
            model: Some("model-1".to_string()),
            ..Default::default()
        };
        manager.merge(config1);

        // Second config
        let config2 = AlephConfig {
            plugin: Some(vec!["plugin-b".to_string()]),
            model: Some("model-2".to_string()),
            ..Default::default()
        };
        manager.merge(config2);

        // Plugins should be concatenated
        let plugins = manager.config.plugin.as_ref().unwrap();
        assert!(plugins.contains(&"plugin-a".to_string()));
        assert!(plugins.contains(&"plugin-b".to_string()));

        // Model should be overridden
        assert_eq!(manager.config.model, Some("model-2".to_string()));
    }
}
