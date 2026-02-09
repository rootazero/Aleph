use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Binding between tool parameter and capability
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParameterBinding {
    /// Capability string: "filesystem.read_only", "filesystem.read_write"
    pub capability: String,
    /// Validation rule: is_file, is_directory
    pub validation: ValidationRule,
    /// Mapping type: single, each_element (for arrays)
    #[serde(default)]
    pub mapping: MappingType,
}

/// Validation rule for parameter values
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationRule {
    IsFile,
    IsDirectory,
    IsPath,
    None,
}

impl Default for ValidationRule {
    fn default() -> Self {
        Self::None
    }
}

/// Mapping type for parameter binding
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MappingType {
    Single,
    EachElement,
}

impl Default for MappingType {
    fn default() -> Self {
        Self::Single
    }
}

/// Required capabilities declaration for a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredCapabilities {
    pub base_preset: String,
    pub description: String,
    #[serde(default)]
    pub overrides: CapabilityOverrides,
    #[serde(default)]
    pub parameter_bindings: HashMap<String, ParameterBinding>,
}

/// Capability overrides for preset customization
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityOverrides {
    #[serde(default)]
    pub filesystem: Vec<FileSystemOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<super::capabilities::NetworkCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<super::capabilities::EnvironmentCapability>,
}

/// Filesystem override specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemOverride {
    #[serde(rename = "type")]
    pub fs_type: String,  // "read_only", "read_write"
    pub path: String,
    pub reason: String,
}

/// Process override specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_execution_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory_mb: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameter_binding_serialization() {
        let binding = ParameterBinding {
            capability: "filesystem.read_only".to_string(),
            validation: ValidationRule::IsFile,
            mapping: MappingType::Single,
        };
        let json = serde_json::to_string(&binding).unwrap();
        let deserialized: ParameterBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.capability, "filesystem.read_only");
    }
}
