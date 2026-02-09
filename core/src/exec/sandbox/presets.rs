use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use super::capabilities::{
    Capabilities, FileSystemCapability, NetworkCapability,
    ProcessCapability, EnvironmentCapability,
};

/// Registry of preset capability templates
#[derive(Debug, Clone)]
pub struct PresetRegistry {
    presets: HashMap<String, PresetDefinition>,
}

/// Definition of a capability preset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetDefinition {
    pub name: String,
    pub description: String,
    pub capabilities: Capabilities,
    /// Fields that cannot be overridden (hard ceiling)
    pub immutable_fields: Vec<String>,
}

impl PresetRegistry {
    /// Get a preset by name
    pub fn get(&self, name: &str) -> Option<&PresetDefinition> {
        self.presets.get(name)
    }

    /// List all available preset names
    pub fn list_presets(&self) -> Vec<String> {
        self.presets.keys().cloned().collect()
    }
}

impl Default for PresetRegistry {
    fn default() -> Self {
        let mut presets = HashMap::new();

        // file_processor preset
        presets.insert(
            "file_processor".to_string(),
            PresetDefinition {
                name: "file_processor".to_string(),
                description: "File processing tools with no network access".to_string(),
                capabilities: Capabilities {
                    filesystem: vec![FileSystemCapability::TempWorkspace],
                    network: NetworkCapability::Deny,
                    process: ProcessCapability {
                        no_fork: true,
                        max_execution_time: 300,
                        max_memory_mb: Some(512),
                    },
                    environment: EnvironmentCapability::Restricted,
                },
                immutable_fields: vec!["network".to_string()],
            },
        );

        // web_scraper preset
        presets.insert(
            "web_scraper".to_string(),
            PresetDefinition {
                name: "web_scraper".to_string(),
                description: "Web scraping tools with network access".to_string(),
                capabilities: Capabilities {
                    filesystem: vec![FileSystemCapability::TempWorkspace],
                    network: NetworkCapability::AllowAll,
                    process: ProcessCapability {
                        no_fork: true,
                        max_execution_time: 600,
                        max_memory_mb: Some(1024),
                    },
                    environment: EnvironmentCapability::Restricted,
                },
                immutable_fields: vec!["filesystem".to_string()],
            },
        );

        // code_analyzer preset
        presets.insert(
            "code_analyzer".to_string(),
            PresetDefinition {
                name: "code_analyzer".to_string(),
                description: "Code analysis tools with read-only workspace access".to_string(),
                capabilities: Capabilities {
                    filesystem: vec![FileSystemCapability::ReadOnly {
                        path: "${WORKSPACE}".into(),
                    }],
                    network: NetworkCapability::Deny,
                    process: ProcessCapability {
                        no_fork: true,
                        max_execution_time: 900,
                        max_memory_mb: Some(2048),
                    },
                    environment: EnvironmentCapability::Restricted,
                },
                immutable_fields: vec!["network".to_string()],
            },
        );

        // data_transformer preset
        presets.insert(
            "data_transformer".to_string(),
            PresetDefinition {
                name: "data_transformer".to_string(),
                description: "Data transformation tools with temp workspace and read-only data access".to_string(),
                capabilities: Capabilities {
                    filesystem: vec![
                        FileSystemCapability::TempWorkspace,
                        FileSystemCapability::ReadOnly {
                            path: "${PROJECT_ROOT}/data".into(),
                        },
                    ],
                    network: NetworkCapability::Deny,
                    process: ProcessCapability {
                        no_fork: true,
                        max_execution_time: 1800,
                        max_memory_mb: Some(4096),
                    },
                    environment: EnvironmentCapability::Restricted,
                },
                immutable_fields: vec![],
            },
        );

        Self { presets }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preset_registry_get_file_processor() {
        let registry = PresetRegistry::default();
        let preset = registry.get("file_processor").unwrap();
        assert_eq!(preset.name, "file_processor");
        assert!(matches!(preset.capabilities.network, NetworkCapability::Deny));
    }

    #[test]
    fn test_preset_registry_get_web_scraper() {
        let registry = PresetRegistry::default();
        let preset = registry.get("web_scraper").unwrap();
        assert_eq!(preset.name, "web_scraper");
        assert!(matches!(preset.capabilities.network, NetworkCapability::AllowAll));
    }

    #[test]
    fn test_preset_registry_get_code_analyzer() {
        let registry = PresetRegistry::default();
        let preset = registry.get("code_analyzer").unwrap();
        assert_eq!(preset.name, "code_analyzer");
        assert!(matches!(preset.capabilities.network, NetworkCapability::Deny));
    }

    #[test]
    fn test_preset_registry_get_data_transformer() {
        let registry = PresetRegistry::default();
        let preset = registry.get("data_transformer").unwrap();
        assert_eq!(preset.name, "data_transformer");
        assert_eq!(preset.capabilities.process.max_execution_time, 1800);
    }

    #[test]
    fn test_preset_registry_list_presets() {
        let registry = PresetRegistry::default();
        let presets = registry.list_presets();
        assert_eq!(presets.len(), 4);
        assert!(presets.contains(&"file_processor".to_string()));
        assert!(presets.contains(&"web_scraper".to_string()));
        assert!(presets.contains(&"code_analyzer".to_string()));
        assert!(presets.contains(&"data_transformer".to_string()));
    }
}
