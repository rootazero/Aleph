# Capability-Driven Plugin Architecture Design

**Date**: 2026-03-25
**Status**: Draft
**Scope**: `core/src/extension/` module reorganization + multi-platform plugin compatibility + dynamic registration API

## Summary

Redesign Aleph's plugin system from a Claude Code-only static format reader into a **capability-driven architecture** that:

1. Consumes plugins from any platform (Claude Code, Codex CLI, Cursor) via a trait-based ManifestAdapter system
2. Supports runtime capability registration by MCP/WASM plugins via a CapabilityRegistrar API
3. Cleans up ~1370 lines of deprecated code (Node.js runtime, legacy manifest formats)
4. Surpasses OpenClaw in five dimensions: WASM sandboxing, capability tiering, permission model, hot reload, capability inference

## Motivation

**Learning from OpenClaw**: OpenClaw achieves multi-platform plugin compatibility through platform-specific bundle manifests (`.claude-plugin/`, `.codex-plugin/`, `.cursor-plugin/`) and a `definePluginEntry` + `registerXxx` registration API in TypeScript.

**Aleph's current gap**: Only consumes Claude Code format. No dynamic registration API — MCP configs are stored but never wired to the registry; WASM plugins can execute tools but cannot declare capabilities. The PluginRegistry has 11 HashMap registries, but only Tool and Hook are actively used.

**Aleph's existing strengths**: 4-layer discovery with priority resolution, P0-P3 capability tiering, WASM sandboxing via Extism, strong Rust type system. These should be leveraged, not discarded.

## Architecture

### Core Abstraction: Capability Model

All plugin capabilities — whether parsed from a manifest file or registered at runtime by a running process — are represented as `CapabilityDeclaration`:

```rust
// core/src/extension/capability.rs

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
// core/src/extension/manifest/adapter.rs

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
// core/src/extension/manifest/parsers.rs

pub fn parse_skills_dir(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_commands_dir(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_agents_dir(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_hooks_file(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
pub fn parse_mcp_config(base: &Path, rel: &str) -> Result<Vec<CapabilityDeclaration>> { ... }
```

### 2. CapabilityRegistrar & CapabilityApi

#### CapabilityApi

The unified API surface for writing capabilities into PluginRegistry:

```rust
// core/src/extension/registrar/api.rs

pub struct CapabilityApi<'a> {
    plugin_id: String,
    registry: &'a mut PluginRegistry,
    permissions: &'a [PluginPermission],
}

impl<'a> CapabilityApi<'a> {
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

    // Hot reload
    pub fn reload(&mut self, new_caps: Vec<CapabilityDeclaration>) -> Result<()> {
        self.registry.unregister_plugin(&self.plugin_id);
        self.registry.register_plugin(PluginRecord::new(&self.plugin_id));
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

```rust
// core/src/extension/registrar/mcp_registrar.rs

pub struct McpRegistrar { plugin_id: String }

impl McpRegistrar {
    pub async fn probe_and_register(
        &self,
        mcp_client: &McpClient,
        api: &mut CapabilityApi<'_>,
    ) -> Result<()> {
        // Step 1: Standard MCP tools/list → Tool capabilities
        let tools = mcp_client.list_tools().await?;
        for tool in tools {
            api.register_tool(ToolDeclaration::from_mcp(tool))?;
        }

        // Step 2: Optional aleph://capabilities resource for advanced registrations
        // Non-Aleph MCP servers won't have this → graceful skip
        if let Ok(resource) = mcp_client.read_resource("aleph://capabilities").await {
            let caps: Vec<CapabilityDeclaration> = serde_json::from_str(&resource.text)?;
            for cap in caps {
                api.register_capability(cap)?;
            }
        }

        Ok(())
    }
}
```

#### WASM Registrar

```rust
// core/src/extension/registrar/wasm_registrar.rs

/// Host function exposed to WASM plugins: aleph_register(json_bytes) -> status
pub fn host_fn_register(
    plugin_id: &str,
    registry: &Mutex<PluginRegistry>,
    permissions: &[PluginPermission],
    input: &[u8],
) -> Result<()> {
    let declarations: Vec<CapabilityDeclaration> = serde_json::from_slice(input)?;
    let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
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

Reduce from god object to orchestrator (~200 lines):

```rust
pub struct ExtensionManager {
    discovery: DiscoveryManager,
    adapter_registry: AdapterRegistry,
    plugin_registry: Arc<RwLock<PluginRegistry>>,
    loader: PluginLoader,
    hook_executor: Arc<RwLock<HookExecutor>>,
    service_manager: Arc<RwLock<ServiceManager>>,
    config: ExtensionConfig,
    cache_state: Arc<RwLock<CacheState>>,
}
```

Key change: skills, commands, agents are no longer stored in ExtensionManager's own HashMaps — they live in PluginRegistry. Query methods (`get_skills()`, `get_commands()`, `get_agents()`) delegate to PluginRegistry.

### 5. Permission Model Enhancement

```rust
pub enum PluginPermission {
    Network,
    Filesystem(FilesystemAccess),
    Env,
    Shell,
    Background,    // NEW: required for Service registration
    HttpRoutes,    // NEW: required for HTTP route registration
    GatewayRpc,    // NEW: required for gateway method registration
    Custom(String),
}
```

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
core/src/extension/
├── mod.rs                    ← ExtensionManager (~200 lines)
├── capability.rs             ← NEW: CapabilityDeclaration, CapabilitySource, SourceFormat
│
├── manifest/
│   ├── mod.rs               ← AdapterRegistry entry (~100 lines)
│   ├── adapter.rs           ← ManifestAdapter trait + AdapterRegistry
│   ├── parsers.rs           ← Shared component parsers
│   ├── types.rs             ← Retained, legacy fields removed
│   └── adapters/
│       ├── mod.rs
│       ├── claude_code.rs   ← CC TOML + JSON merged
│       ├── codex.rs         ← NEW
│       ├── cursor.rs        ← NEW
│       └── auto_discover.rs ← Refactored from existing
│
├── registrar/                ← NEW
│   ├── mod.rs               ← CapabilityRegistrar trait
│   ├── api.rs               ← CapabilityApi
│   ├── mcp_registrar.rs     ← MCP probe-and-register
│   └── wasm_registrar.rs    ← WASM host functions
│
├── registry/
│   ├── mod.rs
│   ├── types.rs             ← +SkillRegistration, +AgentRegistration
│   └── plugin_registry.rs   ← Promoted from subdirectory
│
├── discovery/                ← Unchanged
│   ├── mod.rs
│   └── resolver.rs
│
├── hooks/                    ← Unchanged
│   └── mod.rs
│
├── runtime/
│   └── wasm.rs              ← Promoted from subdirectory, +host functions
│
├── marketplace/              ← Unchanged
│   ├── mod.rs
│   ├── github_source.rs
│   └── installer.rs
│
├── loader.rs                 ← Renamed, Node.js code removed (~400 lines)
├── mcp_config.rs             ← Unchanged
├── scope.rs                  ← Unchanged
├── config_manager.rs         ← Unchanged
├── service_manager.rs        ← Unchanged
│
└── types/
    ├── mod.rs
    ├── plugins.rs            ← NodeJs removed from PluginKind
    ├── hooks.rs
    └── runtime.rs
```

### Deleted files

| File | Lines | Reason |
|------|-------|--------|
| `manifest/aleph_plugin_toml.rs` | ~200 | Deprecated, `[aleph]` handled by CC adapter |
| `manifest/aleph_plugin.rs` | ~150 | Deprecated `aleph.plugin.json` format |
| `manifest/package_json.rs` | ~120 | Deprecated npm format |
| `runtime/nodejs/mod.rs` | ~300 | Deprecated Node.js IPC runtime |
| `plugin_loader.rs` Node.js code | ~200 | Removed with runtime |
| `manifest/mod.rs` fallback chain | ~400 | Replaced by AdapterRegistry |
| `content_loader/` entire directory | ~300 | Logic distributed to parsers.rs + ExtensionManager |
| **Total removed** | **~1670** | |

### New files

| File | Est. Lines | Purpose |
|------|-----------|---------|
| `capability.rs` | ~150 | CapabilityDeclaration enum, SourceFormat, declarations |
| `manifest/adapter.rs` | ~80 | ManifestAdapter trait, AdapterRegistry |
| `manifest/parsers.rs` | ~300 | Shared component parsers (from content_loader) |
| `manifest/adapters/claude_code.rs` | ~200 | CC TOML+JSON adapter |
| `manifest/adapters/codex.rs` | ~150 | Codex adapter |
| `manifest/adapters/cursor.rs` | ~150 | Cursor adapter |
| `manifest/adapters/auto_discover.rs` | ~100 | Auto-discover adapter |
| `registrar/mod.rs` | ~30 | Trait definition + re-exports |
| `registrar/api.rs` | ~200 | CapabilityApi implementation |
| `registrar/mcp_registrar.rs` | ~100 | MCP probe logic |
| `registrar/wasm_registrar.rs` | ~80 | WASM host functions |
| **Total new** | **~1540** | |

**Net change**: Remove ~1670 lines, add ~1540 lines. Net reduction of ~130 lines while adding multi-platform support, dynamic registration, permission model, and hot reload.

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
