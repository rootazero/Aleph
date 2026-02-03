# Aether Extension SDK V2 - P0 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement P0 capabilities of Extension SDK V2: TOML manifest support, typed Hook system (Interceptor/Observer/Resolver), and unified Prompt scope (system/tool/standalone).

**Architecture:** Add TOML parser alongside existing JSON parser with priority (TOML > JSON). Extend Hook system with `HookKind` classification. Add Prompt scope to manifest and integrate with skill injection.

**Tech Stack:** Rust, toml crate, serde, schemars (JSON Schema)

---

## Phase 1: TOML Manifest Parser

### Task 1.1: Add TOML Dependency

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add toml crate**

```toml
# In [dependencies] section, add:
toml = "0.8"
```

**Step 2: Verify compilation**

Run: `cd core && cargo check`
Expected: Compiles without errors

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "build(core): add toml crate for TOML manifest support"
```

---

### Task 1.2: Define TOML Manifest Types

**Files:**
- Create: `core/src/extension/manifest/aether_plugin_toml.rs`

**Step 1: Create the TOML types file**

```rust
//! TOML-based plugin manifest parser (aether_plugin.toml)
//!
//! This is the V2 manifest format, preferred over JSON.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::fs;

use super::types::{
    ConfigSchema, ConfigUiHints, PluginAuthor, PluginKind, PluginManifest, PluginPermission,
};
use super::{sanitize_plugin_id, validate_plugin_id};
use crate::AetherError;

/// Root structure for aether_plugin.toml
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AetherPluginToml {
    pub plugin: PluginSection,

    #[serde(default)]
    pub permissions: PermissionsSection,

    #[serde(default)]
    pub prompt: Option<PromptSection>,

    #[serde(default, rename = "tools")]
    pub tools: Vec<ToolSection>,

    #[serde(default, rename = "hooks")]
    pub hooks: Vec<HookSection>,

    #[serde(default, rename = "commands")]
    pub commands: Vec<CommandSection>,

    #[serde(default, rename = "services")]
    pub services: Vec<ServiceSection>,

    #[serde(default)]
    pub capabilities: Option<CapabilitiesSection>,
}

/// [plugin] section - core metadata
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginSection {
    pub id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub kind: Option<String>,
    pub entry: Option<String>,
    pub author: Option<PluginAuthorToml>,

    #[serde(default)]
    pub config_schema: Option<ConfigSchema>,

    #[serde(default)]
    pub config_ui_hints: Option<ConfigUiHints>,
}

/// Plugin author information
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginAuthorToml {
    pub name: Option<String>,
    pub email: Option<String>,
    pub url: Option<String>,
}

/// [permissions] section - security boundary
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PermissionsSection {
    /// Network permissions: ["connect:https://*", "connect:postgres://*"]
    #[serde(default)]
    pub network: Vec<String>,

    /// Filesystem permissions: ["read:./data", "write:/tmp/cache"]
    #[serde(default)]
    pub filesystem: Vec<String>,

    /// Environment variable access: ["DATABASE_URL", "PG_*"]
    #[serde(default)]
    pub env: Vec<String>,

    /// Shell execution permission
    #[serde(default)]
    pub shell: bool,
}

/// [prompt] section - global prompt injection
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PromptSection {
    /// Path to prompt file (relative to plugin root)
    pub file: String,

    /// Scope: "system" (always inject) or "disabled"
    #[serde(default = "default_prompt_scope")]
    pub scope: String,
}

fn default_prompt_scope() -> String {
    "system".to_string()
}

/// [[tools]] section - static tool declarations
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolSection {
    pub name: String,
    pub description: String,
    pub handler: String,

    /// Optional: path to instruction file (tool-scoped prompt)
    pub instruction_file: Option<String>,

    /// JSON Schema for parameters
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

/// [[hooks]] section - typed hook declarations
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HookSection {
    pub event: String,

    /// Hook kind: "interceptor", "observer", "resolver"
    #[serde(default = "default_hook_kind")]
    pub kind: String,

    pub handler: String,

    /// Priority: "system", "high", "normal", "low"
    #[serde(default = "default_hook_priority")]
    pub priority: String,

    /// Optional filter pattern (for tool-related events)
    pub filter: Option<String>,
}

fn default_hook_kind() -> String {
    "observer".to_string()
}

fn default_hook_priority() -> String {
    "normal".to_string()
}

/// [[commands]] section - direct commands (bypass LLM)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandSection {
    pub name: String,
    pub description: Option<String>,

    /// Code handler (mutually exclusive with prompt_file)
    pub handler: Option<String>,

    /// Prompt template file (mutually exclusive with handler)
    pub prompt_file: Option<String>,
}

/// [[services]] section - background services
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServiceSection {
    pub name: String,
    pub description: Option<String>,
    pub start_handler: String,
    pub stop_handler: String,
}

/// [capabilities] section - dynamic capability declarations
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CapabilitiesSection {
    /// Allow api.registerTool() at runtime
    #[serde(default)]
    pub dynamic_tools: bool,

    /// Allowed dynamic hook events: ["after_*"]
    #[serde(default)]
    pub dynamic_hooks: Vec<String>,
}

/// Parse aether_plugin.toml from a directory
pub async fn parse_aether_plugin_toml(dir: &Path) -> Result<PluginManifest, AetherError> {
    let toml_path = dir.join("aether_plugin.toml");
    let content = fs::read_to_string(&toml_path).await.map_err(|e| {
        AetherError::PluginLoad(format!(
            "Failed to read aether_plugin.toml at {}: {}",
            toml_path.display(),
            e
        ))
    })?;

    parse_aether_plugin_toml_content(&content, dir)
}

/// Parse aether_plugin.toml content synchronously
pub fn parse_aether_plugin_toml_sync(dir: &Path) -> Result<PluginManifest, AetherError> {
    let toml_path = dir.join("aether_plugin.toml");
    let content = std::fs::read_to_string(&toml_path).map_err(|e| {
        AetherError::PluginLoad(format!(
            "Failed to read aether_plugin.toml at {}: {}",
            toml_path.display(),
            e
        ))
    })?;

    parse_aether_plugin_toml_content(&content, dir)
}

/// Parse TOML content into PluginManifest
pub fn parse_aether_plugin_toml_content(
    content: &str,
    plugin_dir: &Path,
) -> Result<PluginManifest, AetherError> {
    let toml: AetherPluginToml = toml::from_str(content).map_err(|e| {
        AetherError::PluginLoad(format!("Failed to parse aether_plugin.toml: {}", e))
    })?;

    // Validate or sanitize plugin ID
    let id = if validate_plugin_id(&toml.plugin.id).is_ok() {
        toml.plugin.id.clone()
    } else {
        sanitize_plugin_id(&toml.plugin.id)
    };

    // Parse plugin kind
    let kind = match toml.plugin.kind.as_deref() {
        Some("nodejs") | Some("node") => PluginKind::NodeJs,
        Some("wasm") => PluginKind::Wasm,
        Some("static") | None => PluginKind::Static,
        Some(k) => {
            return Err(AetherError::PluginLoad(format!(
                "Unknown plugin kind: {}",
                k
            )))
        }
    };

    // Convert permissions
    let permissions = convert_permissions(&toml.permissions);

    // Convert author
    let author = toml.plugin.author.map(|a| PluginAuthor {
        name: a.name,
        email: a.email,
        url: a.url,
    });

    // Build manifest
    let manifest = PluginManifest {
        id,
        name: toml.plugin.name,
        version: toml.plugin.version,
        description: toml.plugin.description,
        kind,
        entry: toml.plugin.entry,
        config_schema: toml.plugin.config_schema,
        config_ui_hints: toml.plugin.config_ui_hints,
        permissions,
        author,
        homepage: None,
        repository: None,
        license: None,
        keywords: None,
        extensions: None,
        source_path: Some(plugin_dir.to_path_buf()),
        // V2 extensions
        tools_v2: Some(toml.tools),
        hooks_v2: Some(toml.hooks),
        commands_v2: Some(toml.commands),
        services_v2: Some(toml.services),
        prompt_v2: toml.prompt,
        capabilities_v2: toml.capabilities,
    };

    Ok(manifest)
}

/// Convert TOML permissions to PluginPermission list
fn convert_permissions(perms: &PermissionsSection) -> Vec<PluginPermission> {
    let mut result = Vec::new();

    // Network permissions
    for net in &perms.network {
        result.push(PluginPermission::Network(net.clone()));
    }

    // Filesystem permissions
    for fs in &perms.filesystem {
        if fs.starts_with("read:") {
            result.push(PluginPermission::FilesystemRead(
                fs.strip_prefix("read:").unwrap_or(fs).to_string(),
            ));
        } else if fs.starts_with("write:") {
            result.push(PluginPermission::FilesystemWrite(
                fs.strip_prefix("write:").unwrap_or(fs).to_string(),
            ));
        }
    }

    // Env permissions
    for env in &perms.env {
        result.push(PluginPermission::Env(env.clone()));
    }

    // Shell permission
    if perms.shell {
        result.push(PluginPermission::Custom("shell".to_string()));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_toml() {
        let content = r#"
[plugin]
id = "test-plugin"
"#;
        let manifest = parse_aether_plugin_toml_content(content, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.id, "test-plugin");
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_parse_full_toml() {
        let content = r#"
[plugin]
id = "com.example.sql-explorer"
name = "SQL Explorer"
version = "2.0.0"
kind = "nodejs"
entry = "dist/index.js"

[permissions]
network = ["connect:postgres://*"]
filesystem = ["read:./data"]
env = ["DATABASE_URL"]

[prompt]
file = "SKILL.md"
scope = "system"

[[tools]]
name = "query_sql"
description = "Execute SQL"
handler = "handleQuerySql"

[[hooks]]
event = "before_tool_call"
kind = "interceptor"
handler = "onBeforeToolCall"
priority = "high"
"#;
        let manifest = parse_aether_plugin_toml_content(content, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.id, "com.example.sql-explorer");
        assert_eq!(manifest.kind, PluginKind::NodeJs);
        assert_eq!(manifest.permissions.len(), 3);
        assert!(manifest.tools_v2.as_ref().unwrap().len() == 1);
        assert!(manifest.hooks_v2.as_ref().unwrap().len() == 1);
    }
}
```

**Step 2: Run tests**

Run: `cd core && cargo test aether_plugin_toml`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/extension/manifest/aether_plugin_toml.rs
git commit -m "feat(extension): add TOML manifest parser types"
```

---

### Task 1.3: Extend PluginManifest for V2 Fields

**Files:**
- Modify: `core/src/extension/manifest/types.rs`

**Step 1: Add V2 fields to PluginManifest**

Add these fields to the `PluginManifest` struct (around line 270, before the closing brace):

```rust
    // ═══════════════════════════════════════════
    // V2 Extension fields (from aether_plugin.toml)
    // ═══════════════════════════════════════════

    /// V2: Static tool declarations from TOML
    #[serde(skip)]
    pub tools_v2: Option<Vec<crate::extension::manifest::aether_plugin_toml::ToolSection>>,

    /// V2: Typed hook declarations from TOML
    #[serde(skip)]
    pub hooks_v2: Option<Vec<crate::extension::manifest::aether_plugin_toml::HookSection>>,

    /// V2: Direct command declarations from TOML
    #[serde(skip)]
    pub commands_v2: Option<Vec<crate::extension::manifest::aether_plugin_toml::CommandSection>>,

    /// V2: Background service declarations from TOML
    #[serde(skip)]
    pub services_v2: Option<Vec<crate::extension::manifest::aether_plugin_toml::ServiceSection>>,

    /// V2: Global prompt configuration
    #[serde(skip)]
    pub prompt_v2: Option<crate::extension::manifest::aether_plugin_toml::PromptSection>,

    /// V2: Dynamic capability declarations
    #[serde(skip)]
    pub capabilities_v2: Option<crate::extension::manifest::aether_plugin_toml::CapabilitiesSection>,
```

**Step 2: Update Default impl if needed**

Ensure these fields default to `None` in any existing Default implementations.

**Step 3: Run tests**

Run: `cd core && cargo test`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/extension/manifest/types.rs
git commit -m "feat(extension): add V2 fields to PluginManifest"
```

---

### Task 1.4: Register TOML Module and Update Auto-Detection

**Files:**
- Modify: `core/src/extension/manifest/mod.rs`

**Step 1: Add module declaration**

At the top of the file (around line 10, with other mod declarations), add:

```rust
mod aether_plugin_toml;
pub use aether_plugin_toml::{
    parse_aether_plugin_toml, parse_aether_plugin_toml_content, parse_aether_plugin_toml_sync,
    AetherPluginToml, CapabilitiesSection, CommandSection, HookSection, PermissionsSection,
    PromptSection, ServiceSection, ToolSection,
};
```

**Step 2: Update parse_manifest_from_dir() to check TOML first**

Find `parse_manifest_from_dir()` (around line 200) and update the detection logic:

```rust
pub async fn parse_manifest_from_dir(dir: &Path) -> Result<PluginManifest, AetherError> {
    // V2: TOML format (preferred)
    let toml_path = dir.join("aether_plugin.toml");
    if toml_path.exists() {
        return parse_aether_plugin_toml(dir).await;
    }

    // V1: JSON format
    let json_path = dir.join("aether.plugin.json");
    if json_path.exists() {
        return parse_aether_plugin(dir).await;
    }

    // Legacy: package.json with "aether" field
    let package_json_path = dir.join("package.json");
    if package_json_path.exists() {
        return parse_package_json(dir).await;
    }

    // Legacy: .claude-plugin/plugin.json
    let legacy_path = dir.join(".claude-plugin").join("plugin.json");
    if legacy_path.exists() {
        return parse_aether_plugin(&dir.join(".claude-plugin")).await;
    }

    Err(AetherError::PluginLoad(format!(
        "No plugin manifest found in {}. Expected aether_plugin.toml, aether.plugin.json, or package.json with 'aether' field.",
        dir.display()
    )))
}
```

**Step 3: Update parse_manifest_from_dir_sync() similarly**

Find `parse_manifest_from_dir_sync()` (around line 244) and apply the same TOML-first logic:

```rust
pub fn parse_manifest_from_dir_sync(dir: &Path) -> Result<PluginManifest, AetherError> {
    // V2: TOML format (preferred)
    let toml_path = dir.join("aether_plugin.toml");
    if toml_path.exists() {
        return parse_aether_plugin_toml_sync(dir);
    }

    // V1: JSON format
    let json_path = dir.join("aether.plugin.json");
    if json_path.exists() {
        return parse_aether_plugin_sync(dir);
    }

    // Legacy: package.json with "aether" field
    let package_json_path = dir.join("package.json");
    if package_json_path.exists() {
        return parse_package_json_sync(dir);
    }

    // Legacy: .claude-plugin/plugin.json
    let legacy_path = dir.join(".claude-plugin").join("plugin.json");
    if legacy_path.exists() {
        return parse_aether_plugin_sync(&dir.join(".claude-plugin"));
    }

    Err(AetherError::PluginLoad(format!(
        "No plugin manifest found in {}. Expected aether_plugin.toml, aether.plugin.json, or package.json with 'aether' field.",
        dir.display()
    )))
}
```

**Step 4: Run tests**

Run: `cd core && cargo test manifest`
Expected: All tests pass

**Step 5: Commit**

```bash
git add core/src/extension/manifest/mod.rs
git commit -m "feat(extension): integrate TOML parser with auto-detection (TOML > JSON)"
```

---

## Phase 2: Typed Hook System

### Task 2.1: Define HookKind Enum

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add HookKind enum**

Add after `HookEvent` enum (around line 560):

```rust
/// Hook execution kind - determines how the hook is executed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HookKind {
    /// Interceptor: Pipeline execution, can modify context or block
    /// Execution: Sequential by priority, short-circuit on block
    Interceptor,

    /// Observer: Fire-and-forget, read-only context
    /// Execution: Parallel, errors logged but not propagated
    #[default]
    Observer,

    /// Resolver: First-win competition
    /// Execution: Sequential by priority, stops when one returns Some
    Resolver,
}

impl HookKind {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "interceptor" => HookKind::Interceptor,
            "resolver" => HookKind::Resolver,
            _ => HookKind::Observer,
        }
    }
}

/// Hook priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookPriority {
    /// System-level hooks (security, audit) - runs first
    System = -1000,
    /// High priority business logic
    High = -100,
    /// Default priority
    Normal = 0,
    /// Low priority extensions
    Low = 100,
}

impl Default for HookPriority {
    fn default() -> Self {
        HookPriority::Normal
    }
}

impl HookPriority {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "system" => HookPriority::System,
            "high" => HookPriority::High,
            "low" => HookPriority::Low,
            _ => HookPriority::Normal,
        }
    }

    pub fn as_i32(&self) -> i32 {
        match self {
            HookPriority::System => -1000,
            HookPriority::High => -100,
            HookPriority::Normal => 0,
            HookPriority::Low => 100,
        }
    }
}
```

**Step 2: Run tests**

Run: `cd core && cargo test hook`
Expected: Compiles and tests pass

**Step 3: Commit**

```bash
git add core/src/extension/types.rs
git commit -m "feat(extension): add HookKind and HookPriority enums"
```

---

### Task 2.2: Update HookConfig to Include Kind and Priority

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Update HookConfig struct**

Find `HookConfig` struct (around line 574) and update:

```rust
/// Hook configuration - defines when and how a hook executes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    pub event: HookEvent,

    /// Hook execution kind (V2)
    #[serde(default)]
    pub kind: HookKind,

    /// Hook priority (V2)
    #[serde(default)]
    pub priority: HookPriority,

    /// Pattern matcher for tool-related events (regex)
    pub matcher: Option<String>,

    /// Actions to execute when hook fires
    pub actions: Vec<HookAction>,

    /// Plugin that registered this hook
    pub plugin_name: String,

    /// Plugin root directory
    pub plugin_root: PathBuf,

    /// Handler function name (for runtime plugins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
}
```

**Step 2: Run tests**

Run: `cd core && cargo test`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/extension/types.rs
git commit -m "feat(extension): add kind and priority to HookConfig"
```

---

### Task 2.3: Implement Typed Hook Execution in HookExecutor

**Files:**
- Modify: `core/src/extension/hooks/mod.rs`

**Step 1: Add InterceptorResult type**

Add near the top of the file (after imports, around line 30):

```rust
/// Result from an interceptor hook
#[derive(Debug, Clone, Default)]
pub struct InterceptorResult {
    /// Whether to continue execution (pass)
    pub pass: bool,
    /// Modified context (if any)
    pub modified_context: Option<HookContext>,
    /// Block reason (if blocked)
    pub block_reason: Option<String>,
    /// Silent block (don't report error to user)
    pub silent: bool,
}

impl InterceptorResult {
    pub fn pass() -> Self {
        Self { pass: true, ..Default::default() }
    }

    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            pass: false,
            block_reason: Some(reason.into()),
            ..Default::default()
        }
    }

    pub fn block_silent(reason: impl Into<String>) -> Self {
        Self {
            pass: false,
            block_reason: Some(reason.into()),
            silent: true,
            ..Default::default()
        }
    }

    pub fn modified(ctx: HookContext) -> Self {
        Self {
            pass: true,
            modified_context: Some(ctx),
            ..Default::default()
        }
    }
}

/// Result from a resolver hook
#[derive(Debug, Clone)]
pub enum ResolverResult<T> {
    /// No resolution, try next resolver
    None,
    /// Resolved value, stop execution
    Some(T),
}
```

**Step 2: Add typed execution methods to HookExecutor**

Add these methods to the `impl HookExecutor` block (after existing `execute` method):

```rust
    /// Execute interceptor hooks in priority order
    /// Returns modified context or block reason
    pub async fn execute_interceptors(
        &self,
        event: HookEvent,
        mut context: HookContext,
    ) -> Result<(HookContext, Option<String>), AetherError> {
        let mut hooks: Vec<_> = self.hooks
            .iter()
            .filter(|h| h.event == event && h.kind == HookKind::Interceptor)
            .collect();

        // Sort by priority (lower value = earlier execution)
        hooks.sort_by_key(|h| h.priority.as_i32());

        for hook in hooks {
            // Check pattern match if applicable
            if let Some(ref matcher) = hook.matcher {
                if let Some(ref tool_name) = context.tool_name {
                    if !self.matches_pattern(tool_name, matcher) {
                        continue;
                    }
                }
            }

            // Execute hook actions
            let result = self.execute_hook_actions(hook, &context).await?;

            // Check for block
            if result.blocked {
                return Ok((context, result.block_reason));
            }

            // Apply modifications
            if let Some(modified_args) = result.modified_arguments {
                context.arguments = Some(modified_args);
            }
        }

        Ok((context, None))
    }

    /// Execute observer hooks in parallel (fire-and-forget)
    pub async fn execute_observers(&self, event: HookEvent, context: &HookContext) {
        let hooks: Vec<_> = self.hooks
            .iter()
            .filter(|h| h.event == event && h.kind == HookKind::Observer)
            .collect();

        // Execute all observers in parallel
        let futures: Vec<_> = hooks
            .iter()
            .map(|hook| {
                let hook = (*hook).clone();
                let ctx = context.clone();
                async move {
                    if let Err(e) = self.execute_hook_actions(&hook, &ctx).await {
                        tracing::warn!(
                            plugin = %hook.plugin_name,
                            event = ?hook.event,
                            error = %e,
                            "Observer hook failed (non-fatal)"
                        );
                    }
                }
            })
            .collect();

        // Wait for all but don't propagate errors
        futures::future::join_all(futures).await;
    }

    /// Execute resolver hooks until one returns a value
    pub async fn execute_resolvers<T, F>(
        &self,
        event: HookEvent,
        context: &HookContext,
        resolver_fn: F,
    ) -> Option<T>
    where
        F: Fn(&HookResult) -> Option<T>,
    {
        let mut hooks: Vec<_> = self.hooks
            .iter()
            .filter(|h| h.event == event && h.kind == HookKind::Resolver)
            .collect();

        // Sort by priority
        hooks.sort_by_key(|h| h.priority.as_i32());

        for hook in hooks {
            if let Ok(result) = self.execute_hook_actions(hook, context).await {
                if let Some(resolved) = resolver_fn(&result) {
                    return Some(resolved);
                }
            }
        }

        None
    }

    /// Internal: Execute actions for a single hook
    async fn execute_hook_actions(
        &self,
        hook: &HookConfig,
        context: &HookContext,
    ) -> Result<HookResult, AetherError> {
        let mut result = HookResult::default();

        for action in &hook.actions {
            match action {
                HookAction::Command { command } => {
                    let output = self.execute_command(
                        command,
                        context,
                        &hook.plugin_name,
                        &hook.plugin_root,
                    ).await?;

                    // Check for block signal
                    if output.starts_with("block:") {
                        result.blocked = true;
                        result.block_reason = Some(output.strip_prefix("block:").unwrap_or(&output).trim().to_string());
                        break;
                    }

                    result.action_results.push(HookActionResult {
                        action_type: "command".to_string(),
                        output: Some(output),
                        error: None,
                    });
                }
                HookAction::Prompt { prompt } => {
                    let rendered = self.substitute_variables(prompt, context, &hook.plugin_root);
                    result.messages.push(rendered);
                }
                HookAction::Agent { agent } => {
                    result.agents_to_invoke.push(agent.clone());
                }
            }
        }

        Ok(result)
    }
```

**Step 3: Run tests**

Run: `cd core && cargo test hooks`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/extension/hooks/mod.rs
git commit -m "feat(extension): implement typed hook execution (interceptor/observer/resolver)"
```

---

### Task 2.4: Create V2 Hooks from TOML Manifest

**Files:**
- Modify: `core/src/extension/mod.rs`

**Step 1: Add method to convert TOML hooks to HookConfig**

Find `impl ExtensionManager` and add this method:

```rust
    /// Convert V2 TOML hook declarations to HookConfig
    fn convert_v2_hooks(
        &self,
        hooks: &[crate::extension::manifest::aether_plugin_toml::HookSection],
        plugin_name: &str,
        plugin_root: &Path,
    ) -> Vec<HookConfig> {
        hooks
            .iter()
            .map(|h| {
                let event = match h.event.as_str() {
                    "before_agent_start" => HookEvent::SessionStart,
                    "before_tool_call" => HookEvent::PreToolUse,
                    "after_tool_call" => HookEvent::PostToolUse,
                    "before_message_send" => HookEvent::ChatMessage,
                    "after_message_send" => HookEvent::ChatResponse,
                    "on_error" => HookEvent::PostToolUseFailure,
                    "session_start" => HookEvent::SessionStart,
                    "session_end" => HookEvent::SessionEnd,
                    _ => HookEvent::PreToolUse, // Default
                };

                HookConfig {
                    event,
                    kind: HookKind::from_str(&h.kind),
                    priority: HookPriority::from_str(&h.priority),
                    matcher: h.filter.clone(),
                    actions: vec![], // Runtime hooks use handler, not actions
                    plugin_name: plugin_name.to_string(),
                    plugin_root: plugin_root.to_path_buf(),
                    handler: Some(h.handler.clone()),
                }
            })
            .collect()
    }
```

**Step 2: Run tests**

Run: `cd core && cargo test`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/extension/mod.rs
git commit -m "feat(extension): add V2 hook conversion from TOML manifest"
```

---

## Phase 3: Unified Prompt Scope

### Task 3.1: Define PromptScope Enum

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add PromptScope enum**

Add after `HookPriority` (around where you added it in Task 2.1):

```rust
/// Prompt injection scope
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptScope {
    /// System-level: Always injected when plugin is active
    #[default]
    System,

    /// Tool-bound: Injected when specific tool is available
    Tool,

    /// Standalone: User must explicitly invoke (command)
    Standalone,

    /// Disabled: Not injected
    Disabled,
}

impl PromptScope {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "system" => PromptScope::System,
            "tool" => PromptScope::Tool,
            "standalone" => PromptScope::Standalone,
            "disabled" => PromptScope::Disabled,
            _ => PromptScope::System,
        }
    }
}
```

**Step 2: Run tests**

Run: `cd core && cargo test`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/extension/types.rs
git commit -m "feat(extension): add PromptScope enum for V2 skill injection"
```

---

### Task 3.2: Update ExtensionSkill for Prompt Scope

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add scope field to ExtensionSkill**

Find `ExtensionSkill` struct (around line 68) and add:

```rust
    /// V2: Prompt injection scope
    #[serde(default)]
    pub scope: PromptScope,

    /// V2: Bound tool name (for Tool scope)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_tool: Option<String>,
```

**Step 2: Run tests**

Run: `cd core && cargo test`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/extension/types.rs
git commit -m "feat(extension): add scope and bound_tool to ExtensionSkill"
```

---

### Task 3.3: Load V2 Prompt Configuration

**Files:**
- Modify: `core/src/extension/loader.rs`

**Step 1: Add method to load V2 prompt from manifest**

Find `impl ComponentLoader` and add:

```rust
    /// Load V2 global prompt from manifest
    pub async fn load_v2_prompt(
        &self,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> Result<Option<ExtensionSkill>, AetherError> {
        let prompt_config = match &manifest.prompt_v2 {
            Some(p) => p,
            None => return Ok(None),
        };

        // Check if disabled
        if prompt_config.scope == "disabled" {
            return Ok(None);
        }

        // Read prompt file
        let prompt_path = plugin_dir.join(&prompt_config.file);
        let content = tokio::fs::read_to_string(&prompt_path).await.map_err(|e| {
            AetherError::PluginLoad(format!(
                "Failed to read prompt file {}: {}",
                prompt_path.display(),
                e
            ))
        })?;

        // Parse frontmatter if present
        let (frontmatter, body) = if content.starts_with("---") {
            parse_frontmatter::<SkillFrontmatter>(&content)?
        } else {
            (SkillFrontmatter::default(), content)
        };

        let skill = ExtensionSkill {
            name: frontmatter.name.unwrap_or_else(|| manifest.id.clone()),
            plugin_name: Some(manifest.id.clone()),
            skill_type: SkillType::Skill,
            description: frontmatter.description.unwrap_or_default(),
            content: body,
            disable_model_invocation: frontmatter.disable_model_invocation,
            source_path: prompt_path,
            source: DiscoverySource::Config,
            scope: PromptScope::from_str(&prompt_config.scope),
            bound_tool: None,
        };

        Ok(Some(skill))
    }

    /// Load V2 tool-bound prompts (instruction files)
    pub async fn load_v2_tool_prompts(
        &self,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> Result<Vec<ExtensionSkill>, AetherError> {
        let tools = match &manifest.tools_v2 {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        let mut skills = Vec::new();

        for tool in tools {
            if let Some(ref instruction_file) = tool.instruction_file {
                let path = plugin_dir.join(instruction_file);
                if path.exists() {
                    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                        AetherError::PluginLoad(format!(
                            "Failed to read instruction file {}: {}",
                            path.display(),
                            e
                        ))
                    })?;

                    let skill = ExtensionSkill {
                        name: format!("{}_instructions", tool.name),
                        plugin_name: Some(manifest.id.clone()),
                        skill_type: SkillType::Skill,
                        description: format!("Instructions for {} tool", tool.name),
                        content,
                        disable_model_invocation: true, // Tool-bound, not direct invoke
                        source_path: path,
                        source: DiscoverySource::Config,
                        scope: PromptScope::Tool,
                        bound_tool: Some(tool.name.clone()),
                    };

                    skills.push(skill);
                }
            }
        }

        Ok(skills)
    }
```

**Step 2: Run tests**

Run: `cd core && cargo test loader`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/extension/loader.rs
git commit -m "feat(extension): implement V2 prompt loading with scope support"
```

---

### Task 3.4: Integrate Prompt Scope in Skill Injection

**Files:**
- Modify: `core/src/extension/skill_tool.rs`

**Step 1: Add scope-aware skill filtering**

Add this function (near other helper functions):

```rust
/// Filter skills by scope for injection
pub fn filter_skills_by_scope(
    skills: &[ExtensionSkill],
    active_tools: Option<&[String]>,
) -> Vec<&ExtensionSkill> {
    skills
        .iter()
        .filter(|skill| {
            match skill.scope {
                PromptScope::System => true, // Always include
                PromptScope::Tool => {
                    // Only include if bound tool is active
                    if let (Some(bound), Some(tools)) = (&skill.bound_tool, active_tools) {
                        tools.iter().any(|t| t == bound)
                    } else {
                        false
                    }
                }
                PromptScope::Standalone => false, // Never auto-inject
                PromptScope::Disabled => false,
            }
        })
        .collect()
}
```

**Step 2: Update build_skill_tool_description to use scope filtering**

Find `build_skill_tool_description` and update to filter by scope:

```rust
pub fn build_skill_tool_description(
    skills: &[ExtensionSkill],
    active_tools: Option<&[String]>,
) -> String {
    let filtered = filter_skills_by_scope(skills, active_tools);

    if filtered.is_empty() {
        return String::new();
    }

    let mut output = String::from("<available_skills>\n");

    for skill in filtered {
        if skill.is_auto_invocable() {
            output.push_str(&format!(
                "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>\n",
                skill.qualified_name(),
                skill.description
            ));
        }
    }

    output.push_str("</available_skills>");
    output
}
```

**Step 3: Run tests**

Run: `cd core && cargo test skill`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/extension/skill_tool.rs
git commit -m "feat(extension): implement scope-aware skill injection"
```

---

## Phase 4: Integration Tests

### Task 4.1: Create Integration Test for V2 Plugin

**Files:**
- Create: `core/tests/extension_v2_test.rs`

**Step 1: Create test file**

```rust
//! Integration tests for Extension SDK V2

use std::path::PathBuf;
use tempfile::TempDir;
use std::fs;

use aethecore::extension::manifest::{
    parse_aether_plugin_toml_content, parse_manifest_from_dir_sync,
};
use aethecore::extension::types::{HookKind, HookPriority, PromptScope};

/// Create a temporary V2 plugin directory
fn create_test_plugin(toml_content: &str, skill_content: Option<&str>) -> TempDir {
    let dir = TempDir::new().unwrap();

    // Write TOML manifest
    fs::write(dir.path().join("aether_plugin.toml"), toml_content).unwrap();

    // Write SKILL.md if provided
    if let Some(content) = skill_content {
        fs::write(dir.path().join("SKILL.md"), content).unwrap();
    }

    dir
}

#[test]
fn test_v2_manifest_priority_over_json() {
    let dir = TempDir::new().unwrap();

    // Write both TOML and JSON
    fs::write(
        dir.path().join("aether_plugin.toml"),
        r#"
[plugin]
id = "toml-plugin"
"#,
    ).unwrap();

    fs::write(
        dir.path().join("aether.plugin.json"),
        r#"{"id": "json-plugin"}"#,
    ).unwrap();

    // TOML should win
    let manifest = parse_manifest_from_dir_sync(dir.path()).unwrap();
    assert_eq!(manifest.id, "toml-plugin");
}

#[test]
fn test_v2_tools_parsing() {
    let content = r#"
[plugin]
id = "test-tools"
kind = "nodejs"

[[tools]]
name = "my_tool"
description = "A test tool"
handler = "handleMyTool"
instruction_file = "docs/INSTRUCTIONS.md"

[tools.parameters]
type = "object"
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let tools = manifest.tools_v2.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "my_tool");
    assert_eq!(tools[0].handler, "handleMyTool");
    assert!(tools[0].instruction_file.is_some());
}

#[test]
fn test_v2_hooks_with_kind_and_priority() {
    let content = r#"
[plugin]
id = "test-hooks"

[[hooks]]
event = "before_tool_call"
kind = "interceptor"
priority = "high"
handler = "onBeforeTool"
filter = "query_*"

[[hooks]]
event = "after_tool_call"
kind = "observer"
handler = "onAfterTool"
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let hooks = manifest.hooks_v2.unwrap();
    assert_eq!(hooks.len(), 2);

    assert_eq!(hooks[0].kind, "interceptor");
    assert_eq!(hooks[0].priority, "high");
    assert_eq!(hooks[0].filter, Some("query_*".to_string()));

    assert_eq!(hooks[1].kind, "observer");
}

#[test]
fn test_v2_prompt_scope() {
    let content = r#"
[plugin]
id = "test-prompt"

[prompt]
file = "SKILL.md"
scope = "system"
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let prompt = manifest.prompt_v2.unwrap();
    assert_eq!(prompt.file, "SKILL.md");
    assert_eq!(prompt.scope, "system");
}

#[test]
fn test_v2_permissions_parsing() {
    let content = r#"
[plugin]
id = "test-perms"

[permissions]
network = ["connect:postgres://*", "connect:https://api.example.com/*"]
filesystem = ["read:./data", "write:/tmp/cache"]
env = ["DATABASE_URL", "API_*"]
shell = false
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    assert!(!manifest.permissions.is_empty());
    // Verify network permissions converted
    assert!(manifest.permissions.iter().any(|p| matches!(p, aethecore::extension::manifest::types::PluginPermission::Network(s) if s.contains("postgres"))));
}

#[test]
fn test_v2_capabilities_parsing() {
    let content = r#"
[plugin]
id = "test-caps"

[capabilities]
dynamic_tools = true
dynamic_hooks = ["after_*"]
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let caps = manifest.capabilities_v2.unwrap();
    assert!(caps.dynamic_tools);
    assert_eq!(caps.dynamic_hooks, vec!["after_*"]);
}

#[test]
fn test_v2_services_and_commands() {
    let content = r#"
[plugin]
id = "test-services"

[[services]]
name = "background-worker"
description = "Runs in background"
start_handler = "startWorker"
stop_handler = "stopWorker"

[[commands]]
name = "status"
description = "Show status"
handler = "handleStatus"
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let services = manifest.services_v2.unwrap();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].name, "background-worker");

    let commands = manifest.commands_v2.unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "status");
}
```

**Step 2: Run tests**

Run: `cd core && cargo test extension_v2`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/tests/extension_v2_test.rs
git commit -m "test(extension): add integration tests for SDK V2 features"
```

---

## Phase 5: Documentation Update

### Task 5.1: Update Extension System Documentation

**Files:**
- Modify: `docs/EXTENSION_SYSTEM.md`

**Step 1: Add V2 section to documentation**

Add a new section documenting the V2 features:

```markdown
## Extension SDK V2

### Manifest Format (aether_plugin.toml)

V2 plugins use TOML format for better readability and Rust ecosystem alignment.

```toml
[plugin]
id = "my-plugin"
name = "My Plugin"
version = "1.0.0"
kind = "nodejs"  # nodejs | wasm | static
entry = "dist/index.js"

[permissions]
network = ["connect:https://*"]
filesystem = ["read:./data"]

[prompt]
file = "SKILL.md"
scope = "system"  # system | tool | standalone | disabled

[[tools]]
name = "my_tool"
description = "..."
handler = "handleMyTool"
instruction_file = "docs/INSTRUCTIONS.md"

[[hooks]]
event = "before_tool_call"
kind = "interceptor"  # interceptor | observer | resolver
priority = "normal"   # system | high | normal | low
handler = "onBeforeTool"
```

### Hook Types

- **Interceptor**: Sequential execution, can modify context or block
- **Observer**: Parallel execution, fire-and-forget, errors logged
- **Resolver**: Sequential, first-win competition

### Prompt Scopes

- **system**: Always injected when plugin is active
- **tool**: Injected when bound tool is available
- **standalone**: User must explicitly invoke
- **disabled**: Never injected
```

**Step 2: Commit**

```bash
git add docs/EXTENSION_SYSTEM.md
git commit -m "docs(extension): add SDK V2 documentation"
```

---

## Summary

This plan implements P0 of Extension SDK V2:

| Phase | Tasks | Outcome |
|-------|-------|---------|
| **Phase 1** | 1.1-1.4 | TOML manifest parser with auto-detection |
| **Phase 2** | 2.1-2.4 | Typed hook system (Interceptor/Observer/Resolver) |
| **Phase 3** | 3.1-3.4 | Unified prompt scope (system/tool/standalone) |
| **Phase 4** | 4.1 | Integration tests |
| **Phase 5** | 5.1 | Documentation |

**Total estimated commits:** 13
