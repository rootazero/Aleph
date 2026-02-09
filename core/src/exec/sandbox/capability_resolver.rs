use crate::error::{AlephError, Result};
use super::capabilities::{Capabilities, FileSystemCapability};
use super::parameter_binding::{CapabilityOverrides, FileSystemOverride, ParameterBinding, ValidationRule, MappingType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;

/// Apply capability overrides to base capabilities
pub fn apply_overrides(
    mut base: Capabilities,
    overrides: &CapabilityOverrides,
    immutable_fields: &[String],
) -> Result<Capabilities> {
    // Apply filesystem overrides
    for fs_override in &overrides.filesystem {
        let cap = match fs_override.fs_type.as_str() {
            "read_only" => FileSystemCapability::ReadOnly {
                path: PathBuf::from(&fs_override.path),
            },
            "read_write" => FileSystemCapability::ReadWrite {
                path: PathBuf::from(&fs_override.path),
            },
            _ => {
                return Err(AlephError::InvalidConfig {
                    message: format!("Invalid filesystem type: {}", fs_override.fs_type),
                    suggestion: Some("Use 'read_only' or 'read_write'".to_string()),
                })
            }
        };
        base.filesystem.push(cap);
    }

    // Apply network overrides (check immutability)
    if let Some(ref network) = overrides.network {
        if immutable_fields.contains(&"network".to_string()) {
            return Err(AlephError::InvalidConfig {
                message: "Network capability is immutable for this preset".to_string(),
                suggestion: Some("Remove network override or choose a different preset".to_string()),
            });
        }
        base.network = network.clone();
    }

    // Apply process overrides
    if let Some(ref process) = overrides.process {
        if let Some(max_time) = process.max_execution_time {
            base.process.max_execution_time = max_time;
        }
        if let Some(max_mem) = process.max_memory_mb {
            base.process.max_memory_mb = Some(max_mem);
        }
    }

    // Apply environment overrides (check immutability)
    if let Some(ref env) = overrides.environment {
        if immutable_fields.contains(&"environment".to_string()) {
            return Err(AlephError::InvalidConfig {
                message: "Environment capability is immutable for this preset".to_string(),
                suggestion: Some("Remove environment override or choose a different preset".to_string()),
            });
        }
        base.environment = env.clone();
    }

    Ok(base)
}

/// Bind tool parameters to capabilities
pub fn bind_parameters(
    caps: &mut Capabilities,
    bindings: &HashMap<String, ParameterBinding>,
    parameters: &serde_json::Value,
) -> Result<()> {
    for (param_name, binding) in bindings {
        let param_value = parameters.get(param_name).ok_or_else(|| {
            AlephError::InvalidConfig {
                message: format!("Missing parameter: {}", param_name),
                suggestion: Some("Provide all required parameters".to_string()),
            }
        })?;

        match binding.mapping {
            MappingType::Single => {
                bind_single_parameter(caps, binding, param_value)?;
            }
            MappingType::EachElement => {
                bind_array_parameter(caps, binding, param_value)?;
            }
        }
    }

    Ok(())
}

fn bind_single_parameter(
    caps: &mut Capabilities,
    binding: &ParameterBinding,
    value: &serde_json::Value,
) -> Result<()> {
    let path_str = value.as_str().ok_or_else(|| {
        AlephError::InvalidConfig {
            message: "Parameter value must be a string".to_string(),
            suggestion: Some("Provide a valid file path string".to_string()),
        }
    })?;

    // Validate parameter
    validate_parameter(path_str, &binding.validation)?;

    // Canonicalize path
    let path = fs::canonicalize(path_str).map_err(|e| {
        AlephError::InvalidConfig {
            message: format!("Invalid path {}: {}", path_str, e),
            suggestion: Some("Ensure the path exists and is accessible".to_string()),
        }
    })?;

    // Add capability
    match binding.capability.as_str() {
        "filesystem.read_only" => {
            caps.filesystem.push(FileSystemCapability::ReadOnly { path });
        }
        "filesystem.read_write" => {
            caps.filesystem.push(FileSystemCapability::ReadWrite { path });
        }
        _ => {
            return Err(AlephError::InvalidConfig {
                message: format!("Unknown capability: {}", binding.capability),
                suggestion: Some("Use 'filesystem.read_only' or 'filesystem.read_write'".to_string()),
            })
        }
    }

    Ok(())
}

fn bind_array_parameter(
    caps: &mut Capabilities,
    binding: &ParameterBinding,
    value: &serde_json::Value,
) -> Result<()> {
    let array = value.as_array().ok_or_else(|| {
        AlephError::InvalidConfig {
            message: "Parameter value must be an array".to_string(),
            suggestion: Some("Provide an array of paths".to_string()),
        }
    })?;

    for element in array {
        bind_single_parameter(caps, binding, element)?;
    }

    Ok(())
}

fn validate_parameter(path: &str, rule: &ValidationRule) -> Result<()> {
    match rule {
        ValidationRule::IsFile => {
            let metadata = fs::metadata(path).map_err(|e| {
                AlephError::InvalidConfig {
                    message: format!("Path does not exist: {}", e),
                    suggestion: Some("Ensure the file exists".to_string()),
                }
            })?;
            if !metadata.is_file() {
                return Err(AlephError::InvalidConfig {
                    message: format!("Expected file, got directory: {}", path),
                    suggestion: Some("Provide a file path, not a directory".to_string()),
                });
            }
        }
        ValidationRule::IsDirectory => {
            let metadata = fs::metadata(path).map_err(|e| {
                AlephError::InvalidConfig {
                    message: format!("Path does not exist: {}", e),
                    suggestion: Some("Ensure the directory exists".to_string()),
                }
            })?;
            if !metadata.is_dir() {
                return Err(AlephError::InvalidConfig {
                    message: format!("Expected directory, got file: {}", path),
                    suggestion: Some("Provide a directory path, not a file".to_string()),
                });
            }
        }
        ValidationRule::IsPath => {
            // Just check if path exists
            if !std::path::Path::new(path).exists() {
                return Err(AlephError::InvalidConfig {
                    message: format!("Path does not exist: {}", path),
                    suggestion: Some("Ensure the path exists".to_string()),
                });
            }
        }
        ValidationRule::None => {
            // No validation
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::sandbox::presets::PresetRegistry;
    use crate::exec::sandbox::parameter_binding::CapabilityOverrides;

    #[test]
    fn test_apply_overrides_filesystem() {
        let registry = PresetRegistry::default();
        let preset = registry.get("file_processor").unwrap();
        let base_caps = preset.capabilities.clone();

        let overrides = CapabilityOverrides {
            filesystem: vec![FileSystemOverride {
                fs_type: "read_only".to_string(),
                path: "/tmp/logs".to_string(),
                reason: "Read log files".to_string(),
            }],
            ..Default::default()
        };

        let result = apply_overrides(base_caps, &overrides, &preset.immutable_fields);
        assert!(result.is_ok());
        let caps = result.unwrap();
        assert_eq!(caps.filesystem.len(), 2); // TempWorkspace + new ReadOnly
    }

    #[test]
    fn test_bind_parameters_single_file() {
        use std::io::Write;

        // Create a temporary file for testing
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();
        let temp_path = temp_file.path().to_str().unwrap().to_string();

        let mut caps = Capabilities::default();
        let mut bindings = HashMap::new();
        bindings.insert(
            "log_file".to_string(),
            ParameterBinding {
                capability: "filesystem.read_only".to_string(),
                validation: ValidationRule::IsFile,
                mapping: MappingType::Single,
            },
        );

        let params = serde_json::json!({
            "log_file": temp_path
        });

        let result = bind_parameters(&mut caps, &bindings, &params);
        assert!(result.is_ok());

        // Should have TempWorkspace (from default) + bound file
        assert!(caps.filesystem.len() >= 2);
        assert!(caps.filesystem.iter().any(|c| matches!(c, FileSystemCapability::ReadOnly { .. })));
    }
}
