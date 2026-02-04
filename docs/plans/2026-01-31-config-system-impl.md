# Config System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 Moltbot 风格的配置系统，支持 JSON Schema 生成、UI Hints 和热重载

**Architecture:** 为所有配置结构体添加 `JsonSchema` derive，创建 UI Hints 宏系统，实现 `config.schema` RPC，集成热重载

**Tech Stack:** Rust, schemars 0.8, serde, tokio, notify

---

## Task 1: Add JsonSchema to Core Config Structs

**Files:**
- Modify: `core/src/config/structs.rs`

**Step 1: Add import**

In `core/src/config/structs.rs`, add the schemars import at the top:

```rust
use schemars::JsonSchema;
```

**Step 2: Add JsonSchema derive to Config struct**

Find the `Config` struct and add `JsonSchema` to its derive:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    // ... existing fields
}
```

**Step 3: Add JsonSchema derive to FullConfig struct**

Find the `FullConfig` struct and add `JsonSchema` to its derive:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FullConfig {
    // ... existing fields
}
```

**Step 4: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`
Expected: Compilation succeeds (may have warnings about nested types needing JsonSchema)

**Step 5: Commit**

```bash
git add core/src/config/structs.rs
git commit -m "feat(config): add JsonSchema derive to Config structs"
```

---

## Task 2: Add JsonSchema to General Types

**Files:**
- Modify: `core/src/config/types/general.rs`

**Step 1: Add import**

```rust
use schemars::JsonSchema;
```

**Step 2: Add JsonSchema to all structs**

Add `JsonSchema` to derive for:
- `GeneralConfig`
- `ShortcutsConfig`
- `BehaviorConfig`

**Step 3: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check`

**Step 4: Commit**

```bash
git add core/src/config/types/general.rs
git commit -m "feat(config): add JsonSchema to general config types"
```

---

## Task 3: Add JsonSchema to Provider Types

**Files:**
- Modify: `core/src/config/types/provider.rs`

**Step 1: Add import and derives**

Add `use schemars::JsonSchema;` and `JsonSchema` derive to:
- `ProviderConfig`
- `ProviderConfigEntry`

**Step 2: Mark sensitive fields**

For `api_key` field, add schemars skip attribute:

```rust
#[schemars(skip)]
pub api_key: Option<String>,
```

**Step 3: Add range constraints**

```rust
#[schemars(range(min = 1, max = 300))]
pub timeout_seconds: u64,
```

**Step 4: Verify and commit**

```bash
cargo check
git add core/src/config/types/provider.rs
git commit -m "feat(config): add JsonSchema to provider types with sensitive field handling"
```

---

## Task 4: Add JsonSchema to Remaining Type Files

**Files:**
- Modify: `core/src/config/types/routing.rs`
- Modify: `core/src/config/types/memory.rs`
- Modify: `core/src/config/types/search.rs`
- Modify: `core/src/config/types/smart_flow.rs`
- Modify: `core/src/config/types/tools.rs`
- Modify: `core/src/config/types/video.rs`
- Modify: `core/src/config/types/skills.rs`
- Modify: `core/src/config/types/orchestrator.rs`

**Step 1: For each file, add import and derives**

Pattern for each file:
1. Add `use schemars::JsonSchema;`
2. Add `JsonSchema` to all struct/enum derives

**Step 2: Verify compilation after each file**

Run: `cargo check` after modifying each file

**Step 3: Commit all**

```bash
git add core/src/config/types/*.rs
git commit -m "feat(config): add JsonSchema to all top-level config types"
```

---

## Task 5: Add JsonSchema to Agent Submodule

**Files:**
- Modify: `core/src/config/types/agent/mod.rs`
- Modify: `core/src/config/types/agent/ab_testing.rs`
- Modify: `core/src/config/types/agent/model_routing.rs`
- Modify: `core/src/config/types/agent/ensemble.rs`
- Modify: `core/src/config/types/agent/metrics.rs`
- Modify: `core/src/config/types/agent/health.rs`
- Modify: `core/src/config/types/agent/subagents.rs`
- Modify: `core/src/config/types/agent/file_ops.rs`
- Modify: `core/src/config/types/agent/prompt_analysis.rs`
- Modify: `core/src/config/types/agent/model_profile.rs`
- Modify: `core/src/config/types/agent/semantic_cache.rs`
- Modify: `core/src/config/types/agent/code_exec.rs`

**Step 1: Add imports and derives to all files**

Same pattern as Task 4.

**Step 2: Verify and commit**

```bash
cargo check
git add core/src/config/types/agent/
git commit -m "feat(config): add JsonSchema to agent config types"
```

---

## Task 6: Add JsonSchema to Dispatcher Submodule

**Files:**
- Modify: `core/src/config/types/dispatcher/mod.rs`
- Modify: `core/src/config/types/dispatcher/core.rs`
- Modify: `core/src/config/types/dispatcher/budget.rs`
- Modify: `core/src/config/types/dispatcher/backoff.rs`
- Modify: `core/src/config/types/dispatcher/retry.rs`
- Modify: `core/src/config/types/dispatcher/model_router.rs`

**Step 1: Add imports and derives**

**Step 2: Verify and commit**

```bash
cargo check
git add core/src/config/types/dispatcher/
git commit -m "feat(config): add JsonSchema to dispatcher config types"
```

---

## Task 7: Add JsonSchema to Generation and Policies Submodules

**Files:**
- Modify: `core/src/config/types/generation/*.rs`
- Modify: `core/src/config/types/policies/*.rs`

**Step 1: Add imports and derives to all files**

**Step 2: Verify and commit**

```bash
cargo check
git add core/src/config/types/generation/ core/src/config/types/policies/
git commit -m "feat(config): add JsonSchema to generation and policies types"
```

---

## Task 8: Add JsonSchema to Extension Config Types

**Files:**
- Modify: `core/src/extension/config/types.rs`

**Step 1: Add import**

```rust
use schemars::JsonSchema;
```

**Step 2: Add JsonSchema to all types**

- `AlephConfig`
- `AgentConfigOverride`
- `McpConfig` (enum)
- `OAuthConfig`
- `ProviderOverride`
- `CompactionConfig`
- `ExperimentalConfig`
- `PermissionRule`

**Step 3: Mark sensitive fields**

```rust
// In OAuthConfig
#[schemars(skip)]
pub client_secret: Option<String>,

// In ProviderOverride
#[schemars(skip)]
pub api_key: Option<String>,
```

**Step 4: Verify and commit**

```bash
cargo check
git add core/src/extension/config/types.rs
git commit -m "feat(config): add JsonSchema to extension config types"
```

---

## Task 9: Create Schema Generation Module

**Files:**
- Create: `core/src/config/schema.rs`
- Modify: `core/src/config/mod.rs`

**Step 1: Create schema.rs**

```rust
//! JSON Schema generation for Aleph configuration.

use schemars::{schema::RootSchema, schema_for};
use crate::config::Config;

/// Generate JSON Schema for the main Config struct.
pub fn generate_config_schema() -> RootSchema {
    schema_for!(Config)
}

/// Generate JSON Schema as a serde_json::Value.
pub fn generate_config_schema_json() -> serde_json::Value {
    let schema = generate_config_schema();
    serde_json::to_value(schema).expect("Schema serialization should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_generation() {
        let schema = generate_config_schema();
        assert!(schema.schema.metadata.is_some());

        let json = generate_config_schema_json();
        assert!(json.is_object());
        assert!(json.get("$schema").is_some());
    }

    #[test]
    fn test_schema_has_definitions() {
        let schema = generate_config_schema();
        // Should have definitions for nested types
        assert!(!schema.definitions.is_empty());
    }
}
```

**Step 2: Export from mod.rs**

Add to `core/src/config/mod.rs`:

```rust
pub mod schema;
pub use schema::{generate_config_schema, generate_config_schema_json};
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test config::schema`
Expected: Tests pass

**Step 4: Commit**

```bash
git add core/src/config/schema.rs core/src/config/mod.rs
git commit -m "feat(config): add schema generation module"
```

---

## Task 10: Create UI Hints Types

**Files:**
- Create: `core/src/config/ui_hints/mod.rs`
- Modify: `core/src/config/mod.rs`

**Step 1: Create ui_hints directory and mod.rs**

```rust
//! UI Hints system for configuration field metadata.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata for a configuration group.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupMeta {
    /// Display label for the group.
    pub label: String,
    /// Sort order (lower = higher priority).
    pub order: i32,
    /// Optional icon identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Hint metadata for a single configuration field.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FieldHint {
    /// Human-readable label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Help text / tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Group this field belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Sort order within group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    /// Whether this is an advanced option (hidden by default).
    #[serde(default)]
    pub advanced: bool,
    /// Whether this field contains sensitive data.
    #[serde(default)]
    pub sensitive: bool,
    /// Placeholder text for input fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// Complete UI hints for configuration rendering.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ConfigUiHints {
    /// Group definitions: id -> metadata.
    pub groups: HashMap<String, GroupMeta>,
    /// Field hints: path -> hint.
    pub fields: HashMap<String, FieldHint>,
}

impl ConfigUiHints {
    /// Create empty UI hints.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get hint for a field path, supporting wildcard matching.
    pub fn get_hint(&self, path: &str) -> Option<&FieldHint> {
        // Try exact match first
        if let Some(hint) = self.fields.get(path) {
            return Some(hint);
        }

        // Try wildcard patterns (longest match first)
        let parts: Vec<&str> = path.split('.').collect();
        let mut best_match: Option<(&str, &FieldHint)> = None;

        for (pattern, hint) in &self.fields {
            if Self::matches_pattern(pattern, &parts) {
                if best_match.is_none() || pattern.len() > best_match.unwrap().0.len() {
                    best_match = Some((pattern.as_str(), hint));
                }
            }
        }

        best_match.map(|(_, hint)| hint)
    }

    /// Check if a pattern matches the path parts.
    fn matches_pattern(pattern: &str, path_parts: &[&str]) -> bool {
        let pattern_parts: Vec<&str> = pattern.split('.').collect();
        if pattern_parts.len() != path_parts.len() {
            return false;
        }

        pattern_parts
            .iter()
            .zip(path_parts.iter())
            .all(|(p, t)| *p == "*" || p == t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let mut hints = ConfigUiHints::new();
        hints.fields.insert(
            "general.language".to_string(),
            FieldHint {
                label: Some("Language".to_string()),
                ..Default::default()
            },
        );

        let hint = hints.get_hint("general.language");
        assert!(hint.is_some());
        assert_eq!(hint.unwrap().label, Some("Language".to_string()));
    }

    #[test]
    fn test_wildcard_match() {
        let mut hints = ConfigUiHints::new();
        hints.fields.insert(
            "providers.*.api_key".to_string(),
            FieldHint {
                label: Some("API Key".to_string()),
                sensitive: true,
                ..Default::default()
            },
        );

        let hint = hints.get_hint("providers.openai.api_key");
        assert!(hint.is_some());
        assert!(hint.unwrap().sensitive);

        let hint2 = hints.get_hint("providers.claude.api_key");
        assert!(hint2.is_some());
    }

    #[test]
    fn test_exact_beats_wildcard() {
        let mut hints = ConfigUiHints::new();
        hints.fields.insert(
            "providers.*.model".to_string(),
            FieldHint {
                label: Some("Model (generic)".to_string()),
                ..Default::default()
            },
        );
        hints.fields.insert(
            "providers.openai.model".to_string(),
            FieldHint {
                label: Some("Model (OpenAI)".to_string()),
                ..Default::default()
            },
        );

        let hint = hints.get_hint("providers.openai.model");
        assert_eq!(hint.unwrap().label, Some("Model (OpenAI)".to_string()));

        let hint2 = hints.get_hint("providers.claude.model");
        assert_eq!(hint2.unwrap().label, Some("Model (generic)".to_string()));
    }
}
```

**Step 2: Export from config/mod.rs**

```rust
pub mod ui_hints;
pub use ui_hints::{ConfigUiHints, FieldHint, GroupMeta};
```

**Step 3: Run tests**

Run: `cargo test config::ui_hints`
Expected: All tests pass

**Step 4: Commit**

```bash
mkdir -p core/src/config/ui_hints
git add core/src/config/ui_hints/mod.rs core/src/config/mod.rs
git commit -m "feat(config): add UI hints types with wildcard matching"
```

---

## Task 11: Create UI Hints Macros

**Files:**
- Create: `core/src/config/ui_hints/macros.rs`
- Modify: `core/src/config/ui_hints/mod.rs`

**Step 1: Create macros.rs**

```rust
//! Macros for defining UI hints declaratively.

/// Define configuration groups.
///
/// # Example
/// ```ignore
/// define_groups! {
///     "general" => { label: "General", order: 10, icon: "gear" },
///     "providers" => { label: "Providers", order: 20 },
/// }
/// ```
#[macro_export]
macro_rules! define_groups {
    (
        $( $id:literal => { label: $label:literal, order: $order:expr $(, icon: $icon:literal)? } ),* $(,)?
    ) => {
        {
            let mut groups = std::collections::HashMap::new();
            $(
                groups.insert(
                    $id.to_string(),
                    $crate::config::ui_hints::GroupMeta {
                        label: $label.to_string(),
                        order: $order,
                        icon: define_groups!(@icon $($icon)?),
                    },
                );
            )*
            groups
        }
    };
    (@icon $icon:literal) => { Some($icon.to_string()) };
    (@icon) => { None };
}

/// Define field hints.
///
/// # Example
/// ```ignore
/// define_hints! {
///     "general.language" => {
///         label: "Language",
///         help: "UI display language",
///         group: "general",
///         order: 1,
///     },
///     "providers.*.api_key" => {
///         label: "API Key",
///         sensitive: true,
///     },
/// }
/// ```
#[macro_export]
macro_rules! define_hints {
    (
        $( $path:literal => {
            $( label: $label:literal, )?
            $( help: $help:literal, )?
            $( group: $group:literal, )?
            $( order: $order:expr, )?
            $( advanced: $advanced:literal, )?
            $( sensitive: $sensitive:literal, )?
            $( placeholder: $placeholder:literal, )?
        } ),* $(,)?
    ) => {
        {
            let mut fields = std::collections::HashMap::new();
            $(
                fields.insert(
                    $path.to_string(),
                    $crate::config::ui_hints::FieldHint {
                        label: define_hints!(@opt $( $label )?),
                        help: define_hints!(@opt $( $help )?),
                        group: define_hints!(@opt $( $group )?),
                        order: define_hints!(@opt_num $( $order )?),
                        advanced: define_hints!(@bool $( $advanced )?),
                        sensitive: define_hints!(@bool $( $sensitive )?),
                        placeholder: define_hints!(@opt $( $placeholder )?),
                    },
                );
            )*
            fields
        }
    };
    (@opt $val:literal) => { Some($val.to_string()) };
    (@opt) => { None };
    (@opt_num $val:expr) => { Some($val) };
    (@opt_num) => { None };
    (@bool $val:literal) => { $val };
    (@bool) => { false };
}

pub use define_groups;
pub use define_hints;
```

**Step 2: Update ui_hints/mod.rs**

Add at the top:

```rust
#[macro_use]
pub mod macros;
pub use macros::{define_groups, define_hints};
```

**Step 3: Add macro test**

Add to `ui_hints/mod.rs` tests:

```rust
#[test]
fn test_define_groups_macro() {
    let groups = define_groups! {
        "general" => { label: "General", order: 10, icon: "gear" },
        "advanced" => { label: "Advanced", order: 100 },
    };

    assert_eq!(groups.len(), 2);
    assert_eq!(groups["general"].label, "General");
    assert_eq!(groups["general"].icon, Some("gear".to_string()));
    assert_eq!(groups["advanced"].icon, None);
}

#[test]
fn test_define_hints_macro() {
    let hints = define_hints! {
        "general.language" => {
            label: "Language",
            help: "UI language",
            group: "general",
            order: 1,
        },
        "providers.*.api_key" => {
            label: "API Key",
            sensitive: true,
        },
    };

    assert_eq!(hints.len(), 2);
    assert_eq!(hints["general.language"].label, Some("Language".to_string()));
    assert!(hints["providers.*.api_key"].sensitive);
}
```

**Step 4: Run tests and commit**

```bash
cargo test config::ui_hints
git add core/src/config/ui_hints/
git commit -m "feat(config): add declarative macros for UI hints"
```

---

## Task 12: Create UI Hints Definitions

**Files:**
- Create: `core/src/config/ui_hints/definitions.rs`
- Modify: `core/src/config/ui_hints/mod.rs`

**Step 1: Create definitions.rs with all hints**

```rust
//! Built-in UI hints definitions for Aleph configuration.

use super::{ConfigUiHints, FieldHint, GroupMeta};
use crate::{define_groups, define_hints};

/// Build the complete UI hints for Aleph configuration.
pub fn build_ui_hints() -> ConfigUiHints {
    ConfigUiHints {
        groups: build_groups(),
        fields: build_field_hints(),
    }
}

fn build_groups() -> std::collections::HashMap<String, GroupMeta> {
    define_groups! {
        "general" => { label: "General", order: 10, icon: "gear" },
        "providers" => { label: "AI Providers", order: 20, icon: "cloud" },
        "agents" => { label: "Agents", order: 30, icon: "robot" },
        "channels" => { label: "Channels", order: 40, icon: "chat" },
        "tools" => { label: "Tools", order: 50, icon: "wrench" },
        "memory" => { label: "Memory", order: 60, icon: "brain" },
        "search" => { label: "Search", order: 70, icon: "search" },
        "shortcuts" => { label: "Shortcuts", order: 80, icon: "keyboard" },
        "behavior" => { label: "Behavior", order: 90, icon: "sliders" },
        "advanced" => { label: "Advanced", order: 100, icon: "cog" },
    }
}

fn build_field_hints() -> std::collections::HashMap<String, FieldHint> {
    define_hints! {
        // === General ===
        "general.default_provider" => {
            label: "Default Provider",
            help: "AI provider used when no routing rule matches",
            group: "general",
            order: 1,
        },
        "general.language" => {
            label: "Language",
            help: "UI display language (en, zh-Hans)",
            group: "general",
            order: 2,
        },
        "general.output_dir" => {
            label: "Output Directory",
            help: "Directory for generated files",
            group: "general",
            order: 3,
            placeholder: "~/.aleph/output",
        },

        // === Providers (wildcard) ===
        "providers.*.api_key" => {
            label: "API Key",
            help: "API key for authentication",
            group: "providers",
            sensitive: true,
        },
        "providers.*.model" => {
            label: "Model",
            help: "Model identifier (e.g., gpt-4o, claude-opus-4-5)",
            group: "providers",
        },
        "providers.*.base_url" => {
            label: "Base URL",
            help: "Custom API endpoint URL",
            group: "providers",
            advanced: true,
        },
        "providers.*.timeout_seconds" => {
            label: "Timeout",
            help: "Request timeout in seconds (1-300)",
            group: "providers",
        },
        "providers.*.enabled" => {
            label: "Enabled",
            help: "Whether this provider is active",
            group: "providers",
        },
        "providers.*.temperature" => {
            label: "Temperature",
            help: "Sampling temperature (0.0-2.0)",
            group: "providers",
            advanced: true,
        },
        "providers.*.max_tokens" => {
            label: "Max Tokens",
            help: "Maximum tokens in response",
            group: "providers",
            advanced: true,
        },

        // === Memory ===
        "memory.enabled" => {
            label: "Enable Memory",
            help: "Enable semantic memory for context retrieval",
            group: "memory",
            order: 1,
        },
        "memory.max_context_items" => {
            label: "Max Context Items",
            help: "Maximum number of memory items to include",
            group: "memory",
            order: 2,
        },
        "memory.similarity_threshold" => {
            label: "Similarity Threshold",
            help: "Minimum similarity score for memory retrieval (0.0-1.0)",
            group: "memory",
            order: 3,
        },

        // === Shortcuts ===
        "shortcuts.summon" => {
            label: "Summon Shortcut",
            help: "Keyboard shortcut to summon Aleph",
            group: "shortcuts",
            placeholder: "Command+Grave",
        },
        "shortcuts.cancel" => {
            label: "Cancel Shortcut",
            help: "Keyboard shortcut to cancel current operation",
            group: "shortcuts",
            placeholder: "Escape",
        },
        "shortcuts.command_prompt" => {
            label: "Command Prompt",
            help: "Keyboard shortcut for command prompt",
            group: "shortcuts",
            placeholder: "Option+Space",
        },

        // === Behavior ===
        "behavior.output_mode" => {
            label: "Output Mode",
            help: "How to display AI responses (typewriter, instant)",
            group: "behavior",
        },
        "behavior.typing_speed" => {
            label: "Typing Speed",
            help: "Characters per second for typewriter mode (50-400)",
            group: "behavior",
        },

        // === Search ===
        "search.enabled" => {
            label: "Enable Search",
            help: "Enable web search capabilities",
            group: "search",
            order: 1,
        },
        "search.default_provider" => {
            label: "Search Provider",
            help: "Default search provider",
            group: "search",
            order: 2,
        },
        "search.max_results" => {
            label: "Max Results",
            help: "Maximum search results to return (1-100)",
            group: "search",
            order: 3,
        },

        // === Tools ===
        "tools.fs_enabled" => {
            label: "File System Access",
            help: "Enable file system tools",
            group: "tools",
        },
        "tools.git_enabled" => {
            label: "Git Access",
            help: "Enable Git tools",
            group: "tools",
        },
        "tools.shell_enabled" => {
            label: "Shell Access",
            help: "Enable shell command execution",
            group: "tools",
        },
        "tools.allowed_roots" => {
            label: "Allowed Directories",
            help: "Directories where file operations are permitted",
            group: "tools",
        },

        // === MCP ===
        "mcp.enabled" => {
            label: "Enable MCP",
            help: "Enable Model Context Protocol servers",
            group: "tools",
            advanced: true,
        },

        // === Agent ===
        "agent.require_confirmation" => {
            label: "Require Confirmation",
            help: "Require user confirmation for actions",
            group: "agents",
        },
        "agent.max_parallelism" => {
            label: "Max Parallelism",
            help: "Maximum concurrent agent operations",
            group: "agents",
            advanced: true,
        },

        // === Rules ===
        "rules.*.regex" => {
            label: "Pattern",
            help: "Regex pattern to match",
            group: "advanced",
        },
        "rules.*.provider" => {
            label: "Provider",
            help: "Provider to use when pattern matches",
            group: "advanced",
        },
        "rules.*.system_prompt" => {
            label: "System Prompt",
            help: "Custom system prompt for this rule",
            group: "advanced",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ui_hints() {
        let hints = build_ui_hints();

        // Check groups
        assert!(hints.groups.contains_key("general"));
        assert!(hints.groups.contains_key("providers"));
        assert!(hints.groups.contains_key("advanced"));

        // Check field hints
        assert!(hints.fields.contains_key("general.language"));
        assert!(hints.fields.contains_key("providers.*.api_key"));

        // Check sensitive field
        let api_key_hint = hints.fields.get("providers.*.api_key").unwrap();
        assert!(api_key_hint.sensitive);
    }

    #[test]
    fn test_wildcard_provider_hints() {
        let hints = build_ui_hints();

        // Test wildcard matching for providers
        let hint = hints.get_hint("providers.openai.api_key");
        assert!(hint.is_some());
        assert!(hint.unwrap().sensitive);

        let hint2 = hints.get_hint("providers.claude.model");
        assert!(hint2.is_some());
        assert_eq!(hint2.unwrap().group, Some("providers".to_string()));
    }
}
```

**Step 2: Export from ui_hints/mod.rs**

```rust
pub mod definitions;
pub use definitions::build_ui_hints;
```

**Step 3: Run tests and commit**

```bash
cargo test config::ui_hints
git add core/src/config/ui_hints/definitions.rs core/src/config/ui_hints/mod.rs
git commit -m "feat(config): add UI hints definitions for all config fields"
```

---

## Task 13: Implement config.schema RPC Handler

**Files:**
- Modify: `core/src/gateway/handlers/config.rs`

**Step 1: Read existing config.rs to understand handler pattern**

**Step 2: Add schema handler types and implementation**

Add to `config.rs`:

```rust
use crate::config::{generate_config_schema_json, build_ui_hints, ConfigUiHints};

/// Request params for config.schema
#[derive(Debug, Deserialize)]
pub struct ConfigSchemaRequest {
    #[serde(default = "default_true")]
    pub include_plugins: bool,
}

fn default_true() -> bool {
    true
}

/// Response for config.schema
#[derive(Debug, Serialize)]
pub struct ConfigSchemaResponse {
    pub schema: serde_json::Value,
    pub ui_hints: ConfigUiHints,
    pub version: String,
    pub generated_at: String,
}

/// Handle config.schema RPC request.
pub async fn handle_config_schema(
    _req: ConfigSchemaRequest,
) -> Result<ConfigSchemaResponse, crate::gateway::RpcError> {
    let schema = generate_config_schema_json();
    let ui_hints = build_ui_hints();

    Ok(ConfigSchemaResponse {
        schema,
        ui_hints,
        version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    })
}
```

**Step 3: Register handler in handlers/mod.rs**

Find where handlers are registered and add:

```rust
registry.register("config.schema", handle_config_schema);
```

**Step 4: Verify compilation**

```bash
cargo check
```

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/config.rs core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): add config.schema RPC handler"
```

---

## Task 14: Create Config Diff Module

**Files:**
- Create: `core/src/config/diff.rs`
- Modify: `core/src/config/mod.rs`

**Step 1: Create diff.rs**

```rust
//! Configuration diff detection for hot reload.

use serde_json::Value;

/// Compare two configs and return changed paths.
pub fn diff_config<T: serde::Serialize>(prev: &T, next: &T) -> Vec<String> {
    let prev_value = serde_json::to_value(prev).unwrap_or(Value::Null);
    let next_value = serde_json::to_value(next).unwrap_or(Value::Null);

    let mut changes = Vec::new();
    diff_values(&prev_value, &next_value, "", &mut changes);
    changes
}

fn diff_values(prev: &Value, next: &Value, prefix: &str, changes: &mut Vec<String>) {
    match (prev, next) {
        (Value::Object(prev_map), Value::Object(next_map)) => {
            // Check for removed/changed keys
            for (key, prev_val) in prev_map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };

                match next_map.get(key) {
                    Some(next_val) => {
                        diff_values(prev_val, next_val, &path, changes);
                    }
                    None => {
                        changes.push(path);
                    }
                }
            }

            // Check for added keys
            for key in next_map.keys() {
                if !prev_map.contains_key(key) {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", prefix, key)
                    };
                    changes.push(path);
                }
            }
        }
        (Value::Array(prev_arr), Value::Array(next_arr)) => {
            if prev_arr != next_arr {
                changes.push(prefix.to_string());
            }
        }
        _ => {
            if prev != next {
                changes.push(prefix.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct TestConfig {
        name: String,
        value: i32,
        nested: NestedConfig,
    }

    #[derive(Serialize, Deserialize)]
    struct NestedConfig {
        enabled: bool,
        items: Vec<String>,
    }

    #[test]
    fn test_no_changes() {
        let config = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec!["a".to_string()],
            },
        };

        let changes = diff_config(&config, &config);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_simple_change() {
        let prev = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };

        let next = TestConfig {
            name: "test".to_string(),
            value: 100,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };

        let changes = diff_config(&prev, &next);
        assert_eq!(changes, vec!["value"]);
    }

    #[test]
    fn test_nested_change() {
        let prev = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };

        let next = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: false,
                items: vec![],
            },
        };

        let changes = diff_config(&prev, &next);
        assert_eq!(changes, vec!["nested.enabled"]);
    }

    #[test]
    fn test_array_change() {
        let prev = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec!["a".to_string()],
            },
        };

        let next = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec!["a".to_string(), "b".to_string()],
            },
        };

        let changes = diff_config(&prev, &next);
        assert_eq!(changes, vec!["nested.items"]);
    }
}
```

**Step 2: Export from mod.rs**

```rust
pub mod diff;
pub use diff::diff_config;
```

**Step 3: Run tests and commit**

```bash
cargo test config::diff
git add core/src/config/diff.rs core/src/config/mod.rs
git commit -m "feat(config): add config diff detection module"
```

---

## Task 15: Create Reload Plan Module

**Files:**
- Create: `core/src/config/reload.rs`
- Modify: `core/src/config/mod.rs`

**Step 1: Create reload.rs**

```rust
//! Hot reload planning based on config changes.

use std::collections::HashSet;

/// Plan for handling configuration changes.
#[derive(Debug, Clone, Default)]
pub struct ReloadPlan {
    /// Requires full Gateway restart.
    pub restart_gateway: bool,
    /// Channels that need restart.
    pub restart_channels: HashSet<String>,
    /// Whether to reload hooks.
    pub reload_hooks: bool,
    /// Whether to restart cron.
    pub restart_cron: bool,
    /// Paths that can be hot-updated without restart.
    pub hot_paths: Vec<String>,
}

impl ReloadPlan {
    /// Check if any restart is required.
    pub fn requires_restart(&self) -> bool {
        self.restart_gateway || !self.restart_channels.is_empty()
    }

    /// Check if the plan is empty (no changes).
    pub fn is_empty(&self) -> bool {
        !self.restart_gateway
            && self.restart_channels.is_empty()
            && !self.reload_hooks
            && !self.restart_cron
            && self.hot_paths.is_empty()
    }
}

/// Build a reload plan from changed configuration paths.
pub fn build_reload_plan(changed_paths: &[String]) -> ReloadPlan {
    let mut plan = ReloadPlan::default();

    for path in changed_paths {
        classify_change(path, &mut plan);
    }

    plan
}

fn classify_change(path: &str, plan: &mut ReloadPlan) {
    // Gateway changes require full restart
    if path.starts_with("gateway.") {
        plan.restart_gateway = true;
        return;
    }

    // Plugin changes require full restart
    if path.starts_with("plugins") || path.starts_with("mcp.") {
        plan.restart_gateway = true;
        return;
    }

    // Channel changes restart specific channels
    if path.starts_with("channels.") {
        if let Some(channel) = path.strip_prefix("channels.") {
            let channel_name = channel.split('.').next().unwrap_or(channel);
            plan.restart_channels.insert(channel_name.to_string());
        }
        return;
    }

    // Hook changes
    if path.starts_with("hooks") {
        plan.reload_hooks = true;
        return;
    }

    // Cron changes
    if path.starts_with("cron") {
        plan.restart_cron = true;
        return;
    }

    // Everything else is hot-updatable
    plan.hot_paths.push(path.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_changes() {
        let plan = build_reload_plan(&[]);
        assert!(plan.is_empty());
        assert!(!plan.requires_restart());
    }

    #[test]
    fn test_gateway_restart() {
        let plan = build_reload_plan(&["gateway.port".to_string()]);
        assert!(plan.restart_gateway);
        assert!(plan.requires_restart());
    }

    #[test]
    fn test_channel_restart() {
        let plan = build_reload_plan(&[
            "channels.telegram.token".to_string(),
            "channels.discord.guild_id".to_string(),
        ]);

        assert!(!plan.restart_gateway);
        assert!(plan.restart_channels.contains("telegram"));
        assert!(plan.restart_channels.contains("discord"));
        assert!(plan.requires_restart());
    }

    #[test]
    fn test_hot_update() {
        let plan = build_reload_plan(&[
            "providers.openai.model".to_string(),
            "general.language".to_string(),
        ]);

        assert!(!plan.restart_gateway);
        assert!(plan.restart_channels.is_empty());
        assert!(!plan.requires_restart());
        assert_eq!(plan.hot_paths.len(), 2);
    }

    #[test]
    fn test_hooks_reload() {
        let plan = build_reload_plan(&["hooks.email.enabled".to_string()]);
        assert!(plan.reload_hooks);
        assert!(!plan.restart_gateway);
    }

    #[test]
    fn test_cron_restart() {
        let plan = build_reload_plan(&["cron.jobs".to_string()]);
        assert!(plan.restart_cron);
        assert!(!plan.restart_gateway);
    }

    #[test]
    fn test_mcp_requires_restart() {
        let plan = build_reload_plan(&["mcp.servers".to_string()]);
        assert!(plan.restart_gateway);
    }
}
```

**Step 2: Export from mod.rs**

```rust
pub mod reload;
pub use reload::{build_reload_plan, ReloadPlan};
```

**Step 3: Run tests and commit**

```bash
cargo test config::reload
git add core/src/config/reload.rs core/src/config/mod.rs
git commit -m "feat(config): add reload plan generation module"
```

---

## Task 16: Create Extension Config Loader with TOML Support

**Files:**
- Create: `core/src/extension/config/loader.rs`
- Modify: `core/src/extension/config/mod.rs`

**Step 1: Create loader.rs**

```rust
//! Unified config loader supporting TOML and JSONC.

use std::path::{Path, PathBuf};
use crate::error::AlephError;
use super::types::AlephConfig;

/// Config file priority order.
const CONFIG_FILES: &[&str] = &["aleph.toml", "aleph.jsonc", "aleph.json"];

/// Find the config file in a directory.
pub fn find_config_file(dir: &Path) -> Option<PathBuf> {
    for filename in CONFIG_FILES {
        let path = dir.join(filename);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Load extension config from a directory.
pub fn load_extension_config(dir: &Path) -> Result<Option<AlephConfig>, AlephError> {
    let Some(path) = find_config_file(dir) else {
        return Ok(None);
    };

    load_config_file(&path).map(Some)
}

/// Load config from a specific file.
pub fn load_config_file(path: &Path) -> Result<AlephConfig, AlephError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        AlephError::InvalidConfig(format!("Failed to read {}: {}", path.display(), e))
    })?;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match ext {
        "toml" => parse_toml(&content, path),
        "jsonc" | "json" => parse_jsonc(&content, path),
        _ => Err(AlephError::InvalidConfig(format!(
            "Unknown config file extension: {}",
            path.display()
        ))),
    }
}

fn parse_toml(content: &str, path: &Path) -> Result<AlephConfig, AlephError> {
    toml::from_str(content).map_err(|e| {
        AlephError::InvalidConfig(format!("Failed to parse {}: {}", path.display(), e))
    })
}

fn parse_jsonc(content: &str, path: &Path) -> Result<AlephConfig, AlephError> {
    // Strip comments for JSONC
    let stripped = strip_json_comments(content);

    serde_json::from_str(&stripped).map_err(|e| {
        AlephError::InvalidConfig(format!("Failed to parse {}: {}", path.display(), e))
    })
}

/// Strip single-line and multi-line comments from JSON.
fn strip_json_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        if c == '\\' && in_string {
            result.push(c);
            escape_next = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            result.push(c);
            continue;
        }

        if in_string {
            result.push(c);
            continue;
        }

        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    // Single-line comment
                    chars.next();
                    while let Some(&nc) = chars.peek() {
                        if nc == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                Some('*') => {
                    // Multi-line comment
                    chars.next();
                    while let Some(nc) = chars.next() {
                        if nc == '*' && chars.peek() == Some(&'/') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => result.push(c),
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_find_config_toml_priority() {
        let dir = tempdir().unwrap();

        // Create both files
        fs::write(dir.path().join("aleph.toml"), "").unwrap();
        fs::write(dir.path().join("aleph.jsonc"), "{}").unwrap();

        // TOML should have priority
        let found = find_config_file(dir.path()).unwrap();
        assert!(found.to_string_lossy().ends_with("aleph.toml"));
    }

    #[test]
    fn test_strip_comments() {
        let input = r#"{
            // single line comment
            "key": "value", /* inline comment */
            "key2": "value2"
        }"#;

        let stripped = strip_json_comments(input);
        assert!(!stripped.contains("//"));
        assert!(!stripped.contains("/*"));
        assert!(stripped.contains("\"key\""));
    }

    #[test]
    fn test_parse_toml_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("aleph.toml");

        fs::write(&path, r#"
            model = "claude-opus-4-5"
            plugins = ["npm:@test/plugin"]
        "#).unwrap();

        let config = load_config_file(&path).unwrap();
        assert_eq!(config.model, Some("claude-opus-4-5".to_string()));
    }

    #[test]
    fn test_parse_jsonc_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("aleph.jsonc");

        fs::write(&path, r#"{
            // This is a comment
            "model": "gpt-4o"
        }"#).unwrap();

        let config = load_config_file(&path).unwrap();
        assert_eq!(config.model, Some("gpt-4o".to_string()));
    }
}
```

**Step 2: Update extension/config/mod.rs**

Add:
```rust
pub mod loader;
pub use loader::{find_config_file, load_extension_config, load_config_file};
```

**Step 3: Run tests and commit**

```bash
cargo test extension::config::loader
git add core/src/extension/config/loader.rs core/src/extension/config/mod.rs
git commit -m "feat(extension): add unified config loader with TOML priority"
```

---

## Task 17: Create JSONC to TOML Migration Tool

**Files:**
- Create: `core/src/extension/config/migrate.rs`
- Modify: `core/src/extension/config/mod.rs`

**Step 1: Create migrate.rs**

```rust
//! Migration tool for converting JSONC configs to TOML.

use std::path::{Path, PathBuf};
use crate::error::AlephError;
use super::loader::load_config_file;

/// Migration result.
#[derive(Debug)]
pub struct MigrationResult {
    pub source: PathBuf,
    pub target: PathBuf,
    pub backup: Option<PathBuf>,
}

/// Migrate a JSONC config file to TOML format.
pub fn migrate_to_toml(jsonc_path: &Path) -> Result<MigrationResult, AlephError> {
    // Validate source exists and is JSONC
    if !jsonc_path.exists() {
        return Err(AlephError::InvalidConfig(format!(
            "Source file not found: {}",
            jsonc_path.display()
        )));
    }

    let ext = jsonc_path.extension().and_then(|e| e.to_str());
    if ext != Some("jsonc") && ext != Some("json") {
        return Err(AlephError::InvalidConfig(
            "Source must be a .jsonc or .json file".to_string(),
        ));
    }

    // Load and parse the JSONC config
    let config = load_config_file(jsonc_path)?;

    // Serialize to TOML
    let toml_content = toml::to_string_pretty(&config).map_err(|e| {
        AlephError::InvalidConfig(format!("Failed to serialize to TOML: {}", e))
    })?;

    // Add header comment
    let toml_with_header = format!(
        "# Aleph Extension Configuration\n\
         # Migrated from {}\n\n{}",
        jsonc_path.file_name().unwrap_or_default().to_string_lossy(),
        toml_content
    );

    // Determine target path
    let target_path = jsonc_path.with_extension("toml");
    let target_path = if target_path.file_name().map(|f| f.to_string_lossy()) == Some("aleph.toml".into()) {
        target_path
    } else {
        jsonc_path.parent().unwrap_or(Path::new(".")).join("aleph.toml")
    };

    // Backup original if target exists
    let backup_path = if target_path.exists() {
        let backup = target_path.with_extension("toml.bak");
        std::fs::rename(&target_path, &backup).map_err(|e| {
            AlephError::InvalidConfig(format!("Failed to backup existing file: {}", e))
        })?;
        Some(backup)
    } else {
        None
    };

    // Write new TOML file
    std::fs::write(&target_path, toml_with_header).map_err(|e| {
        AlephError::InvalidConfig(format!("Failed to write TOML file: {}", e))
    })?;

    Ok(MigrationResult {
        source: jsonc_path.to_path_buf(),
        target: target_path,
        backup: backup_path,
    })
}

/// Check if a directory needs migration.
pub fn needs_migration(dir: &Path) -> bool {
    let toml_exists = dir.join("aleph.toml").exists();
    let jsonc_exists = dir.join("aleph.jsonc").exists() || dir.join("aleph.json").exists();

    !toml_exists && jsonc_exists
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_migrate_jsonc_to_toml() {
        let dir = tempdir().unwrap();
        let jsonc_path = dir.path().join("aleph.jsonc");

        fs::write(&jsonc_path, r#"{
            "model": "claude-opus-4-5",
            "plugins": ["npm:@test/plugin"]
        }"#).unwrap();

        let result = migrate_to_toml(&jsonc_path).unwrap();

        assert!(result.target.exists());
        assert_eq!(result.target.file_name().unwrap(), "aleph.toml");

        let content = fs::read_to_string(&result.target).unwrap();
        assert!(content.contains("model = \"claude-opus-4-5\""));
        assert!(content.contains("# Migrated from"));
    }

    #[test]
    fn test_needs_migration() {
        let dir = tempdir().unwrap();

        // No files - no migration
        assert!(!needs_migration(dir.path()));

        // Only JSONC - needs migration
        fs::write(dir.path().join("aleph.jsonc"), "{}").unwrap();
        assert!(needs_migration(dir.path()));

        // Both exist - no migration (TOML takes priority)
        fs::write(dir.path().join("aleph.toml"), "").unwrap();
        assert!(!needs_migration(dir.path()));
    }
}
```

**Step 2: Update extension/config/mod.rs**

```rust
pub mod migrate;
pub use migrate::{migrate_to_toml, needs_migration, MigrationResult};
```

**Step 3: Run tests and commit**

```bash
cargo test extension::config::migrate
git add core/src/extension/config/migrate.rs core/src/extension/config/mod.rs
git commit -m "feat(extension): add JSONC to TOML migration tool"
```

---

## Task 18: Integration Test

**Files:**
- Create: `core/src/config/tests/schema_integration.rs`

**Step 1: Create integration test**

```rust
//! Integration tests for the config schema system.

use crate::config::{
    Config, generate_config_schema, generate_config_schema_json,
    build_ui_hints, diff_config, build_reload_plan,
};

#[test]
fn test_full_schema_generation() {
    let schema = generate_config_schema();

    // Check schema metadata
    assert!(schema.schema.metadata.is_some());

    // Check definitions exist for nested types
    assert!(!schema.definitions.is_empty());

    // Verify JSON serialization
    let json = generate_config_schema_json();
    assert!(json.is_object());
    assert!(json.get("$schema").is_some());
    assert!(json.get("definitions").is_some());
}

#[test]
fn test_ui_hints_coverage() {
    let hints = build_ui_hints();

    // Check all groups are defined
    assert!(hints.groups.len() >= 6);

    // Check critical fields have hints
    assert!(hints.get_hint("general.default_provider").is_some());
    assert!(hints.get_hint("providers.openai.api_key").is_some());
    assert!(hints.get_hint("memory.enabled").is_some());

    // Check sensitive fields are marked
    let api_key_hint = hints.get_hint("providers.claude.api_key").unwrap();
    assert!(api_key_hint.sensitive);
}

#[test]
fn test_config_diff_and_reload_plan() {
    let mut prev = Config::default();
    let mut next = prev.clone();

    // Modify a provider
    // (Note: actual modification depends on Config structure)

    let changes = diff_config(&prev, &next);
    let plan = build_reload_plan(&changes);

    // Empty changes should produce empty plan
    assert!(plan.is_empty());
}

#[test]
fn test_schema_and_hints_consistency() {
    let schema = generate_config_schema_json();
    let hints = build_ui_hints();

    // For each field hint, verify the path could exist in schema
    // (This is a basic sanity check)
    for (path, _hint) in &hints.fields {
        // Wildcard paths are valid
        if path.contains('*') {
            continue;
        }

        // Check path structure is valid (dot-separated)
        assert!(!path.is_empty());
        assert!(!path.starts_with('.'));
        assert!(!path.ends_with('.'));
    }
}
```

**Step 2: Add test module to config/mod.rs**

```rust
#[cfg(test)]
mod tests;
```

**Step 3: Run all tests**

```bash
cargo test config::
```

**Step 4: Commit**

```bash
git add core/src/config/tests/
git commit -m "test(config): add schema system integration tests"
```

---

## Task 19: Final Verification

**Step 1: Run full test suite**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test
```

**Step 2: Build with all features**

```bash
cargo build --all-features
```

**Step 3: Verify schema output**

Create a quick test binary or add a test that prints the schema:

```bash
cargo test test_full_schema_generation -- --nocapture
```

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat(config): complete config schema system implementation

- JSON Schema generation via schemars
- UI Hints with wildcard matching
- Config diff detection
- Reload plan generation
- TOML extension config support
- JSONC to TOML migration

Closes #config-system-design"
```

---

## Success Criteria Checklist

- [ ] All config structs have `#[derive(JsonSchema)]`
- [ ] `generate_config_schema()` produces valid JSON Schema Draft-07
- [ ] UI Hints cover all user-facing fields with wildcard support
- [ ] `config.schema` RPC handler is registered and functional
- [ ] Config diff detects changes accurately
- [ ] Reload plan classifies changes correctly
- [ ] Extension config loader supports TOML with JSONC fallback
- [ ] Migration tool converts JSONC to TOML
- [ ] All tests pass
- [ ] Build succeeds with all features
