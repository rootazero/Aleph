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

