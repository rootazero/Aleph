# Plugin Extension System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a hybrid WASM + Node.js plugin system with 4-layer discovery, enabling full Moltbot compatibility.

**Architecture:** ExtensionManager orchestrates three plugin runtimes (WASM/Extism, Node.js/IPC, Static/MD) through a unified PluginRegistry. Discovery scans 4 priority layers, LoaderManager routes to appropriate runtime, and PluginApi trait provides consistent registration interface.

**Tech Stack:** Rust, Extism (WASM), tokio (async), JSON-RPC 2.0, serde/schemars (serialization), Node.js (IPC host)

---

## Phase 1: Infrastructure

### Task 1: Add Dependencies to Cargo.toml

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add new dependencies**

```toml
# Add to [dependencies] section
extism = { version = "1.7", optional = true }
schemars = "0.8"

# Add to [features] section
plugin-wasm = ["extism"]
plugin-nodejs = []
plugin-all = ["plugin-wasm", "plugin-nodejs"]
```

**Step 2: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo check --features plugin-wasm`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "feat(extension): add extism and schemars dependencies"
```

---

### Task 2: Define PluginOrigin and PluginKind Enums

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Write test for PluginOrigin priority**

Add to `core/src/extension/types.rs` at the end:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_origin_priority() {
        assert!(PluginOrigin::Config.priority() > PluginOrigin::Workspace.priority());
        assert!(PluginOrigin::Workspace.priority() > PluginOrigin::Global.priority());
        assert!(PluginOrigin::Global.priority() > PluginOrigin::Bundled.priority());
    }

    #[test]
    fn test_plugin_kind_detection() {
        assert_eq!(PluginKind::detect_from_path(Path::new("plugin.wasm")), Some(PluginKind::Wasm));
        assert_eq!(PluginKind::detect_from_path(Path::new("package.json")), Some(PluginKind::NodeJs));
        assert_eq!(PluginKind::detect_from_path(Path::new("SKILL.md")), Some(PluginKind::Static));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::types::tests`
Expected: FAIL - PluginOrigin and PluginKind not defined

**Step 3: Implement PluginOrigin and PluginKind**

Add before the tests section in `core/src/extension/types.rs`:

```rust
use std::path::Path;

/// Plugin discovery origin with priority (higher = takes precedence)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginOrigin {
    /// Config-specified paths (priority 4 - highest)
    Config,
    /// Workspace local .aether/extensions/ (priority 3)
    Workspace,
    /// Global ~/.aleph/extensions/ (priority 2)
    Global,
    /// Bundled with binary (priority 1 - lowest)
    Bundled,
}

impl PluginOrigin {
    /// Returns priority value (higher = takes precedence in conflicts)
    pub fn priority(&self) -> u8 {
        match self {
            PluginOrigin::Config => 4,
            PluginOrigin::Workspace => 3,
            PluginOrigin::Global => 2,
            PluginOrigin::Bundled => 1,
        }
    }
}

/// Plugin runtime kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// WASM plugin (Extism runtime)
    Wasm,
    /// Node.js/TypeScript plugin (IPC runtime)
    NodeJs,
    /// Static markdown-based plugin (existing Skills/Commands/Agents)
    Static,
}

impl PluginKind {
    /// Detect plugin kind from file path
    pub fn detect_from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str());

        match (filename, ext) {
            (_, Some("wasm")) => Some(PluginKind::Wasm),
            ("package.json", _) => Some(PluginKind::NodeJs),
            ("aether.plugin.json", _) => Some(PluginKind::Wasm), // standalone WASM manifest
            ("SKILL.md" | "COMMAND.md" | "AGENT.md", _) => Some(PluginKind::Static),
            (_, Some("md")) => Some(PluginKind::Static),
            _ => None,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::types::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/extension/types.rs
git commit -m "feat(extension): add PluginOrigin and PluginKind enums"
```

---

### Task 3: Define PluginStatus and PluginRecord

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Write test for PluginRecord**

Add to the tests module in `core/src/extension/types.rs`:

```rust
    #[test]
    fn test_plugin_record_creation() {
        let record = PluginRecord::new(
            "test-plugin".to_string(),
            "Test Plugin".to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        );
        assert_eq!(record.id, "test-plugin");
        assert_eq!(record.status, PluginStatus::Loaded);
        assert!(record.tool_names.is_empty());
    }

    #[test]
    fn test_plugin_status_is_active() {
        assert!(PluginStatus::Loaded.is_active());
        assert!(!PluginStatus::Disabled.is_active());
        assert!(!PluginStatus::Overridden.is_active());
        assert!(!PluginStatus::Error("test".to_string()).is_active());
    }
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::types::tests::test_plugin_record`
Expected: FAIL - PluginRecord not defined

**Step 3: Implement PluginStatus and PluginRecord**

Add after PluginKind in `core/src/extension/types.rs`:

```rust
/// Plugin loading status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    /// Successfully loaded and active
    Loaded,
    /// Disabled by user configuration
    Disabled,
    /// Overridden by higher priority plugin with same ID
    Overridden,
    /// Failed to load with error message
    Error(String),
}

impl PluginStatus {
    /// Returns true if plugin is active and usable
    pub fn is_active(&self) -> bool {
        matches!(self, PluginStatus::Loaded)
    }
}

/// Record of a loaded plugin with its registrations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    /// Unique plugin identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Version string
    pub version: Option<String>,
    /// Description
    pub description: Option<String>,
    /// Runtime kind
    pub kind: PluginKind,
    /// Discovery origin
    pub origin: PluginOrigin,
    /// Current status
    pub status: PluginStatus,
    /// Error message if status is Error
    pub error: Option<String>,
    /// Root directory path
    pub root_dir: PathBuf,

    // Registration tracking
    /// Names of tools registered by this plugin
    pub tool_names: Vec<String>,
    /// Number of hooks registered
    pub hook_count: usize,
    /// IDs of channels registered
    pub channel_ids: Vec<String>,
    /// IDs of providers registered
    pub provider_ids: Vec<String>,
    /// Names of gateway methods registered
    pub gateway_methods: Vec<String>,
    /// IDs of services registered
    pub service_ids: Vec<String>,
}

impl PluginRecord {
    /// Create a new plugin record with default values
    pub fn new(id: String, name: String, kind: PluginKind, origin: PluginOrigin) -> Self {
        Self {
            id,
            name,
            version: None,
            description: None,
            kind,
            origin,
            status: PluginStatus::Loaded,
            error: None,
            root_dir: PathBuf::new(),
            tool_names: Vec::new(),
            hook_count: 0,
            channel_ids: Vec::new(),
            provider_ids: Vec::new(),
            gateway_methods: Vec::new(),
            service_ids: Vec::new(),
        }
    }

    /// Set the plugin as errored
    pub fn with_error(mut self, error: String) -> Self {
        self.status = PluginStatus::Error(error.clone());
        self.error = Some(error);
        self
    }

    /// Set the root directory
    pub fn with_root_dir(mut self, path: PathBuf) -> Self {
        self.root_dir = path;
        self
    }
}
```

Also add this import at the top of the file if not present:
```rust
use std::path::PathBuf;
```

**Step 4: Run test to verify it passes**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::types::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/extension/types.rs
git commit -m "feat(extension): add PluginStatus and PluginRecord types"
```

---

### Task 4: Define Registration Types (9 types)

**Files:**
- Create: `core/src/extension/registry/types.rs`
- Create: `core/src/extension/registry/mod.rs`

**Step 1: Create registry module directory**

Run: `mkdir -p /Volumes/TBU4/Workspace/Aether/core/src/extension/registry`

**Step 2: Write registration types**

Create `core/src/extension/registry/types.rs`:

```rust
//! Plugin registration type definitions
//!
//! Defines the 9 registration types that plugins can provide:
//! - P0 Core: Tools, Hooks
//! - P1 Important: Channels, Providers, GatewayMethods
//! - P2 Useful: HttpRoutes, HttpHandlers, Cli, Services
//! - P3 Optional: Commands

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// JSON Schema representation for tool parameters
pub type JsonSchema = JsonValue;

/// Tool registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRegistration {
    /// Unique tool name
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for parameters
    pub parameters: JsonSchema,
    /// Handler identifier (WASM: function name, Node.js: method name)
    pub handler: String,
    /// Plugin that registered this tool
    pub plugin_id: String,
}

/// Hook event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    BeforeAgentStart,
    AgentEnd,
    BeforeToolCall,
    AfterToolCall,
    ToolResultPersist,
    MessageReceived,
    MessageSending,
    MessageSent,
    SessionStart,
    SessionEnd,
    BeforeCompaction,
    AfterCompaction,
    GatewayStart,
    GatewayStop,
}

/// Hook registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRegistration {
    /// Event to hook into
    pub event: HookEvent,
    /// Execution priority (lower = earlier)
    pub priority: i32,
    /// Handler identifier
    pub handler: String,
    /// Optional name for debugging
    pub name: Option<String>,
    /// Optional description
    pub description: Option<String>,
    /// Plugin that registered this hook
    pub plugin_id: String,
}

/// Channel registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRegistration {
    /// Unique channel ID (e.g., "telegram", "discord")
    pub id: String,
    /// Display label
    pub label: String,
    /// Documentation path
    pub docs_path: Option<String>,
    /// Short description
    pub blurb: Option<String>,
    /// System image/icon identifier
    pub system_image: Option<String>,
    /// Alternative names
    pub aliases: Vec<String>,
    /// Display order
    pub order: i32,
    /// Plugin that registered this channel
    pub plugin_id: String,
}

/// Provider registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRegistration {
    /// Unique provider ID (e.g., "openai", "anthropic")
    pub id: String,
    /// Display name
    pub name: String,
    /// Supported model patterns
    pub models: Vec<String>,
    /// Plugin that registered this provider
    pub plugin_id: String,
}

/// Gateway RPC method registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayMethodRegistration {
    /// Method name (e.g., "custom.myMethod")
    pub method: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Handler identifier
    pub handler: String,
    /// Plugin that registered this method
    pub plugin_id: String,
}

/// HTTP route registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRouteRegistration {
    /// Route path (e.g., "/webhook")
    pub path: String,
    /// HTTP methods (GET, POST, etc.)
    pub methods: Vec<String>,
    /// Handler identifier
    pub handler: String,
    /// Plugin that registered this route
    pub plugin_id: String,
}

/// HTTP handler registration (catch-all) from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHandlerRegistration {
    /// Handler identifier
    pub handler: String,
    /// Priority for handler ordering
    pub priority: i32,
    /// Plugin that registered this handler
    pub plugin_id: String,
}

/// CLI command registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliRegistration {
    /// Command name (e.g., "my-command")
    pub name: String,
    /// Description for help text
    pub description: String,
    /// Handler identifier
    pub handler: String,
    /// Plugin that registered this command
    pub plugin_id: String,
}

/// Background service registration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRegistration {
    /// Unique service ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Start handler identifier
    pub start_handler: String,
    /// Stop handler identifier
    pub stop_handler: String,
    /// Plugin that registered this service
    pub plugin_id: String,
}

/// Simple command registration (non-LLM) from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRegistration {
    /// Command name
    pub name: String,
    /// Description
    pub description: String,
    /// Handler identifier
    pub handler: String,
    /// Plugin that registered this command
    pub plugin_id: String,
}

/// Plugin diagnostic message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    /// Severity level
    pub level: DiagnosticLevel,
    /// Message content
    pub message: String,
    /// Associated plugin ID (if any)
    pub plugin_id: Option<String>,
    /// Source file path (if any)
    pub source: Option<String>,
}

/// Diagnostic severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Warn,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registration() {
        let tool = ToolRegistration {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handle_my_tool".to_string(),
            plugin_id: "test-plugin".to_string(),
        };
        assert_eq!(tool.name, "my_tool");
    }

    #[test]
    fn test_hook_event_serialization() {
        let event = HookEvent::BeforeToolCall;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"before_tool_call\"");
    }
}
```

**Step 3: Create registry module**

Create `core/src/extension/registry/mod.rs`:

```rust
//! Plugin Registry Module
//!
//! Manages registration of all plugin-provided capabilities.

mod types;

pub use types::*;
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::registry::types::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/extension/registry/
git commit -m "feat(extension): add 9 registration types for plugin API"
```

---

### Task 5: Implement PluginRegistry

**Files:**
- Create: `core/src/extension/registry/registry.rs`
- Modify: `core/src/extension/registry/mod.rs`

**Step 1: Write test for PluginRegistry**

Add to `core/src/extension/registry/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::types::{PluginKind, PluginOrigin, PluginRecord};

    #[test]
    fn test_registry_register_plugin() {
        let mut registry = PluginRegistry::new();
        let record = PluginRecord::new(
            "test-plugin".to_string(),
            "Test".to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        );
        registry.register_plugin(record);
        assert!(registry.get_plugin("test-plugin").is_some());
    }

    #[test]
    fn test_registry_register_tool() {
        let mut registry = PluginRegistry::new();
        let tool = ToolRegistration {
            name: "my_tool".to_string(),
            description: "Test".to_string(),
            parameters: serde_json::json!({}),
            handler: "handler".to_string(),
            plugin_id: "test".to_string(),
        };
        registry.register_tool(tool);
        assert!(registry.get_tool("my_tool").is_some());
        assert_eq!(registry.list_tools().len(), 1);
    }

    #[test]
    fn test_registry_hooks_sorted_by_priority() {
        let mut registry = PluginRegistry::new();
        registry.register_hook(HookRegistration {
            event: HookEvent::BeforeToolCall,
            priority: 10,
            handler: "h1".to_string(),
            name: None,
            description: None,
            plugin_id: "p1".to_string(),
        });
        registry.register_hook(HookRegistration {
            event: HookEvent::BeforeToolCall,
            priority: 5,
            handler: "h2".to_string(),
            name: None,
            description: None,
            plugin_id: "p2".to_string(),
        });

        let hooks = registry.get_hooks_for_event(HookEvent::BeforeToolCall);
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].handler, "h2"); // priority 5 first
        assert_eq!(hooks[1].handler, "h1"); // priority 10 second
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::registry::tests`
Expected: FAIL - PluginRegistry not defined

**Step 3: Implement PluginRegistry**

Create `core/src/extension/registry/registry.rs`:

```rust
//! Plugin Registry Implementation
//!
//! Central storage for all plugin registrations.

use std::collections::HashMap;

use super::types::*;
use crate::extension::types::{PluginRecord, PluginStatus};

/// Central registry for all plugin registrations
#[derive(Debug, Default)]
pub struct PluginRegistry {
    /// Plugin metadata indexed by ID
    plugins: HashMap<String, PluginRecord>,

    /// Tools indexed by name
    tools: HashMap<String, ToolRegistration>,

    /// Hooks sorted by priority
    hooks: Vec<HookRegistration>,

    /// Channels indexed by ID
    channels: HashMap<String, ChannelRegistration>,

    /// Providers indexed by ID
    providers: HashMap<String, ProviderRegistration>,

    /// Gateway methods indexed by method name
    gateway_methods: HashMap<String, GatewayMethodRegistration>,

    /// HTTP routes
    http_routes: Vec<HttpRouteRegistration>,

    /// HTTP handlers sorted by priority
    http_handlers: Vec<HttpHandlerRegistration>,

    /// CLI commands indexed by name
    cli_commands: HashMap<String, CliRegistration>,

    /// Services indexed by ID
    services: HashMap<String, ServiceRegistration>,

    /// Simple commands indexed by name
    commands: HashMap<String, CommandRegistration>,

    /// Diagnostic messages
    diagnostics: Vec<PluginDiagnostic>,
}

impl PluginRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all registrations
    pub fn clear(&mut self) {
        self.plugins.clear();
        self.tools.clear();
        self.hooks.clear();
        self.channels.clear();
        self.providers.clear();
        self.gateway_methods.clear();
        self.http_routes.clear();
        self.http_handlers.clear();
        self.cli_commands.clear();
        self.services.clear();
        self.commands.clear();
        self.diagnostics.clear();
    }

    // === Plugin Management ===

    /// Register a plugin record
    pub fn register_plugin(&mut self, record: PluginRecord) {
        self.plugins.insert(record.id.clone(), record);
    }

    /// Get a plugin by ID
    pub fn get_plugin(&self, id: &str) -> Option<&PluginRecord> {
        self.plugins.get(id)
    }

    /// Get a mutable plugin by ID
    pub fn get_plugin_mut(&mut self, id: &str) -> Option<&mut PluginRecord> {
        self.plugins.get_mut(id)
    }

    /// List all plugins
    pub fn list_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins.values().collect()
    }

    /// List active (loaded) plugins only
    pub fn list_active_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins
            .values()
            .filter(|p| p.status.is_active())
            .collect()
    }

    /// Disable a plugin by ID
    pub fn disable_plugin(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.status = PluginStatus::Disabled;
            true
        } else {
            false
        }
    }

    /// Enable a plugin by ID
    pub fn enable_plugin(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.status = PluginStatus::Loaded;
            true
        } else {
            false
        }
    }

    // === Tool Registration ===

    /// Register a tool
    pub fn register_tool(&mut self, tool: ToolRegistration) {
        // Track in plugin record
        if let Some(plugin) = self.plugins.get_mut(&tool.plugin_id) {
            plugin.tool_names.push(tool.name.clone());
        }
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Get a tool by name
    pub fn get_tool(&self, name: &str) -> Option<&ToolRegistration> {
        self.tools.get(name)
    }

    /// List all tools
    pub fn list_tools(&self) -> Vec<&ToolRegistration> {
        self.tools.values().collect()
    }

    // === Hook Registration ===

    /// Register a hook (maintains priority ordering)
    pub fn register_hook(&mut self, hook: HookRegistration) {
        // Track in plugin record
        if let Some(plugin) = self.plugins.get_mut(&hook.plugin_id) {
            plugin.hook_count += 1;
        }
        self.hooks.push(hook);
        // Sort by priority (lower priority value = earlier execution)
        self.hooks.sort_by_key(|h| h.priority);
    }

    /// Get hooks for a specific event
    pub fn get_hooks_for_event(&self, event: HookEvent) -> Vec<&HookRegistration> {
        self.hooks
            .iter()
            .filter(|h| h.event == event)
            .collect()
    }

    // === Channel Registration ===

    /// Register a channel
    pub fn register_channel(&mut self, channel: ChannelRegistration) {
        if let Some(plugin) = self.plugins.get_mut(&channel.plugin_id) {
            plugin.channel_ids.push(channel.id.clone());
        }
        self.channels.insert(channel.id.clone(), channel);
    }

    /// Get a channel by ID
    pub fn get_channel(&self, id: &str) -> Option<&ChannelRegistration> {
        self.channels.get(id)
    }

    /// List all channels
    pub fn list_channels(&self) -> Vec<&ChannelRegistration> {
        let mut channels: Vec<_> = self.channels.values().collect();
        channels.sort_by_key(|c| c.order);
        channels
    }

    // === Provider Registration ===

    /// Register a provider
    pub fn register_provider(&mut self, provider: ProviderRegistration) {
        if let Some(plugin) = self.plugins.get_mut(&provider.plugin_id) {
            plugin.provider_ids.push(provider.id.clone());
        }
        self.providers.insert(provider.id.clone(), provider);
    }

    /// Get a provider by ID
    pub fn get_provider(&self, id: &str) -> Option<&ProviderRegistration> {
        self.providers.get(id)
    }

    /// List all providers
    pub fn list_providers(&self) -> Vec<&ProviderRegistration> {
        self.providers.values().collect()
    }

    // === Gateway Method Registration ===

    /// Register a gateway method
    pub fn register_gateway_method(&mut self, method: GatewayMethodRegistration) {
        if let Some(plugin) = self.plugins.get_mut(&method.plugin_id) {
            plugin.gateway_methods.push(method.method.clone());
        }
        self.gateway_methods.insert(method.method.clone(), method);
    }

    /// Get a gateway method by name
    pub fn get_gateway_method(&self, method: &str) -> Option<&GatewayMethodRegistration> {
        self.gateway_methods.get(method)
    }

    /// List all gateway methods
    pub fn list_gateway_methods(&self) -> Vec<&GatewayMethodRegistration> {
        self.gateway_methods.values().collect()
    }

    // === HTTP Registration ===

    /// Register an HTTP route
    pub fn register_http_route(&mut self, route: HttpRouteRegistration) {
        self.http_routes.push(route);
    }

    /// List all HTTP routes
    pub fn list_http_routes(&self) -> &[HttpRouteRegistration] {
        &self.http_routes
    }

    /// Register an HTTP handler
    pub fn register_http_handler(&mut self, handler: HttpHandlerRegistration) {
        self.http_handlers.push(handler);
        self.http_handlers.sort_by_key(|h| h.priority);
    }

    /// List all HTTP handlers (sorted by priority)
    pub fn list_http_handlers(&self) -> &[HttpHandlerRegistration] {
        &self.http_handlers
    }

    // === CLI Registration ===

    /// Register a CLI command
    pub fn register_cli(&mut self, cli: CliRegistration) {
        self.cli_commands.insert(cli.name.clone(), cli);
    }

    /// Get a CLI command by name
    pub fn get_cli(&self, name: &str) -> Option<&CliRegistration> {
        self.cli_commands.get(name)
    }

    /// List all CLI commands
    pub fn list_cli_commands(&self) -> Vec<&CliRegistration> {
        self.cli_commands.values().collect()
    }

    // === Service Registration ===

    /// Register a service
    pub fn register_service(&mut self, service: ServiceRegistration) {
        if let Some(plugin) = self.plugins.get_mut(&service.plugin_id) {
            plugin.service_ids.push(service.id.clone());
        }
        self.services.insert(service.id.clone(), service);
    }

    /// Get a service by ID
    pub fn get_service(&self, id: &str) -> Option<&ServiceRegistration> {
        self.services.get(id)
    }

    /// List all services
    pub fn list_services(&self) -> Vec<&ServiceRegistration> {
        self.services.values().collect()
    }

    // === Command Registration ===

    /// Register a simple command
    pub fn register_command(&mut self, command: CommandRegistration) {
        self.commands.insert(command.name.clone(), command);
    }

    /// Get a command by name
    pub fn get_command(&self, name: &str) -> Option<&CommandRegistration> {
        self.commands.get(name)
    }

    /// List all commands
    pub fn list_commands(&self) -> Vec<&CommandRegistration> {
        self.commands.values().collect()
    }

    // === Diagnostics ===

    /// Add a diagnostic message
    pub fn add_diagnostic(&mut self, diagnostic: PluginDiagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Get all diagnostics
    pub fn diagnostics(&self) -> &[PluginDiagnostic] {
        &self.diagnostics
    }

    /// Get diagnostics for a specific plugin
    pub fn diagnostics_for_plugin(&self, plugin_id: &str) -> Vec<&PluginDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.plugin_id.as_deref() == Some(plugin_id))
            .collect()
    }

    // === Unregistration ===

    /// Unregister all items from a specific plugin
    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        // Remove tools
        self.tools.retain(|_, t| t.plugin_id != plugin_id);

        // Remove hooks
        self.hooks.retain(|h| h.plugin_id != plugin_id);

        // Remove channels
        self.channels.retain(|_, c| c.plugin_id != plugin_id);

        // Remove providers
        self.providers.retain(|_, p| p.plugin_id != plugin_id);

        // Remove gateway methods
        self.gateway_methods.retain(|_, m| m.plugin_id != plugin_id);

        // Remove HTTP routes
        self.http_routes.retain(|r| r.plugin_id != plugin_id);

        // Remove HTTP handlers
        self.http_handlers.retain(|h| h.plugin_id != plugin_id);

        // Remove CLI commands
        self.cli_commands.retain(|_, c| c.plugin_id != plugin_id);

        // Remove services
        self.services.retain(|_, s| s.plugin_id != plugin_id);

        // Remove commands
        self.commands.retain(|_, c| c.plugin_id != plugin_id);

        // Remove plugin record
        self.plugins.remove(plugin_id);
    }
}
```

**Step 4: Update registry mod.rs**

Update `core/src/extension/registry/mod.rs`:

```rust
//! Plugin Registry Module
//!
//! Manages registration of all plugin-provided capabilities.

mod types;
mod registry;

pub use types::*;
pub use registry::PluginRegistry;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::types::{PluginKind, PluginOrigin, PluginRecord};

    #[test]
    fn test_registry_register_plugin() {
        let mut registry = PluginRegistry::new();
        let record = PluginRecord::new(
            "test-plugin".to_string(),
            "Test".to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        );
        registry.register_plugin(record);
        assert!(registry.get_plugin("test-plugin").is_some());
    }

    #[test]
    fn test_registry_register_tool() {
        let mut registry = PluginRegistry::new();
        let tool = ToolRegistration {
            name: "my_tool".to_string(),
            description: "Test".to_string(),
            parameters: serde_json::json!({}),
            handler: "handler".to_string(),
            plugin_id: "test".to_string(),
        };
        registry.register_tool(tool);
        assert!(registry.get_tool("my_tool").is_some());
        assert_eq!(registry.list_tools().len(), 1);
    }

    #[test]
    fn test_registry_hooks_sorted_by_priority() {
        let mut registry = PluginRegistry::new();
        registry.register_hook(HookRegistration {
            event: HookEvent::BeforeToolCall,
            priority: 10,
            handler: "h1".to_string(),
            name: None,
            description: None,
            plugin_id: "p1".to_string(),
        });
        registry.register_hook(HookRegistration {
            event: HookEvent::BeforeToolCall,
            priority: 5,
            handler: "h2".to_string(),
            name: None,
            description: None,
            plugin_id: "p2".to_string(),
        });

        let hooks = registry.get_hooks_for_event(HookEvent::BeforeToolCall);
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].handler, "h2"); // priority 5 first
        assert_eq!(hooks[1].handler, "h1"); // priority 10 second
    }

    #[test]
    fn test_registry_unregister_plugin() {
        let mut registry = PluginRegistry::new();
        let record = PluginRecord::new(
            "test".to_string(),
            "Test".to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        );
        registry.register_plugin(record);
        registry.register_tool(ToolRegistration {
            name: "tool1".to_string(),
            description: "".to_string(),
            parameters: serde_json::json!({}),
            handler: "".to_string(),
            plugin_id: "test".to_string(),
        });

        assert!(registry.get_plugin("test").is_some());
        assert!(registry.get_tool("tool1").is_some());

        registry.unregister_plugin("test");

        assert!(registry.get_plugin("test").is_none());
        assert!(registry.get_tool("tool1").is_none());
    }
}
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::registry`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/extension/registry/
git commit -m "feat(extension): implement PluginRegistry with all registration methods"
```

---

### Task 6: Define PluginManifest and Parsing

**Files:**
- Create: `core/src/extension/manifest/mod.rs`
- Create: `core/src/extension/manifest/types.rs`
- Create: `core/src/extension/manifest/package_json.rs`
- Create: `core/src/extension/manifest/aether_plugin.rs`

**Step 1: Create manifest module directory**

Run: `mkdir -p /Volumes/TBU4/Workspace/Aether/core/src/extension/manifest`

**Step 2: Create manifest types**

Create `core/src/extension/manifest/types.rs`:

```rust
//! Unified plugin manifest types

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::extension::types::PluginKind;

/// UI hint for a config field
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigUiHint {
    /// Display label
    pub label: Option<String>,
    /// Help text
    pub help: Option<String>,
    /// Whether this is an advanced setting
    pub advanced: Option<bool>,
    /// Whether the value is sensitive (password, API key)
    pub sensitive: Option<bool>,
    /// Placeholder text
    pub placeholder: Option<String>,
}

/// Plugin permission requirement
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginPermission {
    /// Network access
    Network,
    /// Read filesystem access
    #[serde(rename = "filesystem:read")]
    FilesystemRead,
    /// Write filesystem access
    #[serde(rename = "filesystem:write")]
    FilesystemWrite,
    /// Full filesystem access
    Filesystem,
    /// Environment variable access
    Env,
    /// Unknown/custom permission
    #[serde(untagged)]
    Custom(String),
}

/// Unified plugin manifest structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Version string
    #[serde(default)]
    pub version: Option<String>,
    /// Description
    #[serde(default)]
    pub description: Option<String>,
    /// Runtime kind
    pub kind: PluginKind,
    /// Entry point (relative to plugin root)
    pub entry: PathBuf,
    /// Root directory of the plugin
    #[serde(skip)]
    pub root_dir: PathBuf,
    /// JSON Schema for plugin configuration
    #[serde(default)]
    pub config_schema: Option<JsonValue>,
    /// UI hints for config fields
    #[serde(default)]
    pub config_ui_hints: HashMap<String, ConfigUiHint>,
    /// Required permissions (for WASM sandbox)
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
    /// Author information
    #[serde(default)]
    pub author: Option<AuthorInfo>,
    /// Homepage URL
    #[serde(default)]
    pub homepage: Option<String>,
    /// Repository URL
    #[serde(default)]
    pub repository: Option<String>,
    /// License identifier
    #[serde(default)]
    pub license: Option<String>,
    /// Keywords/tags
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Extension entry points (for Node.js plugins with multiple)
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// Author information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorInfo {
    pub name: Option<String>,
    pub email: Option<String>,
    pub url: Option<String>,
}

impl PluginManifest {
    /// Create a minimal manifest
    pub fn new(id: String, name: String, kind: PluginKind, entry: PathBuf) -> Self {
        Self {
            id,
            name,
            version: None,
            description: None,
            kind,
            entry,
            root_dir: PathBuf::new(),
            config_schema: None,
            config_ui_hints: HashMap::new(),
            permissions: Vec::new(),
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: Vec::new(),
            extensions: Vec::new(),
        }
    }

    /// Set the root directory
    pub fn with_root_dir(mut self, root: PathBuf) -> Self {
        self.root_dir = root;
        self
    }

    /// Get the absolute entry path
    pub fn entry_path(&self) -> PathBuf {
        if self.entry.is_absolute() {
            self.entry.clone()
        } else {
            self.root_dir.join(&self.entry)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_entry_path() {
        let manifest = PluginManifest::new(
            "test".to_string(),
            "Test".to_string(),
            PluginKind::Wasm,
            PathBuf::from("plugin.wasm"),
        ).with_root_dir(PathBuf::from("/plugins/test"));

        assert_eq!(manifest.entry_path(), PathBuf::from("/plugins/test/plugin.wasm"));
    }
}
```

**Step 3: Create package.json parser**

Create `core/src/extension/manifest/package_json.rs`:

```rust
//! package.json parser for Node.js plugins

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

use super::types::{AuthorInfo, ConfigUiHint, PluginManifest};
use crate::extension::error::ExtensionError;
use crate::extension::types::PluginKind;

/// package.json structure with aether extension
#[derive(Debug, Clone, Deserialize)]
pub struct PackageJson {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub author: Option<PackageAuthor>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<PackageRepository>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    /// Aleph-specific extension configuration
    #[serde(default)]
    pub aether: Option<AetherPackageConfig>,
}

/// Author field can be string or object
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PackageAuthor {
    String(String),
    Object {
        name: Option<String>,
        email: Option<String>,
        url: Option<String>,
    },
}

impl PackageAuthor {
    pub fn to_author_info(&self) -> AuthorInfo {
        match self {
            PackageAuthor::String(s) => AuthorInfo {
                name: Some(s.clone()),
                email: None,
                url: None,
            },
            PackageAuthor::Object { name, email, url } => AuthorInfo {
                name: name.clone(),
                email: email.clone(),
                url: url.clone(),
            },
        }
    }
}

/// Repository field can be string or object
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PackageRepository {
    String(String),
    Object {
        #[serde(rename = "type")]
        repo_type: Option<String>,
        url: String,
    },
}

impl PackageRepository {
    pub fn url(&self) -> &str {
        match self {
            PackageRepository::String(s) => s,
            PackageRepository::Object { url, .. } => url,
        }
    }
}

/// Aleph-specific configuration in package.json
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlephPackageConfig {
    /// Extension entry points
    #[serde(default)]
    pub extensions: Vec<String>,
    /// JSON Schema for plugin configuration
    #[serde(default)]
    pub config_schema: Option<JsonValue>,
    /// UI hints for config fields
    #[serde(default)]
    pub config_ui_hints: HashMap<String, ConfigUiHint>,
}

/// Parse package.json and convert to PluginManifest
pub fn parse_package_json(path: &Path) -> Result<PluginManifest, ExtensionError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ExtensionError::Io(e.to_string()))?;

    let pkg: PackageJson = serde_json::from_str(&content)
        .map_err(|e| ExtensionError::InvalidManifest(format!("Invalid package.json: {}", e)))?;

    // Must have aether config to be a plugin
    let aether = pkg.aether.ok_or_else(|| {
        ExtensionError::InvalidManifest("Missing 'aether' field in package.json".to_string())
    })?;

    // Determine entry point
    let entry = if !aether.extensions.is_empty() {
        aether.extensions[0].clone()
    } else if let Some(main) = &pkg.main {
        main.clone()
    } else {
        "index.js".to_string()
    };

    // Extract plugin ID from package name (remove scope)
    let id = extract_plugin_id(&pkg.name);

    let root_dir = path.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    Ok(PluginManifest {
        id,
        name: pkg.name.clone(),
        version: pkg.version,
        description: pkg.description,
        kind: PluginKind::NodeJs,
        entry: entry.into(),
        root_dir,
        config_schema: aether.config_schema,
        config_ui_hints: aether.config_ui_hints,
        permissions: Vec::new(), // Node.js plugins don't use WASM permissions
        author: pkg.author.map(|a| a.to_author_info()),
        homepage: pkg.homepage,
        repository: pkg.repository.map(|r| r.url().to_string()),
        license: pkg.license,
        keywords: pkg.keywords.unwrap_or_default(),
        extensions: aether.extensions,
    })
}

/// Extract plugin ID from npm package name
/// @scope/package-name -> package-name
/// package-name -> package-name
fn extract_plugin_id(name: &str) -> String {
    if let Some(idx) = name.find('/') {
        name[idx + 1..].to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_plugin_id() {
        assert_eq!(extract_plugin_id("@aether/my-plugin"), "my-plugin");
        assert_eq!(extract_plugin_id("simple-plugin"), "simple-plugin");
    }
}
```

**Step 4: Create aether.plugin.json parser**

Create `core/src/extension/manifest/aether_plugin.rs`:

```rust
//! aether.plugin.json parser for WASM/standalone plugins

use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

use super::types::{AuthorInfo, ConfigUiHint, PluginManifest, PluginPermission};
use crate::extension::error::ExtensionError;
use crate::extension::types::PluginKind;

/// aether.plugin.json structure
#[derive(Debug, Clone, Deserialize)]
pub struct AlephPluginJson {
    /// Unique plugin identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Version string
    #[serde(default)]
    pub version: Option<String>,
    /// Description
    #[serde(default)]
    pub description: Option<String>,
    /// Plugin kind (wasm or nodejs)
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Entry point file
    pub entry: String,
    /// JSON Schema for plugin configuration
    #[serde(default, rename = "configSchema")]
    pub config_schema: Option<JsonValue>,
    /// UI hints for config fields
    #[serde(default, rename = "configUiHints")]
    pub config_ui_hints: HashMap<String, ConfigUiHint>,
    /// Required permissions
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
    /// Author information
    #[serde(default)]
    pub author: Option<AuthorInfo>,
    /// Homepage URL
    #[serde(default)]
    pub homepage: Option<String>,
    /// Repository URL
    #[serde(default)]
    pub repository: Option<String>,
    /// License identifier
    #[serde(default)]
    pub license: Option<String>,
    /// Keywords/tags
    #[serde(default)]
    pub keywords: Vec<String>,
}

fn default_kind() -> String {
    "wasm".to_string()
}

/// Parse aether.plugin.json and convert to PluginManifest
pub fn parse_aether_plugin_json(path: &Path) -> Result<PluginManifest, ExtensionError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ExtensionError::Io(e.to_string()))?;

    let plugin: AlephPluginJson = serde_json::from_str(&content)
        .map_err(|e| ExtensionError::InvalidManifest(format!("Invalid aether.plugin.json: {}", e)))?;

    // Validate plugin ID
    validate_plugin_id(&plugin.id)?;

    // Determine kind
    let kind = match plugin.kind.as_str() {
        "wasm" => PluginKind::Wasm,
        "nodejs" | "node" => PluginKind::NodeJs,
        _ => return Err(ExtensionError::InvalidManifest(
            format!("Unknown plugin kind: {}", plugin.kind)
        )),
    };

    let root_dir = path.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    Ok(PluginManifest {
        id: plugin.id,
        name: plugin.name,
        version: plugin.version,
        description: plugin.description,
        kind,
        entry: plugin.entry.into(),
        root_dir,
        config_schema: plugin.config_schema,
        config_ui_hints: plugin.config_ui_hints,
        permissions: plugin.permissions,
        author: plugin.author,
        homepage: plugin.homepage,
        repository: plugin.repository,
        license: plugin.license,
        keywords: plugin.keywords,
        extensions: Vec::new(),
    })
}

/// Validate plugin ID format
fn validate_plugin_id(id: &str) -> Result<(), ExtensionError> {
    if id.is_empty() {
        return Err(ExtensionError::InvalidManifest("Plugin ID cannot be empty".to_string()));
    }

    let first = id.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(ExtensionError::InvalidManifest(
            "Plugin ID must start with a lowercase letter".to_string()
        ));
    }

    for c in id.chars() {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
            return Err(ExtensionError::InvalidManifest(
                format!("Plugin ID contains invalid character: {}", c)
            ));
        }
    }

    if id.contains("--") {
        return Err(ExtensionError::InvalidManifest(
            "Plugin ID cannot contain consecutive hyphens".to_string()
        ));
    }

    if id.starts_with('-') || id.ends_with('-') {
        return Err(ExtensionError::InvalidManifest(
            "Plugin ID cannot start or end with a hyphen".to_string()
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_plugin_id_valid() {
        assert!(validate_plugin_id("my-plugin").is_ok());
        assert!(validate_plugin_id("plugin123").is_ok());
        assert!(validate_plugin_id("a").is_ok());
    }

    #[test]
    fn test_validate_plugin_id_invalid() {
        assert!(validate_plugin_id("").is_err());
        assert!(validate_plugin_id("123plugin").is_err());
        assert!(validate_plugin_id("My-Plugin").is_err());
        assert!(validate_plugin_id("my--plugin").is_err());
        assert!(validate_plugin_id("-plugin").is_err());
        assert!(validate_plugin_id("plugin-").is_err());
    }
}
```

**Step 5: Create manifest module**

Create `core/src/extension/manifest/mod.rs`:

```rust
//! Plugin Manifest Parsing
//!
//! Supports multiple manifest formats:
//! - package.json with "aether" field (Node.js plugins)
//! - aether.plugin.json (WASM/standalone plugins)

mod types;
mod package_json;
mod aether_plugin;

pub use types::*;
pub use package_json::parse_package_json;
pub use aether_plugin::parse_aether_plugin_json;

use std::path::Path;
use crate::extension::error::ExtensionError;

/// Parse manifest from a directory, auto-detecting format
pub fn parse_manifest_from_dir(dir: &Path) -> Result<PluginManifest, ExtensionError> {
    // Try aether.plugin.json first
    let aether_manifest = dir.join("aether.plugin.json");
    if aether_manifest.exists() {
        return parse_aether_plugin_json(&aether_manifest);
    }

    // Try package.json with aether field
    let package_json = dir.join("package.json");
    if package_json.exists() {
        return parse_package_json(&package_json);
    }

    Err(ExtensionError::InvalidManifest(
        format!("No valid manifest found in {:?}", dir)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_parse_manifest_aether_plugin_json() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("aether.plugin.json");
        fs::write(&manifest_path, r#"{
            "id": "test-plugin",
            "name": "Test Plugin",
            "version": "1.0.0",
            "entry": "plugin.wasm"
        }"#).unwrap();

        let manifest = parse_manifest_from_dir(dir.path()).unwrap();
        assert_eq!(manifest.id, "test-plugin");
        assert_eq!(manifest.kind, crate::extension::types::PluginKind::Wasm);
    }

    #[test]
    fn test_parse_manifest_package_json() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("package.json");
        fs::write(&manifest_path, r#"{
            "name": "@aether/test-plugin",
            "version": "1.0.0",
            "aether": {
                "extensions": ["src/index.ts"]
            }
        }"#).unwrap();

        let manifest = parse_manifest_from_dir(dir.path()).unwrap();
        assert_eq!(manifest.id, "test-plugin");
        assert_eq!(manifest.kind, crate::extension::types::PluginKind::NodeJs);
    }
}
```

**Step 6: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::manifest`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/extension/manifest/
git commit -m "feat(extension): add manifest parsing for package.json and aether.plugin.json"
```

---

### Task 7: Implement Discovery System

**Files:**
- Create: `core/src/extension/discovery/mod.rs`
- Create: `core/src/extension/discovery/scanner.rs`
- Create: `core/src/extension/discovery/resolver.rs`

**Step 1: Create discovery module directory**

Run: `mkdir -p /Volumes/TBU4/Workspace/Aether/core/src/extension/discovery`

**Step 2: Create scanner**

Create `core/src/extension/discovery/scanner.rs`:

```rust
//! Directory scanning for plugin discovery

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use crate::extension::error::ExtensionError;
use crate::extension::manifest::{parse_manifest_from_dir, PluginManifest};
use crate::extension::types::{PluginKind, PluginOrigin};

/// A discovered plugin candidate
#[derive(Debug, Clone)]
pub struct PluginCandidate {
    /// Plugin ID
    pub id: String,
    /// Entry file path
    pub source: PathBuf,
    /// Plugin root directory
    pub root_dir: PathBuf,
    /// Discovery origin
    pub origin: PluginOrigin,
    /// Plugin kind
    pub kind: PluginKind,
    /// Parsed manifest
    pub manifest: PluginManifest,
}

/// Scan a directory for plugins
pub fn scan_directory(
    dir: &Path,
    origin: PluginOrigin,
) -> Vec<Result<PluginCandidate, ExtensionError>> {
    let mut results = Vec::new();

    if !dir.exists() {
        debug!("Plugin directory does not exist: {:?}", dir);
        return results;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Failed to read plugin directory {:?}: {}", dir, e);
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip hidden files/directories
        if path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }

        if path.is_dir() {
            // Try to parse as plugin directory
            match scan_plugin_dir(&path, origin) {
                Ok(Some(candidate)) => results.push(Ok(candidate)),
                Ok(None) => {} // Not a plugin directory
                Err(e) => results.push(Err(e)),
            }
        } else if path.is_file() {
            // Check for standalone files (WASM, MD)
            if let Some(candidate) = scan_standalone_file(&path, origin) {
                results.push(Ok(candidate));
            }
        }
    }

    results
}

/// Scan a single directory as a potential plugin
fn scan_plugin_dir(
    dir: &Path,
    origin: PluginOrigin,
) -> Result<Option<PluginCandidate>, ExtensionError> {
    // Try manifest-based plugins first
    if let Ok(manifest) = parse_manifest_from_dir(dir) {
        return Ok(Some(PluginCandidate {
            id: manifest.id.clone(),
            source: manifest.entry_path(),
            root_dir: dir.to_path_buf(),
            origin,
            kind: manifest.kind,
            manifest,
        }));
    }

    // Check for static plugins (SKILL.md, COMMAND.md, AGENT.md)
    for filename in ["SKILL.md", "COMMAND.md", "AGENT.md"] {
        let md_path = dir.join(filename);
        if md_path.exists() {
            let id = dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            return Ok(Some(PluginCandidate {
                id: id.clone(),
                source: md_path.clone(),
                root_dir: dir.to_path_buf(),
                origin,
                kind: PluginKind::Static,
                manifest: PluginManifest::new(
                    id.clone(),
                    id,
                    PluginKind::Static,
                    md_path.file_name().unwrap().into(),
                ).with_root_dir(dir.to_path_buf()),
            }));
        }
    }

    Ok(None)
}

/// Scan a standalone file as a potential plugin
fn scan_standalone_file(path: &Path, origin: PluginOrigin) -> Option<PluginCandidate> {
    let kind = PluginKind::detect_from_path(path)?;

    // Only process WASM and MD files as standalone
    if !matches!(kind, PluginKind::Wasm | PluginKind::Static) {
        return None;
    }

    let id = path.file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let root_dir = path.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    Some(PluginCandidate {
        id: id.clone(),
        source: path.to_path_buf(),
        root_dir: root_dir.clone(),
        origin,
        kind,
        manifest: PluginManifest::new(
            id.clone(),
            id,
            kind,
            path.file_name().unwrap().into(),
        ).with_root_dir(root_dir),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_scan_directory_with_wasm_plugin() {
        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("aether.plugin.json"), r#"{
            "id": "my-plugin",
            "name": "My Plugin",
            "entry": "plugin.wasm"
        }"#).unwrap();

        let results = scan_directory(dir.path(), PluginOrigin::Global);
        assert_eq!(results.len(), 1);
        let candidate = results[0].as_ref().unwrap();
        assert_eq!(candidate.id, "my-plugin");
        assert_eq!(candidate.kind, PluginKind::Wasm);
    }

    #[test]
    fn test_scan_directory_with_static_skill() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# My Skill\n\nContent").unwrap();

        let results = scan_directory(dir.path(), PluginOrigin::Global);
        assert_eq!(results.len(), 1);
        let candidate = results[0].as_ref().unwrap();
        assert_eq!(candidate.id, "my-skill");
        assert_eq!(candidate.kind, PluginKind::Static);
    }
}
```

**Step 3: Create resolver**

Create `core/src/extension/discovery/resolver.rs`:

```rust
//! Plugin conflict resolution based on origin priority

use std::collections::HashMap;
use tracing::info;

use super::scanner::PluginCandidate;

/// Resolve plugin conflicts based on origin priority
///
/// When multiple plugins have the same ID, the one with highest
/// priority origin wins. Others are marked for override tracking.
pub fn resolve_conflicts(candidates: Vec<PluginCandidate>) -> ResolvedPlugins {
    let mut by_id: HashMap<String, Vec<PluginCandidate>> = HashMap::new();

    // Group by ID
    for candidate in candidates {
        by_id.entry(candidate.id.clone())
            .or_default()
            .push(candidate);
    }

    let mut active = Vec::new();
    let mut overridden = Vec::new();

    for (id, mut group) in by_id {
        if group.len() == 1 {
            active.push(group.pop().unwrap());
        } else {
            // Sort by priority (highest first)
            group.sort_by(|a, b| b.origin.priority().cmp(&a.origin.priority()));

            let winner = group.remove(0);
            info!(
                "Plugin '{}' from {:?} overrides {} other(s)",
                id,
                winner.origin,
                group.len()
            );

            active.push(winner);
            overridden.extend(group);
        }
    }

    ResolvedPlugins { active, overridden }
}

/// Result of conflict resolution
#[derive(Debug)]
pub struct ResolvedPlugins {
    /// Plugins that should be loaded
    pub active: Vec<PluginCandidate>,
    /// Plugins that were overridden by higher priority
    pub overridden: Vec<PluginCandidate>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::manifest::PluginManifest;
    use crate::extension::types::{PluginKind, PluginOrigin};
    use std::path::PathBuf;

    fn make_candidate(id: &str, origin: PluginOrigin) -> PluginCandidate {
        PluginCandidate {
            id: id.to_string(),
            source: PathBuf::new(),
            root_dir: PathBuf::new(),
            origin,
            kind: PluginKind::Static,
            manifest: PluginManifest::new(
                id.to_string(),
                id.to_string(),
                PluginKind::Static,
                PathBuf::new(),
            ),
        }
    }

    #[test]
    fn test_resolve_no_conflicts() {
        let candidates = vec![
            make_candidate("a", PluginOrigin::Global),
            make_candidate("b", PluginOrigin::Workspace),
        ];

        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active.len(), 2);
        assert_eq!(resolved.overridden.len(), 0);
    }

    #[test]
    fn test_resolve_with_conflict() {
        let candidates = vec![
            make_candidate("same", PluginOrigin::Bundled),
            make_candidate("same", PluginOrigin::Workspace),
            make_candidate("same", PluginOrigin::Global),
        ];

        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active.len(), 1);
        assert_eq!(resolved.overridden.len(), 2);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Workspace);
    }

    #[test]
    fn test_resolve_config_highest_priority() {
        let candidates = vec![
            make_candidate("plugin", PluginOrigin::Config),
            make_candidate("plugin", PluginOrigin::Workspace),
        ];

        let resolved = resolve_conflicts(candidates);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Config);
    }
}
```

**Step 4: Create discovery module**

Create `core/src/extension/discovery/mod.rs`:

```rust
//! Plugin Discovery System
//!
//! Implements 4-layer discovery with priority-based conflict resolution:
//! 1. Config-specified paths (highest)
//! 2. Workspace local
//! 3. Global user-level
//! 4. Bundled (lowest)

mod scanner;
mod resolver;

pub use scanner::{scan_directory, PluginCandidate};
pub use resolver::{resolve_conflicts, ResolvedPlugins};

use std::path::PathBuf;
use tracing::debug;

use crate::extension::error::ExtensionError;
use crate::extension::types::PluginOrigin;

/// Discovery manager configuration
#[derive(Debug, Clone, Default)]
pub struct DiscoveryConfig {
    /// Extra paths from config (Priority 1)
    pub extra_paths: Vec<PathBuf>,
    /// Workspace root directory
    pub workspace_dir: Option<PathBuf>,
    /// User home directory override
    pub home_dir: Option<PathBuf>,
    /// Bundled plugins directory
    pub bundled_dir: Option<PathBuf>,
}

/// Discover all plugins from all configured sources
pub fn discover_all(config: &DiscoveryConfig) -> Result<ResolvedPlugins, ExtensionError> {
    let mut all_candidates = Vec::new();

    // Priority 1: Config-specified paths
    for path in &config.extra_paths {
        debug!("Scanning config path: {:?}", path);
        let results = scan_directory(path, PluginOrigin::Config);
        for result in results {
            match result {
                Ok(candidate) => all_candidates.push(candidate),
                Err(e) => debug!("Error scanning {:?}: {}", path, e),
            }
        }
    }

    // Priority 2: Workspace local
    if let Some(workspace) = &config.workspace_dir {
        for subdir in [".aether/extensions", ".claude/extensions"] {
            let path = workspace.join(subdir);
            debug!("Scanning workspace path: {:?}", path);
            let results = scan_directory(&path, PluginOrigin::Workspace);
            for result in results {
                match result {
                    Ok(candidate) => all_candidates.push(candidate),
                    Err(e) => debug!("Error scanning {:?}: {}", path, e),
                }
            }
        }
    }

    // Priority 3: Global user-level
    let home = config.home_dir.clone()
        .or_else(|| dirs::home_dir())
        .unwrap_or_default();

    for subdir in [".aether/extensions", ".claude/extensions"] {
        let path = home.join(subdir);
        debug!("Scanning global path: {:?}", path);
        let results = scan_directory(&path, PluginOrigin::Global);
        for result in results {
            match result {
                Ok(candidate) => all_candidates.push(candidate),
                Err(e) => debug!("Error scanning {:?}: {}", path, e),
            }
        }
    }

    // Priority 4: Bundled
    if let Some(bundled) = &config.bundled_dir {
        debug!("Scanning bundled path: {:?}", bundled);
        let results = scan_directory(bundled, PluginOrigin::Bundled);
        for result in results {
            match result {
                Ok(candidate) => all_candidates.push(candidate),
                Err(e) => debug!("Error scanning {:?}: {}", bundled, e),
            }
        }
    }

    // Resolve conflicts
    Ok(resolve_conflicts(all_candidates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_discover_all_layers() {
        let home = tempdir().unwrap();
        let workspace = tempdir().unwrap();

        // Create global plugin
        let global_ext = home.path().join(".aether/extensions/global-plugin");
        fs::create_dir_all(&global_ext).unwrap();
        fs::write(global_ext.join("SKILL.md"), "# Global").unwrap();

        // Create workspace plugin
        let ws_ext = workspace.path().join(".aether/extensions/ws-plugin");
        fs::create_dir_all(&ws_ext).unwrap();
        fs::write(ws_ext.join("SKILL.md"), "# Workspace").unwrap();

        let config = DiscoveryConfig {
            workspace_dir: Some(workspace.path().to_path_buf()),
            home_dir: Some(home.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert_eq!(resolved.active.len(), 2);
    }

    #[test]
    fn test_discover_workspace_overrides_global() {
        let home = tempdir().unwrap();
        let workspace = tempdir().unwrap();

        // Create same-named plugin in both locations
        let global_ext = home.path().join(".aether/extensions/same-plugin");
        fs::create_dir_all(&global_ext).unwrap();
        fs::write(global_ext.join("SKILL.md"), "# Global").unwrap();

        let ws_ext = workspace.path().join(".aether/extensions/same-plugin");
        fs::create_dir_all(&ws_ext).unwrap();
        fs::write(ws_ext.join("SKILL.md"), "# Workspace").unwrap();

        let config = DiscoveryConfig {
            workspace_dir: Some(workspace.path().to_path_buf()),
            home_dir: Some(home.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = discover_all(&config).unwrap();
        assert_eq!(resolved.active.len(), 1);
        assert_eq!(resolved.overridden.len(), 1);
        assert_eq!(resolved.active[0].origin, PluginOrigin::Workspace);
    }
}
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::discovery`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/extension/discovery/
git commit -m "feat(extension): implement 4-layer plugin discovery with conflict resolution"
```

---

### Task 8: Wire Up New Modules to Extension System

**Files:**
- Modify: `core/src/extension/mod.rs`
- Modify: `core/src/extension/types.rs` (add exports)

**Step 1: Update extension/mod.rs to export new modules**

Add these module declarations and re-exports to `core/src/extension/mod.rs`:

```rust
// Add near the top with other module declarations
pub mod discovery;
pub mod manifest;
pub mod registry;

// Add to public exports
pub use discovery::{DiscoveryConfig, PluginCandidate, discover_all};
pub use manifest::PluginManifest;
pub use registry::{PluginRegistry, ToolRegistration, HookRegistration, HookEvent};
pub use types::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};
```

**Step 2: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo check`
Expected: Compiles successfully (may have warnings)

**Step 3: Commit**

```bash
git add core/src/extension/mod.rs core/src/extension/types.rs
git commit -m "feat(extension): wire up discovery, manifest, and registry modules"
```

---

## Phase 1 Complete Checkpoint

At this point, Phase 1 infrastructure is complete:
- PluginOrigin, PluginKind, PluginStatus, PluginRecord types
- 9 registration types (Tools, Hooks, Channels, Providers, etc.)
- PluginRegistry with full CRUD operations
- Dual manifest parsing (package.json, aether.plugin.json)
- 4-layer discovery system with conflict resolution

**Verify all tests pass:**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension`
Expected: All tests PASS

---

## Phase 2: WASM Runtime (Extism)

### Task 9: Create WASM Runtime Module Structure

**Files:**
- Create: `core/src/extension/runtime/wasm/mod.rs`
- Create: `core/src/extension/runtime/wasm/permissions.rs`
- Modify: `core/src/extension/runtime/mod.rs`

**Step 1: Create wasm directory**

Run: `mkdir -p /Volumes/TBU4/Workspace/Aether/core/src/extension/runtime/wasm`

**Step 2: Create permissions module**

Create `core/src/extension/runtime/wasm/permissions.rs`:

```rust
//! WASM plugin permission checking

use std::collections::HashSet;
use crate::extension::manifest::PluginPermission;

/// Permission checker for WASM plugins
#[derive(Debug, Clone, Default)]
pub struct PermissionChecker {
    allowed: HashSet<PluginPermission>,
}

impl PermissionChecker {
    /// Create a new permission checker with the given permissions
    pub fn new(permissions: Vec<PluginPermission>) -> Self {
        Self {
            allowed: permissions.into_iter().collect(),
        }
    }

    /// Check if network access is allowed
    pub fn can_network(&self) -> bool {
        self.allowed.contains(&PluginPermission::Network)
    }

    /// Check if filesystem read is allowed
    pub fn can_read_filesystem(&self) -> bool {
        self.allowed.contains(&PluginPermission::FilesystemRead)
            || self.allowed.contains(&PluginPermission::Filesystem)
    }

    /// Check if filesystem write is allowed
    pub fn can_write_filesystem(&self) -> bool {
        self.allowed.contains(&PluginPermission::FilesystemWrite)
            || self.allowed.contains(&PluginPermission::Filesystem)
    }

    /// Check if environment access is allowed
    pub fn can_access_env(&self) -> bool {
        self.allowed.contains(&PluginPermission::Env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_permissions() {
        let checker = PermissionChecker::new(vec![]);
        assert!(!checker.can_network());
        assert!(!checker.can_read_filesystem());
        assert!(!checker.can_write_filesystem());
    }

    #[test]
    fn test_network_permission() {
        let checker = PermissionChecker::new(vec![PluginPermission::Network]);
        assert!(checker.can_network());
        assert!(!checker.can_read_filesystem());
    }

    #[test]
    fn test_filesystem_permission() {
        let checker = PermissionChecker::new(vec![PluginPermission::Filesystem]);
        assert!(checker.can_read_filesystem());
        assert!(checker.can_write_filesystem());
    }
}
```

**Step 3: Create WASM runtime module**

Create `core/src/extension/runtime/wasm/mod.rs`:

```rust
//! WASM Plugin Runtime using Extism
//!
//! Provides sandboxed execution of WASM plugins with permission-based
//! access to host functions.

mod permissions;

pub use permissions::PermissionChecker;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use crate::extension::error::ExtensionError;
use crate::extension::manifest::PluginManifest;

#[cfg(feature = "plugin-wasm")]
use extism::{Manifest as ExtismManifest, Plugin, Wasm, UserData, CurrentPlugin};

/// Input for WASM tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolInput {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Output from WASM tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolOutput {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// WASM plugin runtime manager
#[derive(Default)]
pub struct WasmRuntime {
    #[cfg(feature = "plugin-wasm")]
    plugins: HashMap<String, LoadedWasmPlugin>,
    #[cfg(not(feature = "plugin-wasm"))]
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(feature = "plugin-wasm")]
struct LoadedWasmPlugin {
    plugin: Plugin,
    manifest: PluginManifest,
    permissions: PermissionChecker,
}

impl WasmRuntime {
    /// Create a new WASM runtime
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a WASM plugin
    #[cfg(feature = "plugin-wasm")]
    pub fn load_plugin(&mut self, manifest: &PluginManifest) -> Result<(), ExtensionError> {
        let wasm_path = manifest.entry_path();

        if !wasm_path.exists() {
            return Err(ExtensionError::Io(
                format!("WASM file not found: {:?}", wasm_path)
            ));
        }

        info!("Loading WASM plugin: {} from {:?}", manifest.id, wasm_path);

        let extism_manifest = ExtismManifest::new([Wasm::file(&wasm_path)]);

        let plugin = Plugin::new(&extism_manifest, [], true)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to load WASM: {}", e)))?;

        let loaded = LoadedWasmPlugin {
            plugin,
            manifest: manifest.clone(),
            permissions: PermissionChecker::new(manifest.permissions.clone()),
        };

        self.plugins.insert(manifest.id.clone(), loaded);

        Ok(())
    }

    #[cfg(not(feature = "plugin-wasm"))]
    pub fn load_plugin(&mut self, manifest: &PluginManifest) -> Result<(), ExtensionError> {
        Err(ExtensionError::Runtime(
            "WASM runtime not enabled. Compile with --features plugin-wasm".to_string()
        ))
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, plugin_id: &str) -> bool {
        #[cfg(feature = "plugin-wasm")]
        {
            self.plugins.remove(plugin_id).is_some()
        }
        #[cfg(not(feature = "plugin-wasm"))]
        {
            false
        }
    }

    /// Check if a plugin is loaded
    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        #[cfg(feature = "plugin-wasm")]
        {
            self.plugins.contains_key(plugin_id)
        }
        #[cfg(not(feature = "plugin-wasm"))]
        {
            false
        }
    }

    /// Call a tool handler in a WASM plugin
    #[cfg(feature = "plugin-wasm")]
    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        handler: &str,
        input: WasmToolInput,
    ) -> Result<WasmToolOutput, ExtensionError> {
        let loaded = self.plugins.get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        let input_json = serde_json::to_string(&input)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to serialize input: {}", e)))?;

        debug!("Calling WASM handler '{}' with input: {}", handler, input_json);

        let result = loaded.plugin.call::<&str, &str>(handler, &input_json)
            .map_err(|e| ExtensionError::Runtime(format!("WASM call failed: {}", e)))?;

        let output: WasmToolOutput = serde_json::from_str(result)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to parse output: {}", e)))?;

        Ok(output)
    }

    #[cfg(not(feature = "plugin-wasm"))]
    pub fn call_tool(
        &mut self,
        _plugin_id: &str,
        _handler: &str,
        _input: WasmToolInput,
    ) -> Result<WasmToolOutput, ExtensionError> {
        Err(ExtensionError::Runtime(
            "WASM runtime not enabled".to_string()
        ))
    }

    /// Get list of loaded plugin IDs
    pub fn loaded_plugins(&self) -> Vec<String> {
        #[cfg(feature = "plugin-wasm")]
        {
            self.plugins.keys().cloned().collect()
        }
        #[cfg(not(feature = "plugin-wasm"))]
        {
            Vec::new()
        }
    }
}

#[cfg(all(test, feature = "plugin-wasm"))]
mod tests {
    use super::*;
    use crate::extension::types::PluginKind;
    use std::path::PathBuf;

    #[test]
    fn test_wasm_runtime_not_found() {
        let mut runtime = WasmRuntime::new();
        let manifest = PluginManifest::new(
            "test".to_string(),
            "Test".to_string(),
            PluginKind::Wasm,
            PathBuf::from("nonexistent.wasm"),
        );

        let result = runtime.load_plugin(&manifest);
        assert!(result.is_err());
    }
}

#[cfg(all(test, not(feature = "plugin-wasm")))]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_runtime_disabled() {
        let runtime = WasmRuntime::new();
        assert!(runtime.loaded_plugins().is_empty());
    }
}
```

**Step 4: Update runtime/mod.rs**

Update `core/src/extension/runtime/mod.rs` to include wasm module:

```rust
//! Plugin Runtime Systems
//!
//! Provides execution environments for different plugin types:
//! - WASM (Extism) - Sandboxed WebAssembly execution
//! - Node.js (IPC) - JavaScript/TypeScript via subprocess
//! - Static - Markdown-based skills/commands/agents

pub mod wasm;

// Re-export for convenience
pub use wasm::{WasmRuntime, WasmToolInput, WasmToolOutput, PermissionChecker};
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::runtime`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/extension/runtime/
git commit -m "feat(extension): add WASM runtime with Extism and permission system"
```

---

### Task 10: Create Node.js Runtime Module

**Files:**
- Create: `core/src/extension/runtime/nodejs/mod.rs`
- Create: `core/src/extension/runtime/nodejs/ipc.rs`
- Create: `core/src/extension/runtime/nodejs/process.rs`

**Step 1: Create nodejs directory**

Run: `mkdir -p /Volumes/TBU4/Workspace/Aether/core/src/extension/runtime/nodejs`

**Step 2: Create IPC protocol types**

Create `core/src/extension/runtime/nodejs/ipc.rs`:

```rust
//! JSON-RPC 2.0 IPC protocol for Node.js plugins

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// JSON-RPC 2.0 request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: JsonValue,
}

impl JsonRpcRequest {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: impl Into<String>, result: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: impl Into<String>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

/// JSON-RPC 2.0 notification (no id, no response expected)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: JsonValue,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// Plugin registration message from Node.js
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRegistrationParams {
    pub plugin_id: String,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
    #[serde(default)]
    pub channels: Vec<ChannelDefinition>,
    #[serde(default)]
    pub providers: Vec<ProviderDefinition>,
    #[serde(default)]
    pub gateway_methods: Vec<GatewayMethodDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: JsonValue,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub event: String,
    #[serde(default)]
    pub priority: i32,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelDefinition {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDefinition {
    pub id: String,
    pub name: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayMethodDefinition {
    pub method: String,
    pub handler: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let req = JsonRpcRequest::new("1", "plugin.call", serde_json::json!({"foo": "bar"}));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"plugin.call\""));
    }

    #[test]
    fn test_response_success() {
        let resp = JsonRpcResponse::success("1", serde_json::json!({"result": "ok"}));
        assert!(resp.is_success());
    }

    #[test]
    fn test_response_error() {
        let resp = JsonRpcResponse::error("1", -32600, "Invalid request");
        assert!(!resp.is_success());
    }
}
```

**Step 3: Create process manager**

Create `core/src/extension/runtime/nodejs/process.rs`:

```rust
//! Node.js subprocess management

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{debug, error, info, warn};

use super::ipc::{JsonRpcRequest, JsonRpcResponse};
use crate::extension::error::ExtensionError;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request ID
fn next_request_id() -> String {
    format!("req-{}", REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Node.js plugin host process
pub struct NodeProcess {
    child: Child,
    plugin_id: String,
}

impl NodeProcess {
    /// Start a new Node.js plugin host process
    pub fn start(
        node_path: &str,
        host_script: &str,
        plugin_path: &str,
        plugin_id: &str,
    ) -> Result<Self, ExtensionError> {
        info!("Starting Node.js plugin host for: {}", plugin_id);

        let child = Command::new(node_path)
            .arg(host_script)
            .arg(plugin_path)
            .arg(plugin_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ExtensionError::Runtime(
                format!("Failed to start Node.js process: {}", e)
            ))?;

        Ok(Self {
            child,
            plugin_id: plugin_id.to_string(),
        })
    }

    /// Send a request and wait for response
    pub fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<JsonRpcResponse, ExtensionError> {
        let id = next_request_id();
        let request = JsonRpcRequest::new(&id, method, params);

        // Send request
        let stdin = self.child.stdin.as_mut()
            .ok_or_else(|| ExtensionError::Runtime("No stdin".to_string()))?;

        let request_line = serde_json::to_string(&request)
            .map_err(|e| ExtensionError::Runtime(format!("Serialize error: {}", e)))?;

        debug!("Sending to plugin {}: {}", self.plugin_id, request_line);

        writeln!(stdin, "{}", request_line)
            .map_err(|e| ExtensionError::Runtime(format!("Write error: {}", e)))?;
        stdin.flush()
            .map_err(|e| ExtensionError::Runtime(format!("Flush error: {}", e)))?;

        // Read response
        let stdout = self.child.stdout.as_mut()
            .ok_or_else(|| ExtensionError::Runtime("No stdout".to_string()))?;

        let mut reader = BufReader::new(stdout);
        let mut response_line = String::new();

        reader.read_line(&mut response_line)
            .map_err(|e| ExtensionError::Runtime(format!("Read error: {}", e)))?;

        debug!("Received from plugin {}: {}", self.plugin_id, response_line.trim());

        let response: JsonRpcResponse = serde_json::from_str(&response_line)
            .map_err(|e| ExtensionError::Runtime(format!("Parse error: {}", e)))?;

        if response.id != id {
            return Err(ExtensionError::Runtime(
                format!("Response ID mismatch: expected {}, got {}", id, response.id)
            ));
        }

        Ok(response)
    }

    /// Send shutdown signal
    pub fn shutdown(&mut self) -> Result<(), ExtensionError> {
        info!("Shutting down Node.js plugin: {}", self.plugin_id);

        let _ = self.call("shutdown", serde_json::json!({}));

        // Wait briefly for graceful shutdown
        std::thread::sleep(Duration::from_millis(100));

        // Force kill if still running
        if let Err(e) = self.child.kill() {
            warn!("Failed to kill Node.js process: {}", e);
        }

        Ok(())
    }

    /// Check if process is still running
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for NodeProcess {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
```

**Step 4: Create Node.js runtime module**

Create `core/src/extension/runtime/nodejs/mod.rs`:

```rust
//! Node.js Plugin Runtime
//!
//! Manages Node.js subprocess for executing TypeScript/JavaScript plugins
//! via JSON-RPC 2.0 over stdio.

pub mod ipc;
pub mod process;

pub use ipc::*;
pub use process::NodeProcess;

use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, error, info};

use crate::extension::error::ExtensionError;
use crate::extension::manifest::PluginManifest;
use crate::extension::registry::{
    ToolRegistration, HookRegistration, HookEvent,
    ChannelRegistration, ProviderRegistration, GatewayMethodRegistration,
};

/// Node.js runtime manager
pub struct NodeJsRuntime {
    /// Running plugin processes
    processes: HashMap<String, NodeProcess>,
    /// Path to Node.js binary
    node_path: String,
    /// Path to plugin host script
    host_script_path: String,
}

impl NodeJsRuntime {
    /// Create a new Node.js runtime
    pub fn new(node_path: impl Into<String>, host_script_path: impl Into<String>) -> Self {
        Self {
            processes: HashMap::new(),
            node_path: node_path.into(),
            host_script_path: host_script_path.into(),
        }
    }

    /// Load a Node.js plugin
    pub fn load_plugin(&mut self, manifest: &PluginManifest) -> Result<PluginRegistrationParams, ExtensionError> {
        let entry_path = manifest.entry_path();

        if !entry_path.exists() {
            return Err(ExtensionError::Io(
                format!("Plugin entry not found: {:?}", entry_path)
            ));
        }

        info!("Loading Node.js plugin: {} from {:?}", manifest.id, entry_path);

        let mut process = NodeProcess::start(
            &self.node_path,
            &self.host_script_path,
            entry_path.to_str().unwrap_or(""),
            &manifest.id,
        )?;

        // Call load method to get registrations
        let response = process.call("load", serde_json::json!({
            "pluginId": manifest.id,
            "pluginPath": entry_path,
        }))?;

        if !response.is_success() {
            let err = response.error.map(|e| e.message).unwrap_or_default();
            return Err(ExtensionError::Runtime(format!("Plugin load failed: {}", err)));
        }

        let registrations: PluginRegistrationParams = response.result
            .map(|r| serde_json::from_value(r))
            .transpose()
            .map_err(|e| ExtensionError::Runtime(format!("Invalid registration: {}", e)))?
            .unwrap_or_else(|| PluginRegistrationParams {
                plugin_id: manifest.id.clone(),
                tools: vec![],
                hooks: vec![],
                channels: vec![],
                providers: vec![],
                gateway_methods: vec![],
            });

        self.processes.insert(manifest.id.clone(), process);

        Ok(registrations)
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, plugin_id: &str) -> Result<(), ExtensionError> {
        if let Some(mut process) = self.processes.remove(plugin_id) {
            process.shutdown()?;
        }
        Ok(())
    }

    /// Call a tool handler
    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ExtensionError> {
        let process = self.processes.get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        let response = process.call("plugin.call", serde_json::json!({
            "pluginId": plugin_id,
            "handler": handler,
            "args": args,
        }))?;

        if let Some(error) = response.error {
            return Err(ExtensionError::Runtime(error.message));
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// Execute a hook handler
    pub fn execute_hook(
        &mut self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> Result<serde_json::Value, ExtensionError> {
        let process = self.processes.get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        let response = process.call("executeHook", serde_json::json!({
            "pluginId": plugin_id,
            "handler": handler,
            "event": event_data,
        }))?;

        if let Some(error) = response.error {
            return Err(ExtensionError::Runtime(error.message));
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// Check if a plugin is loaded
    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        self.processes.contains_key(plugin_id)
    }

    /// Get list of loaded plugins
    pub fn loaded_plugins(&self) -> Vec<&str> {
        self.processes.keys().map(|s| s.as_str()).collect()
    }

    /// Shutdown all plugins
    pub fn shutdown_all(&mut self) {
        for (id, mut process) in self.processes.drain() {
            if let Err(e) = process.shutdown() {
                error!("Failed to shutdown plugin {}: {}", id, e);
            }
        }
    }
}

impl Drop for NodeJsRuntime {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

/// Convert IPC tool definition to ToolRegistration
pub fn tool_def_to_registration(def: &ToolDefinition, plugin_id: &str) -> ToolRegistration {
    ToolRegistration {
        name: def.name.clone(),
        description: def.description.clone(),
        parameters: def.parameters.clone(),
        handler: def.handler.clone(),
        plugin_id: plugin_id.to_string(),
    }
}

/// Convert IPC hook definition to HookRegistration
pub fn hook_def_to_registration(def: &HookDefinition, plugin_id: &str) -> Option<HookRegistration> {
    let event = match def.event.as_str() {
        "before_agent_start" => HookEvent::BeforeAgentStart,
        "agent_end" => HookEvent::AgentEnd,
        "before_tool_call" => HookEvent::BeforeToolCall,
        "after_tool_call" => HookEvent::AfterToolCall,
        "message_received" => HookEvent::MessageReceived,
        "message_sending" => HookEvent::MessageSending,
        "session_start" => HookEvent::SessionStart,
        "session_end" => HookEvent::SessionEnd,
        _ => return None,
    };

    Some(HookRegistration {
        event,
        priority: def.priority,
        handler: def.handler.clone(),
        name: None,
        description: None,
        plugin_id: plugin_id.to_string(),
    })
}
```

**Step 5: Update runtime/mod.rs**

Update `core/src/extension/runtime/mod.rs`:

```rust
//! Plugin Runtime Systems
//!
//! Provides execution environments for different plugin types:
//! - WASM (Extism) - Sandboxed WebAssembly execution
//! - Node.js (IPC) - JavaScript/TypeScript via subprocess
//! - Static - Markdown-based skills/commands/agents

pub mod wasm;
pub mod nodejs;

// Re-export for convenience
pub use wasm::{WasmRuntime, WasmToolInput, WasmToolOutput, PermissionChecker};
pub use nodejs::{NodeJsRuntime, NodeProcess, JsonRpcRequest, JsonRpcResponse};
```

**Step 6: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension::runtime`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/extension/runtime/
git commit -m "feat(extension): add Node.js runtime with JSON-RPC IPC"
```

---

## Phase 2 Complete Checkpoint

At this point, Phase 2 is complete:
- WASM runtime with Extism (feature-gated)
- Permission system for WASM sandbox
- Node.js runtime with JSON-RPC IPC
- Process management with graceful shutdown

**Verify all tests pass:**

Run: `cd /Volumes/TBU4/Workspace/Aether/core && cargo test --lib extension`
Expected: All tests PASS

---

## Phase 3 & 4: Integration

Phases 3 and 4 involve:
- Creating the plugin-host.js for Node.js
- Integrating runtimes into ExtensionManager
- Adding Gateway RPC handlers (plugins.*)
- CLI commands
- End-to-end tests

These are more complex and require the foundational work from Phases 1-2. The implementation should proceed incrementally with testing at each step.

---

## Summary

This plan covers 10 detailed tasks for Phases 1-2:

| Task | Description | Files |
|------|-------------|-------|
| 1 | Add dependencies | Cargo.toml |
| 2 | PluginOrigin, PluginKind | types.rs |
| 3 | PluginStatus, PluginRecord | types.rs |
| 4 | 9 registration types | registry/types.rs |
| 5 | PluginRegistry | registry/registry.rs |
| 6 | Manifest parsing | manifest/*.rs |
| 7 | Discovery system | discovery/*.rs |
| 8 | Wire up modules | mod.rs |
| 9 | WASM runtime | runtime/wasm/*.rs |
| 10 | Node.js runtime | runtime/nodejs/*.rs |

Each task follows TDD with explicit test-first steps, exact file paths, and commit checkpoints.
