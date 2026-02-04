# Plugin & Extension System Design

> Date: 2026-01-31
> Status: Draft
> Author: Claude + User

## Overview

This document describes the design for Aleph's Plugin & Extension system, enabling both WASM native plugins and Node.js/npm ecosystem compatibility (Moltbot-style).

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Runtime Model | **Hybrid** (WASM + Node.js IPC) | Native performance + npm ecosystem |
| API Scope | **Full** (9 registration types) | Complete Moltbot compatibility |
| Discovery Layers | **4 layers** (config > workspace > global > bundled) | Flexible override mechanism |
| Manifest Format | **Dual** (package.json / aether.plugin.json) | npm + standalone support |
| Node.js IPC | **JSON-RPC over stdio** | Simple, debuggable, reliable |
| WASM Runtime | **Extism** | Purpose-built for plugins |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      ExtensionManager                           │
│                    (Unified entry point)                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │  Discovery  │  │   Loader    │  │  Registry   │             │
│  │   Manager   │──▶│   Manager   │──▶│   Manager   │             │
│  └─────────────┘  └─────────────┘  └─────────────┘             │
│         │                │                │                     │
│         ▼                ▼                ▼                     │
│  ┌─────────────────────────────────────────────────┐           │
│  │              Plugin Runtimes                     │           │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐         │           │
│  │  │  WASM   │  │ Node.js │  │ Static  │         │           │
│  │  │(Extism) │  │  (IPC)  │  │  (MD)   │         │           │
│  │  └─────────┘  └─────────┘  └─────────┘         │           │
│  └─────────────────────────────────────────────────┘           │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Core Components

- **DiscoveryManager** — Scans 4-layer directories, returns PluginCandidate list
- **LoaderManager** — Selects appropriate runtime based on plugin kind
- **RegistryManager** — Stores all registered tools/hooks/channels etc.
- **Plugin Runtimes** — Three runtimes: WASM (Extism), Node.js (IPC), Static (existing MD)

---

## Plugin Discovery

### 4-Layer Directory Structure

```
Priority 1 (highest): Config-specified paths
  config.plugins.extra_paths: ["/custom/plugins"]

Priority 2: Workspace local
  ./.aether/extensions/
  ./.claude/extensions/     (Claude Code compatibility)

Priority 3: Global user-level
  ~/.aleph/extensions/
  ~/.claude/extensions/     (compatibility)

Priority 4 (lowest): Bundled
  <aether-binary>/bundled/
```

### Discovery Types

```rust
pub struct PluginCandidate {
    pub id: String,              // Plugin ID (from manifest or dir name)
    pub source: PathBuf,         // Entry file path
    pub root_dir: PathBuf,       // Plugin root directory
    pub origin: PluginOrigin,    // Config/Workspace/Global/Bundled
    pub kind: PluginKind,        // Wasm/NodeJs/Static
    pub manifest: PluginManifest,// Parsed manifest
}

pub enum PluginOrigin {
    Config,     // Priority 1
    Workspace,  // Priority 2
    Global,     // Priority 3
    Bundled,    // Priority 4
}

pub enum PluginKind {
    Wasm,       // .wasm file + aether.plugin.json
    NodeJs,     // package.json + "aether" field
    Static,     // SKILL.md / COMMAND.md / AGENT.md
}
```

### Conflict Resolution

Same-ID plugins: keep highest priority origin, mark others as `overridden`.

---

## Manifest Formats

### Format A: package.json (Node.js plugins)

```json
{
  "name": "@aether/my-plugin",
  "version": "1.0.0",
  "main": "dist/index.js",
  "aether": {
    "extensions": ["src/index.ts"],
    "configSchema": {
      "type": "object",
      "properties": {
        "apiKey": { "type": "string" }
      }
    },
    "configUiHints": {
      "apiKey": { "sensitive": true, "label": "API Key" }
    }
  }
}
```

### Format B: aether.plugin.json (WASM/standalone plugins)

```json
{
  "id": "my-wasm-plugin",
  "name": "My WASM Plugin",
  "version": "1.0.0",
  "kind": "wasm",
  "entry": "plugin.wasm",
  "configSchema": { },
  "configUiHints": { },
  "permissions": ["network", "filesystem:read"]
}
```

### Format C: Static plugins (existing format, continued support)

```
my-skill/
├── SKILL.md          # YAML frontmatter + content
└── assets/
```

### Unified Internal Structure

```rust
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub kind: PluginKind,
    pub entry: PathBuf,
    pub config_schema: Option<JsonSchema>,
    pub config_ui_hints: HashMap<String, UiHint>,
    pub permissions: Vec<Permission>,
}
```

---

## PluginApi Trait

### Core Trait Definition

```rust
/// Plugin registration API - shared by all plugin types
pub trait PluginApi: Send + Sync {
    // === P0 Core ===
    fn register_tool(&self, tool: ToolRegistration);
    fn register_hook(&self, hook: HookRegistration);

    // === P1 Important ===
    fn register_channel(&self, channel: ChannelRegistration);
    fn register_provider(&self, provider: ProviderRegistration);
    fn register_gateway_method(&self, method: GatewayMethodRegistration);

    // === P2 Useful ===
    fn register_http_route(&self, route: HttpRouteRegistration);
    fn register_http_handler(&self, handler: HttpHandlerRegistration);
    fn register_cli(&self, cli: CliRegistration);
    fn register_service(&self, service: ServiceRegistration);

    // === P3 Optional ===
    fn register_command(&self, command: CommandRegistration);

    // === Context Access ===
    fn plugin_id(&self) -> &str;
    fn plugin_config(&self) -> Option<&serde_json::Value>;
    fn logger(&self) -> &dyn PluginLogger;
}
```

### Registration Types

```rust
pub struct ToolRegistration {
    pub name: String,
    pub description: String,
    pub parameters: JsonSchema,
    pub handler: ToolHandler,  // WASM: fn name, Node: method name
}

pub struct HookRegistration {
    pub event: HookEvent,      // BeforeToolCall, AfterToolCall, etc.
    pub priority: i32,         // Execution order
    pub handler: HookHandler,
}

pub enum HookEvent {
    BeforeAgentStart,
    AgentEnd,
    BeforeToolCall,
    AfterToolCall,
    MessageReceived,
    MessageSending,
    SessionStart,
    SessionEnd,
    // ... 13 total
}
```

---

## Node.js IPC Protocol

### Plugin Host Architecture

```
┌─────────────────┐         stdio          ┌─────────────────┐
│   Rust Core     │◀═══════════════════════▶│  Node.js Host   │
│                 │    JSON-RPC 2.0         │                 │
│  NodeJsRuntime  │                         │  plugin-host.js │
└─────────────────┘                         └─────────────────┘
                                                    │
                                            ┌───────┴───────┐
                                            ▼               ▼
                                      [plugin-a.ts]   [plugin-b.ts]
```

### Protocol Messages

```typescript
// Rust → Node.js: Call plugin method
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "method": "plugin.call",
  "params": {
    "pluginId": "my-plugin",
    "handler": "toolHandler",
    "args": { "input": "hello" }
  }
}

// Node.js → Rust: Return result
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "result": { "output": "world" }
}

// Node.js → Rust: Plugin registration (at startup)
{
  "jsonrpc": "2.0",
  "method": "register",
  "params": {
    "pluginId": "my-plugin",
    "tools": [{ "name": "my_tool", "schema": {...} }],
    "hooks": [{ "event": "before_tool_call" }]
  }
}
```

### Lifecycle

1. Rust starts Node.js subprocess (plugin-host.js)
2. Node.js loads all NodeJs-type plugins
3. Each plugin calls api.registerXxx() to register capabilities
4. Node.js sends "register" message reporting all registrations
5. Rust receives and updates Registry
6. Runtime: Rust calls plugins via "plugin.call"
7. Shutdown: Rust sends "shutdown", Node.js exits gracefully

---

## WASM Plugin Runtime (Extism)

### Extism Integration

```rust
use extism::{Plugin, Manifest, Wasm};

pub struct WasmRuntime {
    plugins: HashMap<String, Plugin>,
}

impl WasmRuntime {
    pub fn load_plugin(&mut self, id: &str, wasm_path: &Path) -> Result<()> {
        let manifest = Manifest::new([Wasm::file(wasm_path)]);
        let plugin = Plugin::new(&manifest, [], true)?; // true = WASI support
        self.plugins.insert(id.to_string(), plugin);
        Ok(())
    }

    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        tool_name: &str,
        input: &str
    ) -> Result<String> {
        let plugin = self.plugins.get_mut(plugin_id)?;
        let result = plugin.call::<&str, &str>(tool_name, input)?;
        Ok(result.to_string())
    }
}
```

### WASM Plugin SDK (Rust side)

```rust
// SDK for plugin authors
use aether_plugin_sdk::*;

#[aether_plugin]
pub fn register(api: &mut PluginApi) {
    api.register_tool(Tool {
        name: "hello",
        description: "Say hello",
        handler: hello_handler,
    });
}

#[tool_handler]
fn hello_handler(input: ToolInput) -> ToolOutput {
    ToolOutput::text(format!("Hello, {}!", input.get("name")?))
}
```

### Host Functions (Rust exposes to WASM)

```rust
// Host capabilities callable by WASM plugins
host_fn!(pub fn aether_log(level: i32, msg: &str));
host_fn!(pub fn aether_http_get(url: &str) -> String);
host_fn!(pub fn aether_read_file(path: &str) -> String);  // requires permission
host_fn!(pub fn aether_emit_event(event: &str, data: &str));
```

### Permission Sandbox

- WASM has no filesystem/network access by default
- `permissions: ["network"]` required for `aether_http_get`
- `permissions: ["filesystem:read"]` required for file reading

---

## Registry Management

### Unified Registry Structure

```rust
pub struct PluginRegistry {
    // Plugin metadata
    plugins: HashMap<String, PluginRecord>,

    // Registration items indexed by type
    tools: HashMap<String, ToolRegistration>,         // tool_name → registration
    hooks: Vec<HookRegistration>,                      // sorted by priority
    channels: HashMap<String, ChannelRegistration>,   // channel_id → registration
    providers: HashMap<String, ProviderRegistration>, // provider_id → registration
    gateway_methods: HashMap<String, GatewayMethodRegistration>,
    http_routes: Vec<HttpRouteRegistration>,
    http_handlers: Vec<HttpHandlerRegistration>,
    cli_commands: Vec<CliRegistration>,
    services: HashMap<String, ServiceRegistration>,
    commands: HashMap<String, CommandRegistration>,

    // Diagnostics
    diagnostics: Vec<PluginDiagnostic>,
}

pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub kind: PluginKind,
    pub origin: PluginOrigin,
    pub status: PluginStatus,        // Loaded / Disabled / Error
    pub error: Option<String>,

    // What this plugin registered
    pub tool_names: Vec<String>,
    pub hook_count: usize,
    pub channel_ids: Vec<String>,
    pub provider_ids: Vec<String>,
    pub gateway_methods: Vec<String>,
    pub service_ids: Vec<String>,
}

pub enum PluginStatus {
    Loaded,
    Disabled,      // User disabled
    Overridden,    // Overridden by higher priority
    Error(String), // Load failed
}
```

### Query Interface

```rust
impl PluginRegistry {
    // Tool queries
    pub fn get_tool(&self, name: &str) -> Option<&ToolRegistration>;
    pub fn list_tools(&self) -> Vec<&ToolRegistration>;

    // Hook execution
    pub fn get_hooks_for_event(&self, event: HookEvent) -> Vec<&HookRegistration>;

    // Plugin management
    pub fn get_plugin(&self, id: &str) -> Option<&PluginRecord>;
    pub fn list_plugins(&self) -> Vec<&PluginRecord>;
    pub fn disable_plugin(&mut self, id: &str) -> Result<()>;
    pub fn enable_plugin(&mut self, id: &str) -> Result<()>;
}
```

---

## File Structure

### New/Modified Files

```
core/src/extension/
├── mod.rs                    # ExtensionManager (modify)
├── discovery/
│   ├── mod.rs               # DiscoveryManager (new)
│   ├── scanner.rs           # Directory scanning logic
│   └── resolver.rs          # Conflict resolution, priority
├── manifest/
│   ├── mod.rs               # Unified manifest parsing
│   ├── package_json.rs      # package.json + aether field
│   ├── aether_plugin.rs     # aether.plugin.json
│   └── static_plugin.rs     # SKILL.md/COMMAND.md (existing)
├── registry/
│   ├── mod.rs               # PluginRegistry (new)
│   ├── types.rs             # Registration type definitions
│   └── query.rs             # Query interface
├── runtime/
│   ├── mod.rs               # Runtime trait
│   ├── wasm/
│   │   ├── mod.rs           # WasmRuntime (Extism)
│   │   ├── host_functions.rs# Host functions
│   │   └── permissions.rs   # Permission checking
│   ├── nodejs/
│   │   ├── mod.rs           # NodeJsRuntime
│   │   ├── ipc.rs           # JSON-RPC communication
│   │   ├── process.rs       # Subprocess management
│   │   └── host/            # plugin-host.js source
│   └── static_runtime.rs    # Static plugins (migrate existing logic)
├── api/
│   ├── mod.rs               # PluginApi trait
│   └── registrations.rs     # 9 registration types
├── loader.rs                # LoaderManager (refactor)
├── hooks/                   # (existing, keep)
└── config/                  # (existing, keep)
```

### New Dependencies (Cargo.toml)

```toml
[dependencies]
extism = "1.0"              # WASM runtime
jsonrpc-core = "18.0"       # JSON-RPC protocol
schemars = "0.8"            # JSON Schema generation

[features]
plugin-wasm = ["extism"]
plugin-nodejs = []          # Requires system Node.js
plugin-all = ["plugin-wasm", "plugin-nodejs"]
```

---

## Implementation Roadmap

### Phase 1: Infrastructure (Week 1)

- [ ] Discovery system refactor
  - [ ] 4-layer directory scanning
  - [ ] Dual-format manifest parsing
  - [ ] Conflict resolution logic
- [ ] Registry system
  - [ ] PluginRegistry structure
  - [ ] 9 registration type definitions
  - [ ] Query interface
- [ ] PluginApi trait definition

### Phase 2: WASM Runtime (Week 2)

- [ ] Extism integration
  - [ ] WasmRuntime implementation
  - [ ] Host functions (log/http/file/event)
  - [ ] Permission sandbox
- [ ] WASM Plugin SDK
  - [ ] aether-plugin-sdk crate
  - [ ] proc macros (#[aether_plugin], #[tool_handler])
- [ ] Example WASM plugin

### Phase 3: Node.js Runtime (Week 3)

- [ ] plugin-host.js implementation
  - [ ] Plugin loading (Jiti/ts-node)
  - [ ] PluginApi injection
  - [ ] JSON-RPC server
- [ ] NodeJsRuntime (Rust side)
  - [ ] Subprocess management
  - [ ] IPC communication
  - [ ] Lifecycle control
- [ ] Example Node.js plugin

### Phase 4: Integration & Testing (Week 4)

- [ ] ExtensionManager refactor
  - [ ] Unify three runtimes
  - [ ] Hot reload support
  - [ ] Error recovery
- [ ] Gateway integration
  - [ ] plugins.* RPC methods
  - [ ] Plugin status queries
- [ ] CLI commands
  - [ ] aether plugins list/install/disable
  - [ ] aether plugins create (scaffolding)
- [ ] Complete test suite

---

## Acceptance Criteria

- [ ] WASM plugin can register tool and be called by Agent
- [ ] Node.js plugin can register hook and intercept tool calls
- [ ] 4-layer discovery correctly handles priority conflicts
- [ ] `plugins.list` RPC returns all loaded plugins
- [ ] Plugin config schema validation works correctly

---

## References

- [Moltbot Plugin System](https://github.com/moltbot/moltbot/tree/main/src/plugins)
- [Extism Documentation](https://extism.org/docs/overview)
- [JSON-RPC 2.0 Specification](https://www.jsonrpc.org/specification)
