# Capability-Driven Plugin Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign Aleph's plugin system into a capability-driven architecture with multi-platform manifest adapters, dynamic registration API, and ~4,500 lines of deprecated code removal.

**Architecture:** Two core traits (`ManifestAdapter` for static parsing, `CapabilityRegistrar` for runtime registration) funnel through a unified `CapabilityApi` into the existing `PluginRegistry`. Deprecated Node.js runtime and legacy manifest formats are removed. New Codex/Cursor adapters added.

**Tech Stack:** Rust, serde, anyhow, tracing. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-03-25-capability-driven-plugin-architecture-design.md`

---

## File Map

### New Files
| File | Responsibility |
|------|---------------|
| `src/extension/capability.rs` | `CapabilityDeclaration` enum, `CapabilitySource`, `SourceFormat`, type aliases |
| `src/extension/manifest/adapter.rs` | `ManifestAdapter` trait, `AdapterOutput`, `AdapterRegistry` |
| `src/extension/manifest/parsers.rs` | Shared component parsers (skills/commands/agents/hooks/mcp dirs) |
| `src/extension/manifest/adapters/mod.rs` | Re-exports for adapter implementations |
| `src/extension/manifest/adapters/codex.rs` | Codex CLI manifest adapter |
| `src/extension/manifest/adapters/cursor.rs` | Cursor IDE manifest adapter |
| `src/extension/manifest/adapters/auto_discover.rs` | Auto-discover fallback adapter |
| `src/extension/registrar/mod.rs` | `CapabilityRegistrar` trait, re-exports |
| `src/extension/registrar/api.rs` | `CapabilityApi` — unified write surface for PluginRegistry |
| `src/extension/registrar/mcp_registrar.rs` | MCP collect-then-batch probe and register |
| `src/extension/registrar/wasm_registrar.rs` | WASM host function for capability registration |

### Files to Delete
| File/Directory | Lines |
|---------------|-------|
| `src/extension/manifest/aleph_plugin_toml/` (6 files) | 1,279 |
| `src/extension/manifest/aleph_plugin.rs` | 636 |
| `src/extension/manifest/package_json.rs` | 474 |
| `src/extension/runtime/nodejs/` (3 files) | 1,026 |
| `src/extension/runtime/mod.rs` | 812 |
| `src/extension/content_loader/` (6 files) | 1,125 |
| `src/extension/plugin_loader.rs` | 778 (replaced by `loader.rs`) |

### Files to Modify
| File | Change |
|------|--------|
| `src/extension/mod.rs` | Remove old module refs, add new ones, simplify ExtensionManager |
| `src/extension/registry/types.rs` | Add `SkillRegistration`, `AgentRegistration` |
| `src/extension/registry/plugin_registry/mod.rs` → `plugin_registry.rs` | Promote to file, add skill/agent storage |
| `src/extension/manifest/mod.rs` | Rewrite as AdapterRegistry entry point |
| `src/extension/manifest/types.rs` | Remove `aleph_plugin_toml` imports, update `PluginPermission` with new variants |
| `src/extension/manifest/cc_plugin_toml.rs` | Refactor to implement `ManifestAdapter` trait |
| `src/extension/manifest/cc_plugin_json.rs` | Refactor to implement `ManifestAdapter` trait |
| `src/extension/types/plugins.rs` | Remove `NodeJs` from `PluginKind` |
| `src/extension/runtime/wasm/mod.rs` | Add `host_fn_register` integration |
| `src/extension/sync_api.rs` | Delegate skills/commands/agents to PluginRegistry |
| `src/extension/skill_ops.rs` | Delegate to PluginRegistry |

---

## Task 1: Foundation — CapabilityDeclaration & Registry Extensions

**Files:**
- Create: `src/extension/capability.rs`
- Modify: `src/extension/registry/types.rs`
- Modify: `src/extension/registry/plugin_registry/mod.rs`
- Modify: `src/extension/manifest/types.rs` (update `PluginPermission`)
- Modify: `src/extension/mod.rs` (add `pub mod capability;`)
- Test: inline `#[cfg(test)]` modules

- [ ] **Step 0: Update `PluginPermission` enum with new variants**

In `src/extension/manifest/types.rs`, update the `PluginPermission` enum to add dedicated variants for capability-tier gating:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginPermission {
    Network,
    #[serde(rename = "filesystem:read")]
    FilesystemRead,
    #[serde(rename = "filesystem:write")]
    FilesystemWrite,
    Filesystem,
    Env,
    /// Shell command execution (required for CliCommand registration)
    Shell,
    /// Background service registration
    Background,
    /// HTTP route registration
    #[serde(rename = "http-routes")]
    HttpRoutes,
    /// Gateway RPC method registration
    #[serde(rename = "gateway-rpc")]
    GatewayRpc,
    #[serde(untagged)]
    Custom(String),
}
```

Update the `Display` impl to handle the new variants. Note: `FilesystemRead`/`FilesystemWrite`/`Filesystem` consolidation into `Filesystem(FilesystemAccess)` is **deferred** to avoid a breaking change in this iteration — only the new variants are added now.

- [ ] **Step 1: Add `SkillRegistration` and `AgentRegistration` to registry types**

Add to `src/extension/registry/types.rs` after the P3 section:

```rust
// ============================================================================
// Skill & Agent Registration Types (added for capability-driven architecture)
// ============================================================================

/// Skill registration for plugins to expose prompt-based skills
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRegistration {
    /// Skill name (unique within plugin namespace)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Markdown content (the skill prompt)
    pub content: String,
    /// Natural language trigger keywords
    pub triggers: Vec<String>,
    /// Tools this skill is allowed to use
    pub allowed_tools: Vec<String>,
    /// Category tag (developer, media, productivity)
    pub category: Option<String>,
    /// ID of the plugin that registered this skill
    pub plugin_id: String,
}

/// Agent registration for plugins to expose agent definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistration {
    /// Agent name
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// System prompt markdown content
    pub content: String,
    /// Preferred model override
    pub model: Option<String>,
    /// ID of the plugin that registered this agent
    pub plugin_id: String,
}
```

- [ ] **Step 2: Add skills/agents storage to PluginRegistry**

In `src/extension/registry/plugin_registry/mod.rs`, add fields and methods:

```rust
// In struct PluginRegistry:
/// Registered skills by name
skills: HashMap<String, SkillRegistration>,
/// Registered agents by name
agents: HashMap<String, AgentRegistration>,

// In clear():
self.skills.clear();
self.agents.clear();

// In unregister_plugin():
self.skills.retain(|_, s| s.plugin_id != plugin_id);
self.agents.retain(|_, a| a.plugin_id != plugin_id);
```

Add registration methods (same pattern as `register_tool`):

```rust
pub fn register_skill(&mut self, skill: SkillRegistration) {
    let namespaced_key = format!("{}:{}", skill.plugin_id, skill.name);
    let short_key = skill.name.clone();
    self.skills.entry(short_key).or_insert_with(|| skill.clone());
    self.skills.insert(namespaced_key, skill);
}

pub fn get_skill(&self, name: &str) -> Option<&SkillRegistration> {
    self.skills.get(name)
}

pub fn list_skills(&self) -> Vec<&SkillRegistration> {
    self.skills.values().collect()
}

pub fn register_agent(&mut self, agent: AgentRegistration) {
    let namespaced_key = format!("{}:{}", agent.plugin_id, agent.name);
    let short_key = agent.name.clone();
    self.agents.entry(short_key).or_insert_with(|| agent.clone());
    self.agents.insert(namespaced_key, agent);
}

pub fn get_agent(&self, name: &str) -> Option<&AgentRegistration> {
    self.agents.get(name)
}

pub fn list_agents(&self) -> Vec<&AgentRegistration> {
    self.agents.values().collect()
}
```

Update `stats()` to include `skills` and `agents` counts.

- [ ] **Step 3: Create `capability.rs`**

Create `src/extension/capability.rs`:

```rust
//! Capability Declaration Model
//!
//! Defines `CapabilityDeclaration` as the universal intermediate representation
//! for plugin capabilities. Both static manifest parsing and dynamic runtime
//! registration produce these declarations, which are then written into
//! `PluginRegistry` via `CapabilityApi`.

use crate::extension::registry::types::{
    AgentRegistration, ChannelRegistration, CliRegistration, CommandRegistration,
    GatewayMethodRegistration, HookRegistration, HttpRouteRegistration, ProviderRegistration,
    ServiceRegistration, SkillRegistration, ToolRegistration,
};
use crate::extension::types::{McpServerConfig, PluginOrigin};
use crate::extension::manifest::types::PluginPermission;

// Type aliases: Declaration = Registration (same struct, semantic naming)
pub type ToolDeclaration = ToolRegistration;
pub type HookDeclaration = HookRegistration;
pub type SkillDeclaration = SkillRegistration;
pub type CommandDeclaration = CommandRegistration;
pub type AgentDeclaration = AgentRegistration;
pub type ProviderDeclaration = ProviderRegistration;
pub type ChannelDeclaration = ChannelRegistration;
pub type ServiceDeclaration = ServiceRegistration;
pub type McpServerDeclaration = McpServerConfig;
pub type GatewayMethodDeclaration = GatewayMethodRegistration;
pub type HttpRouteDeclaration = HttpRouteRegistration;
pub type CliDeclaration = CliRegistration;

/// A capability that a plugin declares it can provide.
/// Serialize/Deserialize needed for WASM host function (JSON roundtrip).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapabilityDeclaration {
    // P0 Core
    Tool(ToolDeclaration),
    Hook(HookDeclaration),
    Skill(SkillDeclaration),
    // P1 Important
    Command(CommandDeclaration),
    Agent(AgentDeclaration),
    // P2 Pluggable
    Provider(ProviderDeclaration),
    Channel(ChannelDeclaration),
    Service(ServiceDeclaration),
    McpServer(McpServerDeclaration),
    // P3 Gateway Extensions
    GatewayMethod(GatewayMethodDeclaration),
    HttpRoute(HttpRouteDeclaration),
    CliCommand(CliDeclaration),
}

impl CapabilityDeclaration {
    /// Returns the capability tier for permission checking.
    pub fn tier(&self) -> Tier {
        match self {
            Self::Tool(_) | Self::Hook(_) | Self::Skill(_) => Tier::Core,
            Self::Command(_) | Self::Agent(_) => Tier::Important,
            Self::Provider(_) | Self::Channel(_) | Self::Service(_) | Self::McpServer(_) => Tier::Pluggable,
            Self::GatewayMethod(_) | Self::HttpRoute(_) | Self::CliCommand(_) => Tier::GatewayExtension,
        }
    }

    /// Returns a human-readable kind name for diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Tool(_) => "tool",
            Self::Hook(_) => "hook",
            Self::Skill(_) => "skill",
            Self::Command(_) => "command",
            Self::Agent(_) => "agent",
            Self::Provider(_) => "provider",
            Self::Channel(_) => "channel",
            Self::Service(_) => "service",
            Self::McpServer(_) => "mcp_server",
            Self::GatewayMethod(_) => "gateway_method",
            Self::HttpRoute(_) => "http_route",
            Self::CliCommand(_) => "cli_command",
        }
    }
}

/// Capability tier for permission and policy grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    Core,             // P0: Tool, Hook, Skill — always allowed
    Important,        // P1: Command, Agent — always allowed
    Pluggable,        // P2: Provider, Channel, Service, McpServer — some permission-gated
    GatewayExtension, // P3: HttpRoute, GatewayMethod, CliCommand — all permission-gated
}

/// Metadata about a capability's source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySource {
    pub plugin_id: String,
    pub origin: PluginOrigin,
    pub source_format: SourceFormat,
    pub permissions: Vec<PluginPermission>,
}

/// Which format the capability was declared in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    ClaudeCode,
    Codex,
    Cursor,
    AlephNative,
    AutoDiscovered,
    RuntimeMcp,
    RuntimeWasm,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_classification() {
        let tool = CapabilityDeclaration::Tool(ToolDeclaration {
            name: "test".into(),
            description: "test tool".into(),
            parameters: serde_json::json!({}),
            handler: "handle".into(),
            plugin_id: "test-plugin".into(),
        });
        assert_eq!(tool.tier(), Tier::Core);
        assert_eq!(tool.kind_name(), "tool");
    }

    #[test]
    fn test_all_tiers() {
        // Verify each variant maps to expected tier
        assert_eq!(CapabilityDeclaration::Skill(SkillDeclaration {
            name: "s".into(), description: "".into(), content: "".into(),
            triggers: vec![], allowed_tools: vec![], category: None, plugin_id: "p".into(),
        }).tier(), Tier::Core);
    }
}
```

- [ ] **Step 4: Wire `capability` module into `mod.rs`**

Add `pub mod capability;` to `src/extension/mod.rs` after line 35, and add re-export:
```rust
pub use capability::{CapabilityDeclaration, CapabilitySource, SourceFormat, Tier};
```

- [ ] **Step 5: Run `cargo check -p alephcore`**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors.

- [ ] **Step 6: Commit**

```bash
git add src/extension/capability.rs src/extension/registry/ src/extension/mod.rs
git commit -m "extension: add CapabilityDeclaration model and SkillRegistration/AgentRegistration to PluginRegistry"
```

---

## Task 2: CapabilityApi — Unified Registration Surface

**Files:**
- Create: `src/extension/registrar/mod.rs`
- Create: `src/extension/registrar/api.rs`
- Modify: `src/extension/mod.rs` (add `pub mod registrar;`)
- Test: inline `#[cfg(test)]` in `api.rs`

- [ ] **Step 1: Create `registrar/mod.rs`**

```rust
//! Capability Registrar — dynamic registration API for plugins
//!
//! Provides `CapabilityApi` for writing capabilities into `PluginRegistry`,
//! and `CapabilityRegistrar` trait for runtime plugins.

pub mod api;
pub mod mcp_registrar;
pub mod wasm_registrar;

pub use api::CapabilityApi;

use anyhow::Result;

/// Trait for runtime plugins that register capabilities dynamically.
pub trait CapabilityRegistrar: Send + Sync {
    fn register(&self, api: &mut CapabilityApi) -> Result<()>;
    fn unregister(&self, api: &mut CapabilityApi) -> Result<()>;
}
```

- [ ] **Step 2: Create `registrar/api.rs`**

Full implementation of `CapabilityApi` with permission checking, dispatch, and reload. See spec Section 2 for the complete API surface. Key implementation:

```rust
use anyhow::{anyhow, Result};
use tracing;

use crate::extension::capability::{CapabilityDeclaration, Tier};
use crate::extension::manifest::types::PluginPermission;
use crate::extension::registry::plugin_registry::PluginRegistry;
use crate::extension::registry::types::*;
use crate::extension::types::{McpServerConfig, PluginRecord};

pub struct CapabilityApi<'a> {
    plugin_id: String,
    registry: &'a mut PluginRegistry,
    permissions: Vec<PluginPermission>,
}

impl<'a> CapabilityApi<'a> {
    pub fn new(plugin_id: &str, registry: &'a mut PluginRegistry, permissions: &[PluginPermission]) -> Self {
        Self { plugin_id: plugin_id.to_string(), registry, permissions: permissions.to_vec() }
    }

    pub fn register_capability(&mut self, decl: CapabilityDeclaration) -> Result<()> {
        match decl.tier() {
            Tier::Core | Tier::Important => {},
            Tier::Pluggable => {
                if let Some(perm) = Self::required_permission(&decl) {
                    self.require_permission(perm)?;
                }
            }
            Tier::GatewayExtension => {
                if let Some(perm) = Self::required_permission(&decl) {
                    self.require_permission(perm)?;
                }
                tracing::warn!(plugin = %self.plugin_id, kind = decl.kind_name(), "P3 gateway extension registered");
            }
        }
        self.dispatch(decl)
    }

    fn dispatch(&mut self, decl: CapabilityDeclaration) -> Result<()> {
        match decl {
            CapabilityDeclaration::Tool(d) => { self.registry.register_tool(d); Ok(()) }
            CapabilityDeclaration::Hook(d) => { self.registry.register_hook(d); Ok(()) }
            CapabilityDeclaration::Skill(d) => { self.registry.register_skill(d); Ok(()) }
            CapabilityDeclaration::Command(d) => { self.registry.register_command(d); Ok(()) }
            CapabilityDeclaration::Agent(d) => { self.registry.register_agent(d); Ok(()) }
            CapabilityDeclaration::Provider(d) => { self.registry.register_provider(d); Ok(()) }
            CapabilityDeclaration::Channel(d) => { self.registry.register_channel(d); Ok(()) }
            CapabilityDeclaration::Service(d) => { self.registry.register_service(d); Ok(()) }
            CapabilityDeclaration::McpServer(_) => Ok(()), // MCP servers handled by loader, not registry
            CapabilityDeclaration::GatewayMethod(d) => { self.registry.register_gateway_method(d); Ok(()) }
            CapabilityDeclaration::HttpRoute(d) => { self.registry.register_http_route(d); Ok(()) }
            CapabilityDeclaration::CliCommand(d) => { self.registry.register_cli_command(d); Ok(()) }
        }
    }

    pub fn reload(&mut self, record: PluginRecord, new_caps: Vec<CapabilityDeclaration>) -> Result<()> {
        self.registry.unregister_plugin(&self.plugin_id);
        self.registry.register_plugin(record);
        for cap in new_caps {
            self.register_capability(cap)?;
        }
        Ok(())
    }

    fn required_permission(decl: &CapabilityDeclaration) -> Option<PluginPermission> {
        match decl {
            CapabilityDeclaration::Channel(_) => Some(PluginPermission::Network),
            CapabilityDeclaration::Service(_) => Some(PluginPermission::Background),
            CapabilityDeclaration::HttpRoute(_) => Some(PluginPermission::HttpRoutes),
            CapabilityDeclaration::GatewayMethod(_) => Some(PluginPermission::GatewayRpc),
            CapabilityDeclaration::CliCommand(_) => Some(PluginPermission::Shell),
            _ => None,
        }
    }

    fn require_permission(&self, perm: PluginPermission) -> Result<()> {
        if self.permissions.contains(&perm) {
            Ok(())
        } else {
            Err(anyhow!("Plugin '{}' lacks permission {:?} to register {} capability",
                self.plugin_id, perm, "this"))
        }
    }
}
```

Include tests for: P0 registration (no permission needed), P2 permission gating, P3 with logging, reload, and dispatch to all variants.

- [ ] **Step 3: Create stub `registrar/mcp_registrar.rs` and `registrar/wasm_registrar.rs`**

Create minimal stubs that compile (full implementation in Task 6/7):

```rust
// mcp_registrar.rs
//! MCP Registrar — collect-then-batch capability registration for MCP plugins
pub struct McpRegistrar { pub plugin_id: String }

// wasm_registrar.rs
//! WASM Registrar — host function for WASM plugin capability registration
```

- [ ] **Step 4: Wire `registrar` module into `mod.rs`**

Add `pub mod registrar;` to `src/extension/mod.rs` and `pub use registrar::CapabilityApi;`.

- [ ] **Step 5: Run `cargo check -p alephcore` and `cargo test -p alephcore --lib capability`**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib capability`
Expected: Compiles and tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/extension/registrar/
git commit -m "extension: add CapabilityApi with tiered permission checking and hot reload"
```

---

## Task 3: ManifestAdapter Trait & AdapterRegistry

**Files:**
- Create: `src/extension/manifest/adapter.rs`
- Create: `src/extension/manifest/adapters/mod.rs`
- Test: inline `#[cfg(test)]` in `adapter.rs`

- [ ] **Step 1: Create `manifest/adapter.rs`**

```rust
//! ManifestAdapter trait and AdapterRegistry
//!
//! Provides a trait-based system for parsing plugin directories from
//! multiple platform formats (Claude Code, Codex, Cursor, auto-discover).

use std::path::Path;
use anyhow::{anyhow, Result};
use crate::extension::capability::{CapabilityDeclaration, CapabilitySource};

/// Output from a manifest adapter parse operation.
#[derive(Debug)]
pub struct AdapterOutput {
    pub plugin_id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub capabilities: Vec<CapabilityDeclaration>,
    pub source: CapabilitySource,
}

/// Trait for parsing plugin directories into capability declarations.
pub trait ManifestAdapter: Send + Sync {
    /// Check if this adapter can handle the given directory.
    fn detect(&self, plugin_dir: &Path) -> bool;

    /// Parse the plugin directory and return all capabilities.
    fn parse(&self, plugin_dir: &Path) -> Result<AdapterOutput>;

    /// Human-readable format name for diagnostics.
    fn format_name(&self) -> &str;

    /// Detection priority (higher = checked first).
    fn priority(&self) -> i32 { 0 }
}

/// Registry of manifest adapters, tried in priority order.
pub struct AdapterRegistry {
    adapters: Vec<Box<dyn ManifestAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self { adapters: vec![] }
    }

    /// Create with all built-in adapters registered.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        // Adapters registered in Tasks 4-5
        registry
    }

    pub fn register(&mut self, adapter: Box<dyn ManifestAdapter>) {
        self.adapters.push(adapter);
        self.adapters.sort_by(|a, b| b.priority().cmp(&a.priority()));
    }

    /// Try adapters in priority order, return first match.
    pub fn parse_dir(&self, dir: &Path) -> Result<AdapterOutput> {
        for adapter in &self.adapters {
            if adapter.detect(dir) {
                tracing::debug!(adapter = adapter.format_name(), dir = %dir.display(), "Adapter matched");
                return adapter.parse(dir);
            }
        }
        Err(anyhow!("No manifest adapter matched directory: {}", dir.display()))
    }

    /// Number of registered adapters.
    pub fn len(&self) -> usize { self.adapters.len() }
}
```

Include tests for: adapter priority ordering, parse_dir first-match behavior, empty registry error.

- [ ] **Step 2: Create `manifest/adapters/` with mod.rs and stub files**

Create `manifest/adapters/mod.rs`:
```rust
//! Platform-specific manifest adapter implementations
pub mod auto_discover;
pub mod codex;
pub mod cursor;
```

Create empty stub files to satisfy the module declarations:
- `manifest/adapters/auto_discover.rs` → `//! Auto-discover adapter (implemented in Task 5)`
- `manifest/adapters/codex.rs` → `//! Codex adapter (implemented in Task 5)`
- `manifest/adapters/cursor.rs` → `//! Cursor adapter (implemented in Task 5)`

- [ ] **Step 3: Run `cargo check -p alephcore`**

Expected: Compiles (adapter stubs are empty but valid).

- [ ] **Step 4: Commit**

```bash
git add src/extension/manifest/adapter.rs src/extension/manifest/adapters/
git commit -m "extension: add ManifestAdapter trait and AdapterRegistry"
```

---

## Task 4: Shared Parsers & CC Adapter Refactor

**Files:**
- Create: `src/extension/manifest/parsers.rs`
- Modify: `src/extension/manifest/cc_plugin_toml.rs` (add `ManifestAdapter` impl)
- Modify: `src/extension/manifest/cc_plugin_json.rs` (add `ManifestAdapter` impl)
- Modify: `src/extension/manifest/adapter.rs` (register CC adapters in `with_defaults`)

This is the largest task — it extracts parsing logic from `content_loader/` into shared parsers, and wraps the existing CC parsers with the `ManifestAdapter` trait.

- [ ] **Step 1: Create `manifest/parsers.rs`**

Extract skill/command/agent/hook/mcp parsing from `content_loader/skill.rs`, `content_loader/plugin.rs`, and `mcp_config.rs` into shared functions that return `Vec<CapabilityDeclaration>`.

Key functions:
```rust
pub fn parse_skills_dir(base: &Path, rel_path: &str, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
pub fn parse_commands_dir(base: &Path, rel_path: &str, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
pub fn parse_agents_dir(base: &Path, rel_path: &str, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
pub fn parse_hooks_file(base: &Path, rel_path: &str, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
pub fn parse_mcp_config(base: &Path, rel_path: &str, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
```

Each function reuses existing parsing logic (skill frontmatter parsing from `content_loader/skill.rs`, hooks JSON from `content_loader/plugin.rs`, MCP from `mcp_config.rs`) but outputs `CapabilityDeclaration` instead of the old types.

- [ ] **Step 2: Add `ManifestAdapter` impl to `cc_plugin_toml.rs`**

Add at the end of the file:
```rust
use super::adapter::{AdapterOutput, ManifestAdapter};
use crate::extension::capability::*;

pub struct ClaudeCodeTomlAdapter;

impl ManifestAdapter for ClaudeCodeTomlAdapter {
    fn detect(&self, dir: &Path) -> bool {
        dir.join(".claude-plugin/plugin.toml").exists()
    }

    fn parse(&self, dir: &Path) -> anyhow::Result<AdapterOutput> {
        // Reuse existing parse_cc_plugin_toml() logic
        // Convert PluginManifest → Vec<CapabilityDeclaration> via parsers.rs
        todo!("Wrap existing parse logic")
    }

    fn format_name(&self) -> &str { "Claude Code (TOML)" }
    fn priority(&self) -> i32 { 100 }
}
```

- [ ] **Step 3: Add `ManifestAdapter` impl to `cc_plugin_json.rs`**

Same pattern, priority 90.

- [ ] **Step 4: Register CC adapters in `AdapterRegistry::with_defaults()`**

```rust
pub fn with_defaults() -> Self {
    let mut registry = Self::new();
    registry.register(Box::new(super::cc_plugin_toml::ClaudeCodeTomlAdapter));
    registry.register(Box::new(super::cc_plugin_json::ClaudeCodeJsonAdapter));
    // Codex and Cursor added in Task 5
    registry.register(Box::new(super::adapters::auto_discover::AutoDiscoverAdapter));
    registry
}
```

- [ ] **Step 5: Run `cargo check -p alephcore`**

Expected: Compiles. The adapters may use `todo!()` for parse bodies initially.

- [ ] **Step 6: Implement CC TOML adapter parse body**

Replace `todo!()` with actual parsing that calls `parsers.rs` functions. The adapter calls existing `parse_cc_plugin_toml()` to get the manifest, then uses `parsers::parse_skills_dir()` etc. to build `Vec<CapabilityDeclaration>`.

- [ ] **Step 7: Implement CC JSON adapter parse body**

Same pattern for JSON.

- [ ] **Step 8: Run `cargo test -p alephcore --lib`**

Expected: All existing tests pass. The adapter is additive — old code still works.

- [ ] **Step 9: Commit**

```bash
git add src/extension/manifest/
git commit -m "extension: add shared parsers and ClaudeCode ManifestAdapter impls"
```

---

## Task 5: Codex, Cursor & AutoDiscover Adapters

**Files:**
- Create: `src/extension/manifest/adapters/codex.rs`
- Create: `src/extension/manifest/adapters/cursor.rs`
- Create: `src/extension/manifest/adapters/auto_discover.rs`
- Modify: `src/extension/manifest/adapter.rs` (register in `with_defaults`)

- [ ] **Step 1: Create `adapters/codex.rs`**

```rust
pub struct CodexAdapter;

impl ManifestAdapter for CodexAdapter {
    fn detect(&self, dir: &Path) -> bool {
        dir.join(".codex-plugin/plugin.json").exists()
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        let manifest_path = dir.join(".codex-plugin/plugin.json");
        let content = std::fs::read_to_string(&manifest_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;

        let plugin_id = json.get("name").and_then(|v| v.as_str())
            .unwrap_or_else(|| dir.file_name().unwrap_or_default().to_str().unwrap_or("unknown"))
            .to_string();

        let mut caps = vec![];

        // Codex supports: skills, hooks, mcpServers
        if let Some(skills_path) = json.get("skills").and_then(|v| v.as_str()) {
            caps.extend(parsers::parse_skills_dir(dir, skills_path, &plugin_id)?);
        }
        if let Some(hooks_path) = json.get("hooks").and_then(|v| v.as_str()) {
            caps.extend(parsers::parse_hooks_file(dir, hooks_path, &plugin_id)?);
        }
        if let Some(mcp_path) = json.get("mcpServers").and_then(|v| v.as_str()) {
            caps.extend(parsers::parse_mcp_config(dir, mcp_path, &plugin_id)?);
        }

        Ok(AdapterOutput { plugin_id, capabilities: caps, /* ... */ })
    }

    fn format_name(&self) -> &str { "Codex CLI" }
    fn priority(&self) -> i32 { 80 }
}
```

- [ ] **Step 2: Create `adapters/cursor.rs`**

```rust
pub struct CursorAdapter;

impl ManifestAdapter for CursorAdapter {
    fn detect(&self, dir: &Path) -> bool {
        dir.join(".cursor-plugin/plugin.json").exists()
            || dir.join(".cursorrules").exists()
            || dir.join(".cursor/rules").is_dir()
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        // Parse .cursor-plugin/plugin.json if exists
        // Map .cursorrules → single skill
        // Map .cursor/rules/ → skills
        // ...
    }

    fn format_name(&self) -> &str { "Cursor IDE" }
    fn priority(&self) -> i32 { 70 }
}
```

- [ ] **Step 3: Create `adapters/auto_discover.rs`**

Migrate logic from existing `manifest/auto_discover.rs` (242 lines) to implement `ManifestAdapter`:

```rust
pub struct AutoDiscoverAdapter;

impl ManifestAdapter for AutoDiscoverAdapter {
    fn detect(&self, dir: &Path) -> bool {
        // Check for any known component directories
        dir.join("skills").is_dir()
            || dir.join("commands").is_dir()
            || dir.join("agents").is_dir()
            || dir.join("hooks").exists()
            || dir.join(".mcp.json").exists()
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        // Scan for all component types using parsers.rs
        // ...
    }

    fn format_name(&self) -> &str { "Auto-discovered" }
    fn priority(&self) -> i32 { -100 }
}
```

- [ ] **Step 4: Register all adapters in `AdapterRegistry::with_defaults()`**

```rust
pub fn with_defaults() -> Self {
    let mut registry = Self::new();
    registry.register(Box::new(cc_plugin_toml::ClaudeCodeTomlAdapter));
    registry.register(Box::new(cc_plugin_json::ClaudeCodeJsonAdapter));
    registry.register(Box::new(adapters::codex::CodexAdapter));
    registry.register(Box::new(adapters::cursor::CursorAdapter));
    registry.register(Box::new(adapters::auto_discover::AutoDiscoverAdapter));
    registry
}
```

- [ ] **Step 5: Add tests for each adapter**

Create test fixtures (temporary directories with mock manifest files) and verify:
- CC TOML adapter detects `.claude-plugin/plugin.toml`
- CC JSON adapter detects `.claude-plugin/plugin.json`
- Codex adapter detects `.codex-plugin/plugin.json`
- Cursor adapter detects `.cursorrules`
- AutoDiscover detects bare `skills/` directory
- Priority ordering: CC TOML > CC JSON > Codex > Cursor > AutoDiscover

- [ ] **Step 6: Run `cargo test -p alephcore --lib`**

Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/extension/manifest/
git commit -m "extension: add Codex, Cursor, and AutoDiscover manifest adapters"
```

---

## Task 6: MCP Registrar (collect-then-batch)

**Files:**
- Modify: `src/extension/registrar/mcp_registrar.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Implement `McpRegistrar`**

Replace stub with full collect-then-batch implementation:

```rust
use anyhow::Result;
use crate::extension::capability::*;
use crate::extension::manifest::types::PluginPermission;
use crate::extension::registrar::api::CapabilityApi;
use crate::extension::registry::plugin_registry::PluginRegistry;

pub struct McpRegistrar {
    pub plugin_id: String,
    pub permissions: Vec<PluginPermission>,
}

impl McpRegistrar {
    pub fn new(plugin_id: String, permissions: Vec<PluginPermission>) -> Self {
        Self { plugin_id, permissions }
    }

    /// Phase 2 (sync): Write collected capabilities into registry.
    pub fn batch_register(
        &self,
        caps: Vec<CapabilityDeclaration>,
        registry: &mut PluginRegistry,
    ) -> Result<()> {
        let mut api = CapabilityApi::new(&self.plugin_id, registry, &self.permissions);
        for cap in caps {
            api.register_capability(cap)?;
        }
        Ok(())
    }
}
```

Note: The async `probe_capabilities()` method depends on `McpClient` which is in the MCP module. Add it as a separate method that can be called from the MCP connection callback. The sync `batch_register` is the core testable unit.

- [ ] **Step 2: Add tests for batch_register**

Test that batch registration writes to the correct registry collections.

- [ ] **Step 3: Commit**

```bash
git add src/extension/registrar/mcp_registrar.rs
git commit -m "extension: implement McpRegistrar collect-then-batch pattern"
```

---

## Task 7: WASM Registrar Host Function

**Files:**
- Modify: `src/extension/registrar/wasm_registrar.rs`
- Modify: `src/extension/runtime/wasm/mod.rs` (wire host function)
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Implement `wasm_registrar.rs`**

```rust
use anyhow::Result;
use std::sync::RwLock;
use crate::extension::capability::CapabilityDeclaration;
use crate::extension::manifest::types::PluginPermission;
use crate::extension::registrar::api::CapabilityApi;
use crate::extension::registry::plugin_registry::PluginRegistry;

/// Host function for WASM plugins to register capabilities.
/// Input: JSON-serialized Vec<CapabilityDeclaration>
pub fn host_fn_register(
    plugin_id: &str,
    registry: &RwLock<PluginRegistry>,
    permissions: &[PluginPermission],
    input: &[u8],
) -> Result<()> {
    let declarations: Vec<CapabilityDeclaration> = serde_json::from_slice(input)?;
    let mut reg = registry.write().unwrap_or_else(|e| e.into_inner());
    let mut api = CapabilityApi::new(plugin_id, &mut reg, permissions);
    for decl in declarations {
        api.register_capability(decl)?;
    }
    Ok(())
}
```

- [ ] **Step 2: Wire into WASM runtime**

In `src/extension/runtime/wasm/mod.rs`, add the `aleph_register` host function to the Extism plugin initialization alongside existing `log`, `now_millis`, etc.

- [ ] **Step 3: Add tests**

Test that `host_fn_register` correctly deserializes and writes to registry.

- [ ] **Step 4: Commit**

```bash
git add src/extension/registrar/wasm_registrar.rs src/extension/runtime/wasm/
git commit -m "extension: add WASM host function for capability registration"
```

---

## Task 8: Delete Deprecated Code

**Files to delete:**
- `src/extension/manifest/aleph_plugin_toml/` (entire directory)
- `src/extension/manifest/aleph_plugin.rs`
- `src/extension/manifest/package_json.rs`
- `src/extension/runtime/nodejs/` (entire directory)
- `src/extension/runtime/mod.rs`
- `src/extension/content_loader/` (entire directory)

**Files to modify:**
- `src/extension/mod.rs` (remove old module declarations)
- `src/extension/manifest/mod.rs` (remove old parser imports and fallback chain)
- `src/extension/manifest/types.rs` (remove `aleph_plugin_toml` imports)
- `src/extension/types/plugins.rs` (remove `NodeJs` from `PluginKind`)

This task is large but mechanical. Do it in sub-steps.

- [ ] **Step 1: Remove Node.js runtime**

```bash
rm -rf src/extension/runtime/nodejs/
```

Remove `pub mod nodejs;` from `runtime/mod.rs`. Remove all `NodeJs` references from `plugin_loader.rs` and `types/plugins.rs`.

- [ ] **Step 2: Run `cargo check -p alephcore` and fix compilation errors**

Fix each error by removing dead code paths that referenced Node.js runtime.

- [ ] **Step 3: Commit**

```bash
git commit -m "extension: remove deprecated Node.js IPC runtime (~1,026 lines)"
```

- [ ] **Step 4: Remove legacy manifest formats and old auto_discover**

```bash
rm -rf src/extension/manifest/aleph_plugin_toml/
rm src/extension/manifest/aleph_plugin.rs
rm src/extension/manifest/package_json.rs
rm src/extension/manifest/auto_discover.rs
```

Update `manifest/mod.rs` to remove their imports and fallback chain entries. Update `manifest/types.rs` to remove `use super::aleph_plugin_toml::*` imports. The old `auto_discover.rs` is replaced by `adapters/auto_discover.rs` (created in Task 5).

- [ ] **Step 5: Run `cargo check -p alephcore` and fix compilation errors**

The `PluginManifest` struct in `types.rs` may reference types from `aleph_plugin_toml`. Remove or inline these. Fields like `CapabilitiesSection`, `PermissionsSection` etc. that were only used by the V2 TOML format — if still needed by CC adapter, move them; otherwise delete.

- [ ] **Step 6: Commit**

```bash
git commit -m "extension: remove deprecated manifest formats (aleph_plugin_toml, aleph_plugin, package_json — ~2,389 lines)"
```

- [ ] **Step 7: Remove content_loader**

**IMPORTANT**: This step MUST be done after Task 11 (ExtensionManager simplification) is complete, since ExtensionManager currently uses ContentLoader. If executing tasks in order, this step should be done as part of Task 11 cleanup or after Task 11 commits.

```bash
rm -rf src/extension/content_loader/
```

Remove `mod content_loader;` and `pub use content_loader::*;` from `src/extension/mod.rs`. Fix all compilation errors — callers of `ContentLoader` methods should now use `AdapterRegistry` or `parsers.rs` directly.

- [ ] **Step 8: Run `cargo check -p alephcore` and fix compilation errors**

This may cascade into `mod.rs` (ExtensionManager), `sync_api.rs`, `skill_ops.rs`. Fix each caller.

- [ ] **Step 9: Commit**

```bash
git commit -m "extension: remove content_loader, logic migrated to manifest/parsers.rs (~1,125 lines)"
```

- [ ] **Step 10: Clean up `runtime/mod.rs`**

If `runtime/mod.rs` only dispatched between Node.js and WASM, and Node.js is gone, simplify or remove it. WASM can be accessed directly via `runtime::wasm`.

- [ ] **Step 11: Run `cargo check -p alephcore`**

Expected: Compiles cleanly.

- [ ] **Step 12: Commit**

```bash
git commit -m "extension: clean up runtime dispatcher (~812 lines removed)"
```

---

## Task 9: Rewrite `plugin_loader.rs` → `loader.rs`

**Files:**
- Rename: `src/extension/plugin_loader.rs` → `src/extension/loader.rs`
- Modify: `src/extension/mod.rs` (update module declaration)

- [ ] **Step 1: Rename and strip Node.js code**

```bash
mv src/extension/plugin_loader.rs src/extension/loader.rs
```

Remove all Node.js-related fields, methods, and imports from `loader.rs`. The file should only contain WASM and MCP loading logic (~400 lines).

- [ ] **Step 2: Update `mod.rs` module declaration**

Change `mod plugin_loader;` to `mod loader;` and update `pub use`.

- [ ] **Step 3: Run `cargo check -p alephcore` and fix**

- [ ] **Step 4: Commit**

```bash
git commit -m "extension: rename plugin_loader → loader, remove Node.js code (~378 lines)"
```

---

## Task 10: Rewrite `manifest/mod.rs` as AdapterRegistry Entry

**Files:**
- Modify: `src/extension/manifest/mod.rs` (rewrite from 738 → ~100 lines)

- [ ] **Step 1: Rewrite `manifest/mod.rs`**

Replace the 738-line fallback chain with a thin module that:
1. Re-exports `adapter`, `adapters`, `parsers`, `types`
2. Exposes `AdapterRegistry::with_defaults()` as the primary entry point
3. Keeps any still-needed utility functions (frontmatter parsing etc.)

- [ ] **Step 2: Update callers**

Find all callers of `parse_manifest_from_dir()` / `parse_manifest_from_dir_sync()` and migrate them to `AdapterRegistry::with_defaults().parse_dir()`.

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git commit -m "extension: rewrite manifest/mod.rs as AdapterRegistry entry point (~600 lines removed)"
```

---

## Task 11: ExtensionManager Simplification

**Files:**
- Modify: `src/extension/mod.rs` (simplify ExtensionManager struct and load flow)
- Modify: `src/extension/sync_api.rs` (delegate to PluginRegistry)
- Modify: `src/extension/skill_ops.rs` (delegate to PluginRegistry)

- [ ] **Step 0: Add `PluginRecord::from_adapter_output` constructor**

Add to `src/extension/types/plugins.rs`:

```rust
impl PluginRecord {
    pub fn from_adapter_output(output: &AdapterOutput) -> Self {
        Self {
            id: output.plugin_id.clone(),
            name: output.name.clone().unwrap_or_else(|| output.plugin_id.clone()),
            version: output.version.clone(),
            description: output.description.clone(),
            kind: PluginKind::Static, // overridden later if MCP/WASM
            origin: output.source.origin.clone(),
            status: PluginStatus::Loaded,
            root_dir: PathBuf::new(), // set by caller
            tool_names: vec![],
            hook_count: 0,
            channel_ids: vec![],
            provider_ids: vec![],
            gateway_methods: vec![],
            service_ids: vec![],
        }
    }
}
```

- [ ] **Step 1: Remove skills/commands/agents HashMaps from ExtensionManager**

Remove fields:
```rust
skills: Arc<RwLock<HashMap<String, ExtensionSkill>>>,
commands: Arc<RwLock<HashMap<String, ExtensionCommand>>>,
agents: Arc<RwLock<HashMap<String, ExtensionAgent>>>,
```

Add `adapter_registry: AdapterRegistry` field.

- [ ] **Step 2: Rewrite `load_all_extensions` to use AdapterRegistry + CapabilityApi**

The new flow:
```rust
pub async fn load_all(&self) -> Result<LoadSummary> {
    let discovered = self.discovery.discover_all()?;
    let mut registry = self.plugin_registry.write().unwrap_or_else(|e| e.into_inner());
    registry.clear();

    for plugin_dir in discovered.active_plugins() {
        let output = self.adapter_registry.parse_dir(plugin_dir.path())?;
        let record = PluginRecord::from_adapter_output(&output);
        registry.register_plugin(record);

        let mut api = CapabilityApi::new(&output.plugin_id, &mut registry, &output.source.permissions);
        for cap in output.capabilities {
            api.register_capability(cap)?;
        }
    }
    Ok(summary)
}
```

- [ ] **Step 3: Update query methods to delegate to PluginRegistry**

```rust
pub fn get_skills(&self) -> Vec<SkillRegistration> {
    let reg = self.plugin_registry.read().unwrap_or_else(|e| e.into_inner());
    reg.list_skills().into_iter().cloned().collect()
}
```

- [ ] **Step 4: Update `sync_api.rs`**

`SyncExtensionManager` methods that accessed `self.inner.skills` etc. now delegate through `self.inner.plugin_registry`.

- [ ] **Step 5: Update `skill_ops.rs`**

Skill install/delete operations write to PluginRegistry for plugin-provided skills.

- [ ] **Step 6: Run `cargo check -p alephcore`**

Fix all remaining compilation errors.

- [ ] **Step 7: Run `cargo test -p alephcore --lib`**

Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git commit -m "extension: simplify ExtensionManager — skills/commands/agents moved to PluginRegistry"
```

---

## Task 12: Hot Reload RPC + Integration Test

**Files:**
- Modify: `src/extension/mod.rs` (add `reload_plugin` method)
- Modify: `src/gateway/handlers/plugins/handlers.rs` (add `plugin.reload` RPC)

- [ ] **Step 1: Add `reload_plugin` to ExtensionManager**

```rust
pub async fn reload_plugin(&self, plugin_id: &str) -> Result<()> {
    let mut registry = self.plugin_registry.write().unwrap_or_else(|e| e.into_inner());
    let root_dir = registry.get_plugin(plugin_id)
        .map(|p| p.root_dir.clone())
        .ok_or_else(|| anyhow!("Plugin not found: {}", plugin_id))?;

    registry.unregister_plugin(plugin_id);

    let output = self.adapter_registry.parse_dir(&root_dir)?;
    let record = PluginRecord::from_adapter_output(&output);
    registry.register_plugin(record);

    let mut api = CapabilityApi::new(&output.plugin_id, &mut registry, &output.source.permissions);
    for cap in output.capabilities {
        api.register_capability(cap)?;
    }

    tracing::info!(plugin = plugin_id, "Hot-reloaded successfully");
    Ok(())
}
```

- [ ] **Step 2: Add `plugin.reload` RPC handler**

In `gateway/handlers/plugins/handlers.rs`, add a `handle_reload` function that calls `extension_manager.reload_plugin(id)`.

Then in `src/gateway/handlers/mod.rs`, register the new handler alongside other `plugin.*` handlers:
```rust
registry.register("plugin.reload", handle_plugin_reload);
```

- [ ] **Step 3: Run `cargo check -p alephcore`**

- [ ] **Step 4: Commit**

```bash
git commit -m "extension: add plugin.reload hot-reload RPC method"
```

---

## Task 13: Final Verification & Cleanup

- [ ] **Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -W clippy::all
```

Fix any warnings.

- [ ] **Step 3: Verify module structure matches spec**

Run `find src/extension/ -name "*.rs" | sort` and compare against the spec's module tree.

- [ ] **Step 4: Verify line count reduction**

Run `find src/extension/ -name "*.rs" -exec wc -l {} + | tail -1`
Expected: Total should be ~17,500-18,500 (down from ~22,300).

- [ ] **Step 5: Update architecture doc comment in `mod.rs`**

Update the ASCII art diagram at the top of `src/extension/mod.rs` to reflect the new architecture:

```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        ExtensionManager                                │
//! │  - Orchestrates discovery, adapter parsing, registration               │
//! └────────────────────────────┬───────────────────────────────────────────┘
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          ▼                   ▼                   ▼
//!    AdapterRegistry      PluginRegistry      PluginLoader
//!   (CC/Codex/Cursor)   (unified registry)   (WASM + MCP)
//!          │                   ▲                   │
//!          └─── CapabilityApi ─┤── CapabilityApi ──┘
//!                              │
//!                       HookExecutor
```

- [ ] **Step 6: Final commit**

```bash
git commit -m "extension: capability-driven plugin architecture — final cleanup and verification"
```
