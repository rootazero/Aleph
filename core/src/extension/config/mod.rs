//! Extension configuration system
//!
//! Handles aleph.jsonc configuration with multi-level merging.
//! Now also supports aether.toml as the preferred format.

mod types;
pub mod loader;
pub mod migrate;

pub use types::*;
pub use loader::{find_config_file, load_config_file, load_extension_config};
pub use migrate::{migrate_to_toml, needs_migration, MigrationResult};

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

        // Also check for .json variant
        let alt_files = discovery.find_config_files(ALEPH_CONFIG_FILE_ALT)?;

        // Merge all configs in priority order
        let mut all_files: Vec<_> = config_files
            .into_iter()
            .chain(alt_files)
            .collect();

        // Deduplicate (prefer .jsonc over .json for same directory)
        all_files.sort();
        all_files.dedup_by(|a, b| {
            a.parent() == b.parent() && a.extension() != b.extension()
        });

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
        if let Ok(content) = std::env::var("AETHER_CONFIG_CONTENT") {
            match self.merge_json_str(&content) {
                Ok(()) => {
                    info!("Loaded inline config from AETHER_CONFIG_CONTENT");
                }
                Err(e) => {
                    warn!("Failed to parse AETHER_CONFIG_CONTENT: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Load a config file and merge it
    async fn load_and_merge(&mut self, path: &Path) -> Result<(), ExtensionError> {
        let content = tokio::fs::read_to_string(path).await?;

        // Parse JSONC (with comments)
        let parsed = parse_jsonc(&content, path)?;

        // Merge into current config
        self.merge(parsed);

        Ok(())
    }

    /// Merge a JSON string
    fn merge_json_str(&mut self, content: &str) -> Result<(), ExtensionError> {
        let parsed: AlephConfig = serde_json::from_str(content)
            .map_err(|e| ExtensionError::ConfigMerge(format!("JSON parse error: {}", e)))?;

        self.merge(parsed);
        Ok(())
    }

    /// Merge another config into this one
    fn merge(&mut self, other: AlephConfig) {
        // Plugins are concatenated
        if let Some(plugins) = other.plugin {
            let existing = self.config.plugin.get_or_insert_with(Vec::new);
            for plugin in plugins {
                if !existing.contains(&plugin) {
                    existing.push(plugin);
                }
            }
        }

        // Instructions are concatenated
        if let Some(instructions) = other.instructions {
            let existing = self.config.instructions.get_or_insert_with(Vec::new);
            for inst in instructions {
                if !existing.contains(&inst) {
                    existing.push(inst);
                }
            }
        }

        // Agents are merged (later overrides earlier)
        if let Some(agents) = other.agent {
            let existing = self.config.agent.get_or_insert_with(HashMap::new);
            for (name, agent) in agents {
                existing.insert(name, agent);
            }
        }

        // MCP servers are merged
        if let Some(mcp) = other.mcp {
            let existing = self.config.mcp.get_or_insert_with(HashMap::new);
            for (name, server) in mcp {
                existing.insert(name, server);
            }
        }

        // Permission is merged
        if let Some(permission) = other.permission {
            let existing = self.config.permission.get_or_insert_with(HashMap::new);
            for (tool, rule) in permission {
                existing.insert(tool, rule);
            }
        }

        // Simple fields use later value
        if other.model.is_some() {
            self.config.model = other.model;
        }
        if other.small_model.is_some() {
            self.config.small_model = other.small_model;
        }
        if other.default_agent.is_some() {
            self.config.default_agent = other.default_agent;
        }
    }

    /// Get the merged configuration
    pub fn get_config(&self) -> &AlephConfig {
        &self.config
    }

    /// Get the list of source files
    pub fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    /// Get a specific agent config
    pub fn get_agent(&self, name: &str) -> Option<&AgentConfigOverride> {
        self.config.agent.as_ref()?.get(name)
    }

    /// Get plugins list
    pub fn get_plugins(&self) -> &[String] {
        self.config.plugin.as_deref().unwrap_or(&[])
    }

    /// Get MCP servers
    pub fn get_mcp_servers(&self) -> Option<&HashMap<String, McpConfig>> {
        self.config.mcp.as_ref()
    }
}

/// Parse JSONC (JSON with comments)
fn parse_jsonc(content: &str, path: &Path) -> Result<AlephConfig, ExtensionError> {
    // Remove single-line comments
    let mut cleaned = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        // Handle inline comments
        if let Some(pos) = line.find("//") {
            // Check if it's inside a string (naive check)
            let before = &line[..pos];
            let quote_count = before.matches('"').count();
            if quote_count % 2 == 0 {
                // Not inside a string, remove comment
                cleaned.push_str(&line[..pos]);
                cleaned.push('\n');
                continue;
            }
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }

    // Remove block comments /* ... */
    let mut result = String::new();
    let mut chars = cleaned.chars().peekable();
    let mut in_string = false;

    while let Some(ch) = chars.next() {
        if ch == '"' && !in_string {
            in_string = true;
            result.push(ch);
        } else if ch == '"' && in_string {
            in_string = false;
            result.push(ch);
        } else if ch == '/' && !in_string {
            if chars.peek() == Some(&'*') {
                chars.next(); // consume *
                // Skip until */
                loop {
                    match chars.next() {
                        Some('*') if chars.peek() == Some(&'/') => {
                            chars.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }

    // Handle trailing commas (common in JSONC)
    // Use regex to handle commas followed by whitespace before ] or }
    let trailing_comma_re = regex::Regex::new(r",(\s*[\]}])").unwrap();
    let result = trailing_comma_re.replace_all(&result, "$1").to_string();

    serde_json::from_str(&result)
        .map_err(|e| ExtensionError::config_parse(path, format!("JSONC parse error: {}", e)))
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
