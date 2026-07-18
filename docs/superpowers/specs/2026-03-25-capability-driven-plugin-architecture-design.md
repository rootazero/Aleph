# Capability-Driven Plugin Architecture Design

**Date**: 2026-03-25
**Status**: Draft (v2 — post-review fixes)
**Scope**: `src/extension/` module reorganization + multi-platform plugin compatibility + dynamic registration API

## Summary

Redesign Aleph's plugin system from a Claude Code-only static format reader into a **capability-driven architecture** that:

1. Consumes plugins from any platform (Claude Code, Codex CLI, Cursor) via a trait-based ManifestAdapter system
2. Supports runtime capability registration by MCP/WASM plugins via a CapabilityRegistrar API
3. Cleans up ~4500 lines of deprecated code (Node.js runtime, legacy manifest formats, content_loader)
4. Surpasses OpenClaw in five dimensions: WASM sandboxing, capability tiering, permission model, hot reload, capability inference

## Motivation

**Learning from OpenClaw**: OpenClaw achieves multi-platform plugin compatibility through platform-specific bundle manifests (`.claude-plugin/`, `.codex-plugin/`, `.cursor-plugin/`) and a `definePluginEntry` + `registerXxx` registration API in TypeScript.

**Aleph's current gap**: Only consumes Claude Code format. No dynamic registration API — MCP configs are stored but never wired to the registry; WASM plugins can execute tools but cannot declare capabilities. The PluginRegistry has 12 storage collections (8 HashMaps + 4 Vecs), but only Tool and Hook are actively populated by the current loading pipeline.

**Aleph's existing strengths**: 4-layer discovery with priority resolution, P0-P3 capability tiering, WASM sandboxing via Extism, strong Rust type system. These should be leveraged, not discarded.

## Architecture

### Core Abstraction: Capability Model

All plugin capabilities — whether parsed from a manifest file or registered at runtime by a running process — are represented as `CapabilityDeclaration`:

```rust
// src/extension/capability.rs

#[derive(Debug, Clone)]
pub enum CapabilityDeclaration {
    // P0 Core
    Tool(ToolDeclaration),
    Hook(HookDeclaration),
    Skill(SkillDeclaration),

    // P1 Important
    Command(CommandDeclaration),
    Agent(AgentDeclaration),

    // P2 Pluggable (runtime-only, requires active process)
    Provider(ProviderDeclaration),
    Channel(ChannelDeclaration),
    Service(ServiceDeclaration),
    McpServer(McpServerDeclaration),

    // P3 Gateway Extensions
    GatewayMethod(GatewayMethodDeclaration),
    HttpRoute(HttpRouteDeclaration),
    CliCommand(CliDeclaration),
}

#[derive(Debug, Clone)]
pub struct CapabilitySource {
    pub plugin_id: String,
    pub origin: PluginOrigin,
    pub source_format: SourceFormat,
    pub permissions: Vec<PluginPermission>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceFormat {
    ClaudeCode,
    Codex,
    Cursor,
    AlephNative,
    AutoDiscovered,
    RuntimeMcp,
    RuntimeWasm,
}
```

`CapabilityDeclaration` is an intermediate representation — produced during parse/register, consumed when writing to PluginRegistry, then discarded. The PluginRegistry remains the storage layer.

### Declaration vs Registration Types

Each `*Declaration` type is a **type alias** for the corresponding `*Registration` type in `registry/types.rs`. They are the same struct — the "Declaration" naming is used in capability.rs for semantic clarity (declaring intent) while "Registration" is used in the registry (stored state). Example:

```rust
// src/extension/capability.rs
pub type ToolDeclaration = crate::extension::registry::types::ToolRegistration;
pub type HookDeclaration = crate::extension::registry::types::HookRegistration;
pub type SkillDeclaration = crate::extension::registry::types::SkillRegistration;
pub type CommandDeclaration = crate::extension::registry::types::CommandRegistration;
pub type AgentDeclaration = crate::extension::registry::types::AgentRegistration;
pub type ProviderDeclaration = crate::extension::registry::types::ProviderRegistration;
pub type ChannelDeclaration = crate::extension::registry::types::ChannelRegistration;
pub type ServiceDeclaration = crate::extension::registry::types::ServiceRegistration;
pub type McpServerDeclaration = crate::extension::types::McpServerConfig;
pub type GatewayMethodDeclaration = crate::extension::registry::types::GatewayMethodRegistration;
pub type HttpRouteDeclaration = crate::extension::registry::types::HttpRouteRegistration;
pub type CliDeclaration = crate::extension::registry::types::CliRegistration;
```

This avoids type duplication. If a Declaration needs fields that Registration doesn't have (unlikely), the alias is promoted to a separate struct with `Into<*Registration>` conversion.

### Tier Re-labeling

**Deliberate change**: The existing `registry/types.rs` uses P0=Tool/Hook, P1=Channel/Provider/GatewayMethod, P2=HttpRoute/HttpHandler/Cli/Service, P3=Command. This design re-tiers based on *registration frequency and permission requirements*:

| Tier | New Assignment | Rationale |
|------|---------------|-----------|
| P0 Core | Tool, Hook, Skill | Most common, always allowed, core to all plugins |
| P1 Important | Command, Agent | Common for static plugins, always allowed |
| P2 Pluggable | Provider, Channel, Service, McpServer | Runtime plugins, some permission-gated |
| P3 Gateway Extension | HttpRoute, GatewayMethod, CliCommand | System-level, all permission-gated |

### Two Core Traits

```rust
/// Static path: parse a plugin directory into capability declarations
pub trait ManifestAdapter: Send + Sync {
    fn detect(&self, plugin_dir: &Path) -> bool;
    fn parse(&self, plugin_dir: &Path) -> Result<AdapterOutput>;
    fn format_name(&self) -> &str;
    fn priority(&self) -> i32 { 0 }
}

#[derive(Debug)]
pub struct AdapterOutput {
    pub plugin_id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub capabilities: Vec<CapabilityDeclaration>,
    pub source: CapabilitySource,
}

/// Dynamic path: register capabilities at runtime
pub trait CapabilityRegistrar: Send + Sync {
    fn register(&self, api: &mut CapabilityApi) -> Result<()>;
    fn unregister(&self, api: &mut CapabilityApi) -> Result<()>;
}
```

### Data Flow

```
                 Static Path                    Dynamic Path
          ┌─────────────────────┐       ┌──────────────────────┐
          │  ManifestAdapter    │       │  CapabilityRegistrar  │
          │  .detect() → true   │       │  .register(api)       │
          │  .parse() → output  │       │                       │
          └────────┬────────────┘       └──────────┬───────────┘
                   │ Vec<CapabilityDeclaration>     │ CapabilityApi calls
                   │                                │
                   ▼                                ▼
            ┌──────────────────────────────────────────┐
            │         PluginRegistry (existing)         │
            │  tools / hooks / skills / agents /        │
            │  channels / providers / services / ...    │
            └──────────────────────────────────────────┘
```

## Detailed Design

### 1. ManifestAdapter System

#### AdapterRegistry

```rust
// src/extension/manifest/adapter.rs

pub struct AdapterRegistry {
    adapters: Vec<Box<dyn ManifestAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        let mut registry = Self { adapters: vec![] };
        registry.register(Box::new(ClaudeCodeTomlAdapter));   // priority: 100
        registry.register(Box::new(ClaudeCodeJsonAdapter));   // priority: 90
        registry.register(Box::new(CodexAdapter));            // priority: 80
        registry.register(Box::new(CursorAdapter));           // priority: 70
        registry.register(Box::new(AutoDiscoverAdapter));     // priority: -100
        registry
    }

    pub fn register(&mut self, adapter: Box<dyn ManifestAdapter>) {
        self.adapters.push(adapter);
        self.adapters.sort_by(|a, b| b.priority().cmp(&a.priority()));
    }

    pub fn parse_dir(&self, dir: &Path) -> Result<AdapterOutput> {
        for adapter in &self.adapters {
            if adapter.detect(dir) {
                return adapter.parse(dir);
            }
        }
        Err(anyhow!("No adapter matched directory: {}", dir.display()))
    }
}
```

#### Platform Adapters

**ClaudeCodeTomlAdapter** (priority 100): Detects `.claude-plugin/plugin.toml`. Parses standard CC fields + optional `[aleph]` extensions. Refactored from existing `cc_plugin_toml.rs`.

**ClaudeCodeJsonAdapter** (priority 90): Detects `.claude-plugin/plugin.json`. Parses CC JSON format. Refactored from existing `cc_plugin_json.rs`.

**CodexAdapter** (priority 80): Detects `.codex-plugin/plugin.json` or `AGENTS.md` with Codex markers. Codex supports: skills, hooks, mcpServers, apps. Maps AGENTS.md to Agent capability.

**CursorAdapter** (priority 70): Detects `.cursor-plugin/plugin.json`, `.cursor/rules/`, or `.cursorrules`. Cursor supports: skills, commands, agents, hooks, rules, mcpServers. Maps Cursor rules to Aleph skills.

**AutoDiscoverAdapter** (priority -100): Fallback. Scans for any recognized directory structure (skills/, commands/, agents/, hooks/, .mcp.json). Refactored from existing `auto_discover.rs`.

#### Shared Parsers

All adapters share bottom-level component parsers to avoid code duplication:

```rust
// src/extension/manifest/parsers.rs

pub fn parse_skills_dir(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_commands_dir(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_agents_dir(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_hooks_file(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_mcp_config(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
```

### 2. CapabilityRegistrar & CapabilityApi

#### CapabilityApi

The unified API surface for writing capabilities into PluginRegistry.

**Lifetime strategy**: `CapabilityApi` takes `&mut PluginRegistry` (borrowed) for synchronous callers (static path, WASM host functions). For async callers (MCP registrar), the pattern is **collect-then-batch**: the async code collects all `CapabilityDeclaration`s first, then acquires the lock once and batch-writes via `CapabilityApi`. This avoids holding `RwLockWriteGuard` across await points.

```rust
// src/extension/registrar/api.rs

/// Synchronous API for writing capabilities into PluginRegistry.
/// Callers must acquire the RwLock before creating this.
pub struct CapabilityApi<'a> {
    plugin_id: String,
    registry: &'a mut PluginRegistry,
    permissions: Vec<PluginPermission>,
}

impl<'a> CapabilityApi<'a> {
    pub fn new(
        plugin_id: &str,
        registry: &'a mut PluginRegistry,
        permissions: &[PluginPermission],
    ) -> Self {
        Self {
            plugin_id: plugin_id.to_string(),
            registry,
            permissions: permissions.to_vec(),
        }
    }

    // P0 Core — always allowed
    pub fn register_tool(&mut self, decl: ToolDeclaration) -> Result<()> { ... }
    pub fn register_hook(&mut self, decl: HookDeclaration) -> Result<()> { ... }
    pub fn register_skill(&mut self, decl: SkillDeclaration) -> Result<()> { ... }

    // P1 Important — always allowed
    pub fn register_command(&mut self, decl: CommandDeclaration) -> Result<()> { ... }
    pub fn register_agent(&mut self, decl: AgentDeclaration) -> Result<()> { ... }

    // P2 Pluggable — permission-gated
    pub fn register_provider(&mut self, decl: ProviderDeclaration) -> Result<()> { ... }
    pub fn register_channel(&mut self, decl: ChannelDeclaration) -> Result<()> {
        self.require_permission(PluginPermission::Network)?;
        // ...
    }
    pub fn register_service(&mut self, decl: ServiceDeclaration) -> Result<()> {
        self.require_permission(PluginPermission::Background)?;
        // ...
    }

    // P3 Gateway Extensions — permission-gated + logged
    pub fn register_http_route(&mut self, decl: HttpRouteDeclaration) -> Result<()> {
        self.require_permission(PluginPermission::HttpRoutes)?;
        // ...
    }

    // Dispatch
    pub fn register_capability(&mut self, decl: CapabilityDeclaration) -> Result<()> {
        match decl {
            CapabilityDeclaration::Tool(d) => self.register_tool(d),
            CapabilityDeclaration::Hook(d) => self.register_hook(d),
            // ... all variants
        }
    }

    // Hot reload: unregister all, then re-register with new capabilities
    pub fn reload(
        &mut self,
        record: PluginRecord,
        new_caps: Vec<CapabilityDeclaration>,
    ) -> Result<()> {
        self.registry.unregister_plugin(&self.plugin_id);
        self.registry.register_plugin(record);
        for cap in new_caps {
            self.register_capability(cap)?;
        }
        Ok(())
    }

    fn require_permission(&self, perm: PluginPermission) -> Result<()> {
        if self.permissions.contains(&perm) {
            Ok(())
        } else {
            Err(anyhow!("Plugin '{}' lacks permission {:?}", self.plugin_id, perm))
        }
    }
}
```

#### MCP Registrar

Uses **collect-then-batch** pattern: async probe collects declarations without holding any lock, then a synchronous batch writes them into the registry via `CapabilityApi`.

```rust
// src/extension/registrar/mcp_registrar.rs

pub struct McpRegistrar {
    plugin_id: String,
    permissions: Vec<PluginPermission>,
}

impl McpRegistrar {
    /// Phase 1 (async): Probe MCP server for capabilities. No locks held.
    pub async fn probe_capabilities(
        &self,
        mcp_client: &McpClient,
    ) -> Result<Vec<CapabilityDeclaration>> {
        let mut caps = vec![];

        // Standard MCP tools/list → Tool capabilities
        let tools = mcp_client.list_tools().await?;
        for tool in tools {
            caps.push(CapabilityDeclaration::Tool(ToolDeclaration::from_mcp(tool)));
        }

        // Optional aleph://capabilities resource for advanced registrations
        // Non-Aleph MCP servers simply won't have this → graceful skip
        if let Ok(resource) = mcp_client.read_resource("aleph://capabilities").await {
            if let Ok(extra) = serde_json::from_str::<Vec<CapabilityDeclaration>>(&resource.text) {
                caps.extend(extra);
            }
        }

        Ok(caps)
    }

    /// Phase 2 (sync): Write collected capabilities into registry. Lock held briefly.
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

// Usage pattern (in McpManager callback):
// let caps = registrar.probe_capabilities(&client).await?;
// let mut reg = plugin_registry.write().unwrap_or_else(|e| e.into_inner());
// registrar.batch_register(caps, &mut reg)?;
```

#### WASM Registrar

WASM host functions run synchronously (Extism is sync), so they can acquire the `RwLock` directly.

```rust
// src/extension/registrar/wasm_registrar.rs

/// Host function exposed to WASM plugins: aleph_register(json_bytes) -> status
/// Called from within Extism host function context (synchronous).
pub fn host_fn_register(
    plugin_id: &str,
    registry: &std::sync::RwLock<PluginRegistry>,
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

### 3. PluginRegistry Evolution

The existing PluginRegistry structure is preserved. Changes are additive:

**New registration types**:

```rust
// registry/types.rs — additions

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRegistration {
    pub name: String,
    pub description: String,
    pub content: String,
    pub triggers: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub category: Option<String>,
    pub plugin_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistration {
    pub name: String,
    pub description: String,
    pub content: String,
    pub model: Option<String>,
    pub plugin_id: String,
}
```

**New fields in PluginRegistry**: `skills: HashMap<String, SkillRegistration>`, `agents: HashMap<String, AgentRegistration>` with corresponding register/get/list methods.

**PluginRecord change**: Remove `NodeJs` from `PluginKind` enum.

### 4. ExtensionManager Simplification

Reduce from god object (~480 lines) to orchestrator (~250 lines). Current struct has 12 fields; the new struct has 10:

```rust
pub struct ExtensionManager {
    // Discovery & parsing
    discovery: DiscoveryManager,
    adapter_registry: AdapterRegistry,

    // Central registry (replaces separate skills/commands/agents HashMaps)
    plugin_registry: Arc<RwLock<PluginRegistry>>,

    // Runtimes
    loader: PluginLoader,           // WASM + MCP (Node.js removed)
    hook_executor: Arc<RwLock<HookExecutor>>,
    service_manager: Arc<RwLock<ServiceManager>>,

    // Config & state
    config: ExtensionConfig,
    config_manager: ConfigManager,  // RETAINED (config/ directory, not a single file)
    cache_state: Arc<RwLock<CacheState>>,

    // Skill system v2 integration
    skill_system: crate::skill::SkillSystem,  // RETAINED: independent bounded context
}
```

**Fields removed**: `skills: Arc<RwLock<HashMap<String, ExtensionSkill>>>`, `commands: Arc<RwLock<HashMap<String, ExtensionCommand>>>`, `agents: Arc<RwLock<HashMap<String, ExtensionAgent>>>`, `loader: ContentLoader` (the old content loader).

**Fields retained**: `config_manager` (it's `config/` directory with 4 files, 1265 lines — handles aleph.jsonc), `skill_system` (independent bounded context at `src/skill/` for v2 prompt-driven skills).

**Migration of query methods**:
- `get_skills()` → delegates to `plugin_registry.read().list_skills()`
- `get_commands()` → delegates to `plugin_registry.read().list_commands()`
- `get_agents()` → delegates to `plugin_registry.read().list_agents()`
- `SyncExtensionManager` wrapper (`sync_api.rs`): updated to use same delegation pattern
- `skill_ops.rs`: `install_skill()` / `delete_skill()` continue to work with `SkillSystem` for v2 skills and write to PluginRegistry for plugin-provided skills

**ExtensionSkill → SkillRegistration migration**: `ExtensionSkill` (defined in `types/skills.rs`, 221 lines) has fields: name, description, content, triggers, allowed_tools, category, emoji, plugin_id, source_path. `SkillRegistration` is a subset. During migration, `ExtensionSkill` is retained as the internal type; `SkillRegistration` adds serde traits for Registry storage. A `From<ExtensionSkill> for SkillRegistration` conversion bridges the two. Same pattern for `ExtensionAgent` → `AgentRegistration`.

### 5. Permission Model Enhancement

The existing `PluginPermission` enum has: `Network`, `FilesystemRead`, `FilesystemWrite`, `Filesystem`, `Env`, `Custom(String)`. The new enum consolidates and extends:

```rust
pub enum PluginPermission {
    // Existing (consolidated)
    Network,
    Filesystem(FilesystemAccess),  // replaces FilesystemRead/FilesystemWrite/Filesystem
    Env,
    Custom(String),

    // New
    Shell,         // required for CliCommand registration and shell execution
    Background,    // required for Service registration
    HttpRoutes,    // required for HTTP route registration
    GatewayRpc,    // required for gateway method registration
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilesystemAccess {
    Read,
    Write,
    Full,  // equivalent to old `Filesystem` (no qualifier)
}
```

**Migration**: `FilesystemRead` → `Filesystem(Read)`, `FilesystemWrite` → `Filesystem(Write)`, `Filesystem` → `Filesystem(Full)`. This is a breaking change in manifest parsing but not in runtime behavior — the `[aleph.permissions]` TOML syntax already uses `filesystem = "read"` / `"write"` / `true`, so only the internal representation changes.

**Permission-capability mapping**:

| Capability | Tier | Required Permission |
|-----------|------|-------------------|
| Tool | P0 | None |
| Hook | P0 | None |
| Skill | P0 | None |
| Command | P1 | None |
| Agent | P1 | None |
| Provider | P2 | None |
| Channel | P2 | Network |
| Service | P2 | Background |
| McpServer | P2 | None |
| HttpRoute | P3 | HttpRoutes |
| GatewayMethod | P3 | GatewayRpc |
| CliCommand | P3 | Shell |

Static plugins from CC/Codex/Cursor format (no `[aleph.permissions]`) default to P0+P1 permissions — sufficient for all third-party static plugins.

### 6. Hot Reload

**Static plugins**: `ExtensionManager::reload_plugin(id)` → unregister_plugin + re-parse + re-register. Exposed as `plugin.reload` RPC method.

**MCP plugins**: On reconnect, `McpRegistrar::probe_and_register()` calls `api.reload()`. Leverages existing `PluginRegistry::unregister_plugin()`.

**WASM plugins**: On re-load, WASM host function triggers `api.reload()`.

## Module Structure

### After reorganization

```
src/extension/
├── mod.rs                    ← ExtensionManager (~250 lines, down from 480)
├── capability.rs             ← NEW: CapabilityDeclaration, CapabilitySource, SourceFormat
│
├── manifest/
│   ├── mod.rs               ← AdapterRegistry entry (~100 lines, down from 738)
│   ├── adapter.rs           ← NEW: ManifestAdapter trait + AdapterRegistry
│   ├── parsers.rs           ← NEW: Shared component parsers (extracted from content_loader)
│   ├── types.rs             ← Retained, legacy fields removed (575 lines → ~400)
│   ├── cc_plugin_toml.rs    ← Retained, refactored into ManifestAdapter impl
│   ├── cc_plugin_json.rs    ← Retained, refactored into ManifestAdapter impl
│   └── adapters/
│       ├── mod.rs
│       ├── codex.rs         ← NEW
│       ├── cursor.rs        ← NEW
│       └── auto_discover.rs ← Refactored from existing auto_discover.rs
│
├── registrar/                ← NEW
│   ├── mod.rs               ← CapabilityRegistrar trait
│   ├── api.rs               ← CapabilityApi
│   ├── mcp_registrar.rs     ← MCP probe-and-register (collect-then-batch)
│   └── wasm_registrar.rs    ← WASM host functions
│
├── registry/
│   ├── mod.rs
│   ├── types.rs             ← +SkillRegistration, +AgentRegistration
│   └── plugin_registry.rs   ← Promoted from subdirectory
│
├── discovery/                ← Unchanged
│   ├── mod.rs               (359 lines)
│   ├── resolver.rs          (187 lines)
│   └── scanner.rs           (290 lines)
│
├── hooks/                    ← Unchanged
│   ├── mod.rs               (536 lines)
│   └── executor.rs          (469 lines)
│
├── runtime/
│   └── wasm/                ← Retained as directory (7 files, ~1374 lines)
│       ├── mod.rs           ← +host_fn_register integration
│       ├── host_functions.rs
│       ├── capabilities.rs
│       ├── capability_kernel.rs
│       ├── allowlist.rs
│       ├── limits.rs
│       ├── permissions.rs
│       └── credential_injector.rs
│
├── marketplace/              ← Unchanged
│   ├── mod.rs               (351 lines)
│   ├── github_source.rs     (102 lines)
│   ├── local_source.rs      (99 lines)
│   ├── installer.rs         (197 lines)
│   ├── manifest.rs          (173 lines)
│   └── types.rs             (102 lines)
│
├── config/                   ← Unchanged (NOT config_manager.rs)
│   ├── mod.rs               (327 lines)
│   ├── types.rs             (290 lines)
│   ├── loader.rs            (302 lines)
│   └── migrate.rs           (346 lines)
│
├── loader.rs                 ← Renamed from plugin_loader.rs, Node.js code removed
├── mcp_config.rs             ← Unchanged (265 lines)
├── scope.rs                  ← Unchanged (162 lines)
├── service_manager.rs        ← Unchanged (659 lines)
│
│── # Retained utility files (unchanged)
├── channel_manager.rs        (697 lines)
├── component_id.rs           (114 lines)
├── error.rs                  (140 lines)
├── http_handler.rs           (539 lines)
├── plugin_ops.rs             (136 lines)
├── provider_adapter.rs       (289 lines)
├── service_ops.rs            (84 lines)
├── skill_ops.rs              (210 lines — adapted for PluginRegistry delegation)
├── skill_tool.rs             (344 lines)
├── sync_api.rs               (304 lines — adapted for PluginRegistry delegation)
├── template.rs               (271 lines)
├── validation.rs             (230 lines)
├── watcher.rs                (418 lines)
│
└── types/
    ├── mod.rs                (27 lines)
    ├── plugins.rs            ← NodeJs removed from PluginKind (310 lines → ~300)
    ├── hooks.rs              (277 lines)
    ├── runtime.rs            (324 lines)
    ├── skills.rs             (221 lines — retained, From<ExtensionSkill> for SkillRegistration)
    └── agents.rs             (170 lines — retained, From<ExtensionAgent> for AgentRegistration)
```

**Adapter conflict behavior**: When multiple adapters match (e.g., a directory has both `.claude-plugin/` and `.codex-plugin/`), only the highest-priority adapter runs. This is the intended behavior — the Aleph-preferred format wins. If a user needs to force a specific adapter, they can remove the competing manifest files.

### Deleted files (audited with `wc -l`)

| File/Directory | Actual Lines | Reason |
|------|-------|--------|
| `manifest/aleph_plugin_toml/` (6 files) | 1,279 | Deprecated; `[aleph]` extensions parsed by CC adapter |
| `manifest/aleph_plugin.rs` | 636 | Deprecated `aleph.plugin.json` format |
| `manifest/package_json.rs` | 474 | Deprecated npm format |
| `manifest/auto_discover.rs` | 242 | Logic moved to `adapters/auto_discover.rs` |
| `runtime/nodejs/` (3 files) | 1,026 | Deprecated Node.js IPC runtime |
| `runtime/mod.rs` | 812 | Runtime dispatcher (only dispatched to Node.js/WASM; WASM accessed directly now) |
| `content_loader/` (6 files) | 1,125 | Logic distributed to `manifest/parsers.rs` + `ExtensionManager` |
| `plugin_loader.rs` | 778 | Replaced by `loader.rs` (~400 lines, Node.js code removed) |
| `manifest/mod.rs` (rewrite) | ~600 net | 738-line fallback chain rewritten as ~100-line AdapterRegistry entry |
| **Total removed** | **~6,370** | (some code migrated, not purely deleted) |

**Net code migrated** (not deleted, moved to new locations): ~1,800 lines from content_loader parsers, auto_discover logic, and plugin_loader WASM/MCP code.

**Net pure deletion**: ~4,570 lines of deprecated/replaced code.

### New files

| File | Est. Lines | Purpose |
|------|-----------|---------|
| `capability.rs` | ~150 | CapabilityDeclaration enum, type aliases, SourceFormat |
| `manifest/adapter.rs` | ~80 | ManifestAdapter trait + AdapterRegistry |
| `manifest/parsers.rs` | ~350 | Shared component parsers (migrated from content_loader) |
| `manifest/adapters/codex.rs` | ~150 | Codex adapter |
| `manifest/adapters/cursor.rs` | ~150 | Cursor adapter |
| `manifest/adapters/auto_discover.rs` | ~120 | Auto-discover adapter (migrated + refactored) |
| `registrar/mod.rs` | ~30 | Trait definition + re-exports |
| `registrar/api.rs` | ~200 | CapabilityApi implementation |
| `registrar/mcp_registrar.rs` | ~120 | MCP collect-then-batch probe |
| `registrar/wasm_registrar.rs` | ~80 | WASM host functions |
| **Total new** | **~1,430** | |

**Net impact**: Remove ~6,370 lines, add ~1,430 lines, migrate ~1,800 lines. Net reduction of ~3,140 lines while adding multi-platform support, dynamic registration, permission model, and hot reload.

## Migration Safety

1. **External API preserved**: Gateway RPC handlers (`plugin.list`, `plugin.install`, etc.) call ExtensionManager public methods — signatures unchanged.
2. **PluginRegistry query API preserved**: `get_tool()`, `list_tools()`, `get_hooks_for_event()` — unchanged.
3. **New API is additive**: `get_skill()`, `list_skills()`, `get_agent()`, `list_agents()` on PluginRegistry.
4. **Backward compatible manifest reading**: CC format plugins work exactly as before. New adapters only add format support.
5. **PluginRecord**: Only change is removing `NodeJs` from `PluginKind` enum.

## Advantages Over OpenClaw

| Dimension | OpenClaw | Aleph (this design) |
|-----------|---------|-------------------|
| Plugin isolation | In-process TypeScript, crash = host crash | WASM sandboxed, MCP process-isolated |
| Capability tiering | Flat, all registrations equal | P0-P3 with tier-specific policies |
| Permission model | None, any plugin can register anything | Manifest-declared permissions, enforced at registration |
| Hot reload | Requires full restart | Per-plugin reload (static: re-parse, MCP: reconnect, WASM: re-load) |
| Manifest overhead | 3 separate files per plugin | Single manifest + capability auto-inference |
| Format support | 3 platform-specific parsers | Extensible adapter trait, new platform = new impl |
| Type safety | TypeScript runtime checks | Rust compile-time enum exhaustiveness |
| Performance | Node.js overhead for all plugins | Zero-cost for static, near-native for WASM |

## Out of Scope

- **Plugin SDK crate** (`aleph-plugin-sdk`): Future work for WASM plugin authors. This design ensures compatibility but does not ship an SDK.
- **Plugin export** (producing CC/Codex/Cursor format from Aleph plugins): Deferred.
- **File watcher for automatic hot-reload**: Only manual `plugin.reload` RPC in this iteration.
- **LSP server integration**: Parsed but execution deferred (existing behavior).
- **Output styles**: Parsed but execution deferred (existing behavior).
