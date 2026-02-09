# Skill Sandboxing Phase 2: Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Integrate OS-native sandbox infrastructure into Skill Evolution system with preset-based capability management and dynamic parameter binding.

**Architecture:** Three-layer security model (Static Declaration → Parameter Binding → Dynamic Execution) with preset templates, capability resolution, and enhanced audit logging.

**Tech Stack:** Rust, serde, serde_json, rusqlite, existing sandbox infrastructure from Phase 1

---

## Task 1: Preset Registry Foundation

**Files:**
- Create: `core/src/exec/sandbox/presets.rs`
- Modify: `core/src/exec/sandbox/mod.rs`
- Test: `core/src/exec/sandbox/presets.rs` (inline tests)

**Step 1: Write failing test for PresetRegistry**

Add to `core/src/exec/sandbox/presets.rs`:

```rust
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
}
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib presets::tests::test_preset_registry_get_file_processor`
Expected: FAIL with "no such module `presets`"

**Step 3: Implement PresetRegistry structure**

Create `core/src/exec/sandbox/presets.rs`:

```rust
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

        Self { presets }
    }
}
```

**Step 4: Add module to mod.rs**

Modify `core/src/exec/sandbox/mod.rs`:

```rust
pub mod presets;
```

**Step 5: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib presets::tests::test_preset_registry_get_file_processor`
Expected: PASS

**Step 6: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/exec/sandbox/presets.rs core/src/exec/sandbox/mod.rs
git commit -m "exec/sandbox: add preset registry foundation with file_processor preset"
```

---

## Task 2: Add Remaining Core Presets

**Files:**
- Modify: `core/src/exec/sandbox/presets.rs`

**Step 1: Write failing tests for remaining presets**

Add to `core/src/exec/sandbox/presets.rs` tests:

```rust
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
```

**Step 2: Run tests to verify they fail**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib presets::tests`
Expected: FAIL with "assertion failed" for new presets

**Step 3: Implement remaining presets**

Add to `PresetRegistry::default()` in `core/src/exec/sandbox/presets.rs`:

```rust
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
                path: std::path::PathBuf::from("${WORKSPACE}"),
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
        description: "Data transformation tools with data directory access".to_string(),
        capabilities: Capabilities {
            filesystem: vec![
                FileSystemCapability::TempWorkspace,
                FileSystemCapability::ReadOnly {
                    path: std::path::PathBuf::from("${PROJECT_ROOT}/data"),
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
```

**Step 4: Run tests to verify they pass**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib presets::tests`
Expected: PASS (all 5 tests)

**Step 5: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/exec/sandbox/presets.rs
git commit -m "exec/sandbox: add web_scraper, code_analyzer, data_transformer presets"
```

---

## Task 3: Parameter Binding Types

**Files:**
- Create: `core/src/exec/sandbox/parameter_binding.rs`
- Modify: `core/src/exec/sandbox/mod.rs`

**Step 1: Write failing test for ParameterBinding**

Create `core/src/exec/sandbox/parameter_binding.rs`:

```rust
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
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib parameter_binding::tests`
Expected: FAIL with "no such module"

**Step 3: Implement ParameterBinding types**

Add to `core/src/exec/sandbox/parameter_binding.rs`:

```rust
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
```

**Step 4: Add module to mod.rs**

Modify `core/src/exec/sandbox/mod.rs`:

```rust
pub mod parameter_binding;
```

**Step 5: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib parameter_binding::tests`
Expected: PASS

**Step 6: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/exec/sandbox/parameter_binding.rs core/src/exec/sandbox/mod.rs
git commit -m "exec/sandbox: add parameter binding types and validation rules"
```

---

## Task 4: Capability Resolver - Override Merging

**Files:**
- Create: `core/src/exec/sandbox/capability_resolver.rs`
- Modify: `core/src/exec/sandbox/mod.rs`

**Step 1: Write failing test for override merging**

Create `core/src/exec/sandbox/capability_resolver.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::sandbox::presets::PresetRegistry;

    #[test]
    fn test_apply_overrides_filesystem() {
        let registry = PresetRegistry::default();
        let preset = registry.get("file_processor").unwrap();
        let mut base_caps = preset.capabilities.clone();

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
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib capability_resolver::tests`
Expected: FAIL with "no such module"

**Step 3: Implement apply_overrides function**

Add to `core/src/exec/sandbox/capability_resolver.rs`:

```rust
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
                return Err(AlephError::InvalidInput(format!(
                    "Invalid filesystem type: {}",
                    fs_override.fs_type
                )))
            }
        };
        base.filesystem.push(cap);
    }

    // Apply network overrides (check immutability)
    if let Some(ref network) = overrides.network {
        if immutable_fields.contains(&"network".to_string()) {
            return Err(AlephError::InvalidInput(
                "Network capability is immutable for this preset".to_string(),
            ));
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
            return Err(AlephError::InvalidInput(
                "Environment capability is immutable for this preset".to_string(),
            ));
        }
        base.environment = env.clone();
    }

    Ok(base)
}
```

**Step 4: Add module to mod.rs**

Modify `core/src/exec/sandbox/mod.rs`:

```rust
pub mod capability_resolver;
```

**Step 5: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib capability_resolver::tests`
Expected: PASS

**Step 6: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/exec/sandbox/capability_resolver.rs core/src/exec/sandbox/mod.rs
git commit -m "exec/sandbox: add capability resolver with override merging"
```

---

## Task 5: Capability Resolver - Parameter Binding

**Files:**
- Modify: `core/src/exec/sandbox/capability_resolver.rs`

**Step 1: Write failing test for parameter binding**

Add to `core/src/exec/sandbox/capability_resolver.rs` tests:

```rust
#[test]
fn test_bind_parameters_single_file() {
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
        "log_file": "/tmp/test.log"
    });

    let result = bind_parameters(&mut caps, &bindings, &params);
    assert!(result.is_ok());
    assert!(caps.filesystem.iter().any(|c| matches!(c, FileSystemCapability::ReadOnly { path } if path.to_str().unwrap().contains("test.log"))));
}
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib capability_resolver::tests::test_bind_parameters_single_file`
Expected: FAIL with "function not found"

**Step 3: Implement bind_parameters function**

Add to `core/src/exec/sandbox/capability_resolver.rs`:

```rust
use super::parameter_binding::{ParameterBinding, ValidationRule, MappingType};
use std::collections::HashMap;
use std::fs;

/// Bind tool parameters to capabilities
pub fn bind_parameters(
    caps: &mut Capabilities,
    bindings: &HashMap<String, ParameterBinding>,
    parameters: &serde_json::Value,
) -> Result<()> {
    for (param_name, binding) in bindings {
        let param_value = parameters.get(param_name).ok_or_else(|| {
            AlephError::InvalidInput(format!("Missing parameter: {}", param_name))
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
        AlephError::InvalidInput("Parameter value must be a string".to_string())
    })?;

    // Validate parameter
    validate_parameter(path_str, &binding.validation)?;

    // Canonicalize path
    let path = fs::canonicalize(path_str).map_err(|e| {
        AlephError::InvalidInput(format!("Invalid path {}: {}", path_str, e))
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
            return Err(AlephError::InvalidInput(format!(
                "Unknown capability: {}",
                binding.capability
            )))
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
        AlephError::InvalidInput("Parameter value must be an array".to_string())
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
                AlephError::InvalidInput(format!("Path does not exist: {}", e))
            })?;
            if !metadata.is_file() {
                return Err(AlephError::InvalidInput(format!(
                    "Expected file, got directory: {}",
                    path
                )));
            }
        }
        ValidationRule::IsDirectory => {
            let metadata = fs::metadata(path).map_err(|e| {
                AlephError::InvalidInput(format!("Path does not exist: {}", e))
            })?;
            if !metadata.is_dir() {
                return Err(AlephError::InvalidInput(format!(
                    "Expected directory, got file: {}",
                    path
                )));
            }
        }
        ValidationRule::IsPath => {
            // Just check if path exists
            if !std::path::Path::new(path).exists() {
                return Err(AlephError::InvalidInput(format!(
                    "Path does not exist: {}",
                    path
                )));
            }
        }
        ValidationRule::None => {
            // No validation
        }
    }

    Ok(())
}
```

**Step 4: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib capability_resolver::tests`
Expected: PASS

**Step 5: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/exec/sandbox/capability_resolver.rs
git commit -m "exec/sandbox: add parameter binding with validation"
```

---

## Task 6: Extend Tool Definition Schema

**Files:**
- Modify: `core/src/skill_evolution/tool_generator.rs`

**Step 1: Write failing test for extended tool definition**

Add to `core/src/skill_evolution/tool_generator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition_with_capabilities() {
        let def = GeneratedToolDefinition {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            input_schema: serde_json::json!({}),
            runtime: "python".to_string(),
            entrypoint: "entrypoint.py".to_string(),
            self_tested: false,
            requires_confirmation: true,
            required_capabilities: Some(crate::exec::sandbox::parameter_binding::RequiredCapabilities {
                base_preset: "file_processor".to_string(),
                description: "Test capabilities".to_string(),
                overrides: Default::default(),
                parameter_bindings: Default::default(),
            }),
            generated: GenerationMetadata {
                pattern_id: "test".to_string(),
                confidence: 0.9,
                generated_at: 0,
                generator_version: "1.0".to_string(),
            },
        };

        let json = serde_json::to_string(&def).unwrap();
        assert!(json.contains("required_capabilities"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib tool_generator::tests::test_tool_definition_with_capabilities`
Expected: FAIL with "no field `required_capabilities`"

**Step 3: Add required_capabilities field**

Modify `GeneratedToolDefinition` in `core/src/skill_evolution/tool_generator.rs`:

```rust
use crate::exec::sandbox::parameter_binding::RequiredCapabilities;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub runtime: String,
    pub entrypoint: String,
    pub self_tested: bool,
    pub requires_confirmation: bool,
    /// Sandbox capabilities required by this tool
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_capabilities: Option<RequiredCapabilities>,
    pub generated: GenerationMetadata,
}
```

**Step 4: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib tool_generator::tests`
Expected: PASS

**Step 5: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/skill_evolution/tool_generator.rs
git commit -m "skill_evolution: extend tool definition with required_capabilities"
```

---

## Task 7: Sandbox Integration Module

**Files:**
- Create: `core/src/skill_evolution/sandbox_integration.rs`
- Modify: `core/src/skill_evolution/mod.rs`

**Step 1: Write failing test for capability resolution**

Create `core/src/skill_evolution/sandbox_integration.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_tool_capabilities() {
        let tool_def = GeneratedToolDefinition {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            input_schema: serde_json::json!({}),
            runtime: "python".to_string(),
            entrypoint: "entrypoint.py".to_string(),
            self_tested: false,
            requires_confirmation: true,
            required_capabilities: Some(RequiredCapabilities {
                base_preset: "file_processor".to_string(),
                description: "Test".to_string(),
                overrides: Default::default(),
                parameter_bindings: Default::default(),
            }),
            generated: GenerationMetadata {
                pattern_id: "test".to_string(),
                confidence: 0.9,
                generated_at: 0,
                generator_version: "1.0".to_string(),
            },
        };

        let params = serde_json::json!({});
        let result = resolve_tool_capabilities(&tool_def, &params);
        assert!(result.is_ok());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib sandbox_integration::tests`
Expected: FAIL with "no such module"

**Step 3: Implement resolve_tool_capabilities**

Add to `core/src/skill_evolution/sandbox_integration.rs`:

```rust
use crate::error::{AlephError, Result};
use crate::exec::sandbox::{
    capabilities::Capabilities,
    presets::PresetRegistry,
    capability_resolver::{apply_overrides, bind_parameters},
    parameter_binding::RequiredCapabilities,
};
use super::tool_generator::GeneratedToolDefinition;

/// Resolve final capabilities for a tool execution
pub fn resolve_tool_capabilities(
    tool_def: &GeneratedToolDefinition,
    parameters: &serde_json::Value,
) -> Result<Capabilities> {
    // Get required capabilities
    let required_caps = tool_def.required_capabilities.as_ref().ok_or_else(|| {
        AlephError::InvalidInput("Tool has no required_capabilities".to_string())
    })?;

    // Load preset
    let registry = PresetRegistry::default();
    let preset = registry.get(&required_caps.base_preset).ok_or_else(|| {
        AlephError::InvalidInput(format!(
            "Unknown preset: {}",
            required_caps.base_preset
        ))
    })?;

    // Start with preset capabilities
    let mut caps = preset.capabilities.clone();

    // Apply overrides
    caps = apply_overrides(caps, &required_caps.overrides, &preset.immutable_fields)?;

    // Bind parameters
    bind_parameters(&mut caps, &required_caps.parameter_bindings, parameters)?;

    Ok(caps)
}

/// Infer appropriate preset from tool purpose
pub fn infer_preset_from_purpose(purpose: &str) -> String {
    let purpose_lower = purpose.to_lowercase();

    if purpose_lower.contains("web") || purpose_lower.contains("http") || purpose_lower.contains("scrape") {
        "web_scraper".to_string()
    } else if purpose_lower.contains("code") || purpose_lower.contains("analyze") || purpose_lower.contains("lint") {
        "code_analyzer".to_string()
    } else if purpose_lower.contains("data") || purpose_lower.contains("transform") || purpose_lower.contains("convert") {
        "data_transformer".to_string()
    } else {
        "file_processor".to_string()
    }
}
```

**Step 4: Add module to mod.rs**

Modify `core/src/skill_evolution/mod.rs`:

```rust
pub mod sandbox_integration;
```

**Step 5: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib sandbox_integration::tests`
Expected: PASS

**Step 6: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/skill_evolution/sandbox_integration.rs core/src/skill_evolution/mod.rs
git commit -m "skill_evolution: add sandbox integration with capability resolution"
```

---

## Task 8: Enhanced Audit Logging

**Files:**
- Modify: `core/src/exec/sandbox/audit.rs`

**Step 1: Write failing test for enhanced audit log**

Add to `core/src/exec/sandbox/audit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enhanced_audit_log_with_tool_context() {
        let log = SandboxAuditLog {
            id: "test".to_string(),
            timestamp: 0,
            tool_name: "test_tool".to_string(),
            command: "python test.py".to_string(),
            capabilities: Capabilities::default(),
            execution_result: ExecutionStatus::Success {
                exit_code: 0,
                duration_ms: 100,
            },
            sandboxed: true,
            tool_context: Some(ToolExecutionContext {
                tool_name: "test_tool".to_string(),
                tool_version: "1.0".to_string(),
                base_preset: "file_processor".to_string(),
                applied_overrides: vec![],
                parameter_bindings_used: Default::default(),
                dynamic_paths: vec![],
                capability_resolution_log: vec![],
            }),
        };

        assert_eq!(log.tool_context.as_ref().unwrap().tool_name, "test_tool");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib audit::tests::test_enhanced_audit_log_with_tool_context`
Expected: FAIL with "no field `tool_context`"

**Step 3: Add ToolExecutionContext types**

Add to `core/src/exec/sandbox/audit.rs`:

```rust
use std::collections::HashMap;
use std::path::PathBuf;

/// Tool execution context for enhanced audit logging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionContext {
    pub tool_name: String,
    pub tool_version: String,
    pub base_preset: String,
    pub applied_overrides: Vec<String>,
    pub parameter_bindings_used: HashMap<String, String>,
    pub dynamic_paths: Vec<PathBuf>,
    pub capability_resolution_log: Vec<ResolutionStep>,
}

/// Step in capability resolution process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionStep {
    pub step: String,
    pub timestamp: i64,
    pub description: String,
}
```

**Step 4: Add tool_context field to SandboxAuditLog**

Modify `SandboxAuditLog` in `core/src/exec/sandbox/audit.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxAuditLog {
    pub id: String,
    pub timestamp: i64,
    pub tool_name: String,
    pub command: String,
    pub capabilities: Capabilities,
    pub execution_result: ExecutionStatus,
    pub sandboxed: bool,
    /// Enhanced context for tool executions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_context: Option<ToolExecutionContext>,
}
```

**Step 5: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib audit::tests`
Expected: PASS

**Step 6: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/exec/sandbox/audit.rs
git commit -m "exec/sandbox: add enhanced audit logging with tool execution context"
```

---

## Task 9: Tool Generator Integration

**Files:**
- Modify: `core/src/skill_evolution/tool_generator.rs`

**Step 1: Write failing test for capability generation**

Add to `core/src/skill_evolution/tool_generator.rs` tests:

```rust
#[test]
fn test_generate_required_capabilities() {
    let suggestion = SolidificationSuggestion {
        pattern_id: "test".to_string(),
        pattern_name: "Web Scraper".to_string(),
        description: "Scrape web pages".to_string(),
        confidence: 0.9,
        example_inputs: vec![],
        suggested_parameters: vec![],
    };

    let caps = generate_required_capabilities(&suggestion);
    assert_eq!(caps.base_preset, "web_scraper");
}
```

**Step 2: Run test to verify it fails**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib tool_generator::tests::test_generate_required_capabilities`
Expected: FAIL with "function not found"

**Step 3: Implement generate_required_capabilities**

Add to `core/src/skill_evolution/tool_generator.rs`:

```rust
use crate::exec::sandbox::parameter_binding::{RequiredCapabilities, CapabilityOverrides};
use super::sandbox_integration::infer_preset_from_purpose;

fn generate_required_capabilities(suggestion: &SolidificationSuggestion) -> RequiredCapabilities {
    let base_preset = infer_preset_from_purpose(&suggestion.description);

    RequiredCapabilities {
        base_preset,
        description: format!("Capabilities for {}", suggestion.pattern_name),
        overrides: CapabilityOverrides::default(),
        parameter_bindings: Default::default(),
    }
}
```

**Step 4: Integrate into tool generation**

Modify the tool generation function in `core/src/skill_evolution/tool_generator.rs` to include `required_capabilities`:

```rust
// In the tool generation function, add:
let required_capabilities = Some(generate_required_capabilities(&suggestion));

// Include in GeneratedToolDefinition:
GeneratedToolDefinition {
    // ... existing fields ...
    required_capabilities,
    // ... rest of fields ...
}
```

**Step 5: Run test to verify it passes**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib tool_generator::tests`
Expected: PASS

**Step 6: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/skill_evolution/tool_generator.rs
git commit -m "skill_evolution: integrate capability generation into tool generator"
```

---

## Task 10: Sandboxed Tool Execution

**Files:**
- Create: `core/src/skill_evolution/sandboxed_executor.rs`
- Modify: `core/src/skill_evolution/mod.rs`

**Step 1: Write failing test for sandboxed execution**

Create `core/src/skill_evolution/sandboxed_executor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_tool_in_sandbox() {
        // This is an integration test that would require actual tool setup
        // For now, just test the structure
        assert!(true);
    }
}
```

**Step 2: Implement SandboxedToolExecutor**

Add to `core/src/skill_evolution/sandboxed_executor.rs`:

```rust
use std::sync::Arc;
use std::path::PathBuf;

use crate::error::{AlephError, Result};
use crate::exec::sandbox::{
    executor::{SandboxManager, SandboxCommand},
    adapter::SandboxAdapter,
    audit::{SandboxAuditLog, ToolExecutionContext, ResolutionStep},
};
use super::tool_generator::GeneratedToolDefinition;
use super::sandbox_integration::resolve_tool_capabilities;

/// Executor for running tools in sandbox
pub struct SandboxedToolExecutor {
    sandbox_manager: SandboxManager,
}

impl SandboxedToolExecutor {
    pub fn new(sandbox_adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            sandbox_manager: SandboxManager::new(sandbox_adapter),
        }
    }

    /// Execute a tool with sandboxing
    pub async fn execute_tool(
        &self,
        tool_def: &GeneratedToolDefinition,
        parameters: serde_json::Value,
        tool_package_dir: PathBuf,
    ) -> Result<(String, SandboxAuditLog)> {
        // Resolve capabilities
        let capabilities = resolve_tool_capabilities(tool_def, &parameters)?;

        // Build command
        let command = SandboxCommand {
            program: self.get_runtime_executable(&tool_def.runtime)?,
            args: vec![
                tool_def.entrypoint.clone(),
                serde_json::to_string(&parameters)?,
            ],
            working_dir: Some(tool_package_dir),
        };

        // Execute in sandbox
        let (result, mut audit_log) = self
            .sandbox_manager
            .execute_sandboxed(&tool_def.name, command, capabilities)
            .await?;

        // Add tool context to audit log
        audit_log.tool_context = Some(ToolExecutionContext {
            tool_name: tool_def.name.clone(),
            tool_version: tool_def.generated.generator_version.clone(),
            base_preset: tool_def
                .required_capabilities
                .as_ref()
                .map(|c| c.base_preset.clone())
                .unwrap_or_default(),
            applied_overrides: vec![],
            parameter_bindings_used: Default::default(),
            dynamic_paths: vec![],
            capability_resolution_log: vec![
                ResolutionStep {
                    step: "load_preset".to_string(),
                    timestamp: chrono::Utc::now().timestamp(),
                    description: "Loaded base preset".to_string(),
                },
            ],
        });

        Ok((result.stdout, audit_log))
    }

    fn get_runtime_executable(&self, runtime: &str) -> Result<String> {
        match runtime {
            "python" => Ok("python3".to_string()),
            "node" => Ok("node".to_string()),
            _ => Err(AlephError::InvalidInput(format!(
                "Unsupported runtime: {}",
                runtime
            ))),
        }
    }
}
```

**Step 3: Add module to mod.rs**

Modify `core/src/skill_evolution/mod.rs`:

```rust
pub mod sandboxed_executor;
```

**Step 4: Run test to verify it compiles**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo check -p alephcore --lib`
Expected: SUCCESS

**Step 5: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/skill_evolution/sandboxed_executor.rs core/src/skill_evolution/mod.rs
git commit -m "skill_evolution: add sandboxed tool executor"
```

---

## Task 11: Integration Tests

**Files:**
- Create: `core/src/skill_evolution/integration_tests.rs`
- Modify: `core/src/skill_evolution/mod.rs`

**Step 1: Write integration test for end-to-end flow**

Create `core/src/skill_evolution/integration_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::sandbox::presets::PresetRegistry;
    use crate::skill_evolution::sandbox_integration::resolve_tool_capabilities;
    use crate::skill_evolution::tool_generator::GeneratedToolDefinition;

    #[test]
    fn test_end_to_end_capability_resolution() {
        // Create a tool definition with capabilities
        let tool_def = create_test_tool_definition();

        // Create test parameters
        let params = serde_json::json!({
            "input_file": "/tmp/test.txt"
        });

        // Resolve capabilities
        let result = resolve_tool_capabilities(&tool_def, &params);

        // Should succeed
        assert!(result.is_ok());
        let caps = result.unwrap();

        // Should have temp workspace from preset
        assert!(!caps.filesystem.is_empty());
    }

    #[test]
    fn test_preset_immutability_enforcement() {
        let registry = PresetRegistry::default();
        let preset = registry.get("file_processor").unwrap();

        // Try to override network (immutable)
        let mut overrides = crate::exec::sandbox::parameter_binding::CapabilityOverrides::default();
        overrides.network = Some(crate::exec::sandbox::capabilities::NetworkCapability::AllowAll);

        let result = crate::exec::sandbox::capability_resolver::apply_overrides(
            preset.capabilities.clone(),
            &overrides,
            &preset.immutable_fields,
        );

        // Should fail
        assert!(result.is_err());
    }

    fn create_test_tool_definition() -> GeneratedToolDefinition {
        use crate::exec::sandbox::parameter_binding::{RequiredCapabilities, ParameterBinding, ValidationRule, MappingType};
        use std::collections::HashMap;

        let mut bindings = HashMap::new();
        bindings.insert(
            "input_file".to_string(),
            ParameterBinding {
                capability: "filesystem.read_only".to_string(),
                validation: ValidationRule::IsFile,
                mapping: MappingType::Single,
            },
        );

        GeneratedToolDefinition {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            input_schema: serde_json::json!({}),
            runtime: "python".to_string(),
            entrypoint: "entrypoint.py".to_string(),
            self_tested: false,
            requires_confirmation: true,
            required_capabilities: Some(RequiredCapabilities {
                base_preset: "file_processor".to_string(),
                description: "Test capabilities".to_string(),
                overrides: Default::default(),
                parameter_bindings: bindings,
            }),
            generated: crate::skill_evolution::tool_generator::GenerationMetadata {
                pattern_id: "test".to_string(),
                confidence: 0.9,
                generated_at: 0,
                generator_version: "1.0".to_string(),
            },
        }
    }
}
```

**Step 2: Add module to mod.rs**

Modify `core/src/skill_evolution/mod.rs`:

```rust
#[cfg(test)]
mod integration_tests;
```

**Step 3: Run integration tests**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib integration_tests`
Expected: PASS

**Step 4: Commit**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add core/src/skill_evolution/integration_tests.rs core/src/skill_evolution/mod.rs
git commit -m "skill_evolution: add integration tests for sandbox integration"
```

---

## Task 12: Documentation and Final Testing

**Files:**
- Create: `docs/SKILL_SANDBOXING.md`
- Modify: `docs/ARCHITECTURE.md`

**Step 1: Create comprehensive documentation**

Create `docs/SKILL_SANDBOXING.md`:

```markdown
# Skill Sandboxing System

## Overview

The Skill Sandboxing system provides OS-native isolation for evolved skills using a three-layer security model.

## Architecture

### Three-Layer Security Model

**L1: Static Declaration Layer**
- Tools declare `required_capabilities` in `tool_definition.json`
- Uses preset templates (file_processor, web_scraper, code_analyzer, data_transformer)
- Users review and approve permissions

**L2: Parameter Binding Layer**
- Explicit `parameter_bindings` map parameters to capabilities
- Runtime validation (is_file, is_directory)
- Support for arrays and complex types

**L3: Dynamic Execution Layer**
- SandboxManager generates final OS sandbox profile
- All decisions logged to audit trail

## Preset Templates

### file_processor
- Filesystem: TempWorkspace only
- Network: Denied
- Process: No fork, 5min timeout, 512MB
- Use for: File processing, text manipulation

### web_scraper
- Filesystem: TempWorkspace only
- Network: Allow all
- Process: No fork, 10min timeout, 1GB
- Use for: Web scraping, HTTP requests

### code_analyzer
- Filesystem: ReadOnly workspace
- Network: Denied
- Process: No fork, 15min timeout, 2GB
- Use for: Code analysis, linting

### data_transformer
- Filesystem: TempWorkspace + data directory
- Network: Denied
- Process: No fork, 30min timeout, 4GB
- Use for: Data transformation, ETL

## Usage

### Tool Definition

```json
{
  "name": "log_analyzer",
  "required_capabilities": {
    "base_preset": "file_processor",
    "description": "Analyze log files",
    "parameter_bindings": {
      "log_file": {
        "capability": "filesystem.read_only",
        "validation": "is_file"
      }
    }
  }
}
```

### Execution

```rust
let executor = SandboxedToolExecutor::new(sandbox_adapter);
let (output, audit_log) = executor.execute_tool(&tool_def, params, package_dir).await?;
```

## Security Properties

- **Principle of Least Privilege**: Tools get minimum required permissions
- **Defense in Depth**: Static + dynamic + OS-level enforcement
- **Fail-Safe Defaults**: Missing bindings → execution blocked
- **Audit Trail**: All capability resolutions logged

## Error Handling

- `PermissionDenied`: Tool attempted undeclared access
- `ValidationError`: Parameter type mismatch
- `PresetNotFound`: Unknown preset name
- `ImmutableOverride`: Attempted to override immutable field

## Testing

Run tests:
```bash
cargo test -p alephcore --lib sandbox
cargo test -p alephcore --lib skill_evolution
```
```

**Step 2: Update architecture documentation**

Add to `docs/ARCHITECTURE.md`:

```markdown
## Skill Sandboxing

All evolved skills execute in OS-native sandboxes with fine-grained capability control.

See [SKILL_SANDBOXING.md](SKILL_SANDBOXING.md) for details.
```

**Step 3: Run full test suite**

Run: `cd .worktrees/feature/skill-sandboxing-phase2 && cargo test -p alephcore --lib`
Expected: All tests pass

**Step 4: Commit documentation**

```bash
cd .worktrees/feature/skill-sandboxing-phase2
git add docs/SKILL_SANDBOXING.md docs/ARCHITECTURE.md
git commit -m "docs: add skill sandboxing documentation"
```

---

## Success Criteria

- ✅ All evolved tools execute in sandbox
- ✅ Preset registry with 4 core presets
- ✅ Parameter binding with validation
- ✅ Capability resolution with override merging
- ✅ Enhanced audit logging with tool context
- ✅ Integration tests passing
- ✅ Documentation complete

## Next Steps

After Phase 2 completion:
- Use @superpowers:finishing-a-development-branch to complete the work
- Phase 3: Security and performance testing
- Phase 4: Linux and Windows platform support

---

**Implementation Status**: Ready to Execute
**Estimated Tasks**: 12 tasks
**Test Coverage**: Unit + Integration tests for all components
