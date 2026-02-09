use crate::error::{AlephError, Result};
use super::capabilities::{Capabilities, FileSystemCapability};
use super::parameter_binding::{CapabilityOverrides, FileSystemOverride};
use std::path::PathBuf;

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
}
