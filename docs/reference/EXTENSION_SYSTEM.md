# Extension System

> Plugin architecture with WASM and Node.js runtimes

---

## Overview

Aleph's extension system allows third-party tools via:
- **WASM Plugins**: Fast, sandboxed WebAssembly modules
- **Node.js Plugins**: JavaScript/TypeScript extensions
- **Manifest-driven**: Declarative plugin definitions

**Location**: `src/extension/`

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Extension Manager                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Loader     │     │   Registry   │     │   Watcher    │    │
│  │              │     │              │     │              │    │
│  │ • Discovery  │     │ • Register   │     │ • Hot reload │    │
│  │ • Manifest   │     │ • Lookup     │     │ • Events     │    │
│  │ • Validate   │     │ • Unregister │     │              │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                     Plugin Runtimes                       │   │
│  │  ┌────────────────────┐  ┌────────────────────┐         │   │
│  │  │    WASM Runtime    │  │  Node.js Runtime   │         │   │
│  │  │    (Extism)        │  │    (IPC)           │         │   │
│  │  │                    │  │                    │         │   │
│  │  │ • Sandboxed        │  │ • Stdio comm       │         │   │
│  │  │ • Fast startup     │  │ • Process mgmt     │         │   │
│  │  │ • Limited I/O      │  │ • Full Node API    │         │   │
│  │  └────────────────────┘  └────────────────────┘         │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Plugin Structure

### Directory Layout

```
~/.aleph/plugins/
├── my-plugin/
│   ├── aleph_plugin.toml    # Plugin manifest
│   ├── package.json          # (Node.js) or
│   ├── plugin.wasm           # (WASM)
│   └── src/
│       └── index.ts
└── another-plugin/
    └── ...
```

### Manifest (aleph_plugin.toml)

```toml
[plugin]
name = "my-plugin"
version = "1.0.0"
description = "My awesome plugin"
author = "Your Name"

[runtime]
type = "nodejs"  # or "wasm"
entry = "dist/index.js"

[[tools]]
name = "my_tool"
description = "Does something useful"

[tools.args]
input = { type = "string", required = true }
options = { type = "object", required = false }
```

---

## WASM Runtime

**Location**: `src/extension/runtime/wasm/`

Feature-gated: `plugin-wasm`

### Architecture

```rust
pub struct WasmRuntime {
    plugins: HashMap<String, ExtismPlugin>,
}

impl WasmRuntime {
    pub fn load(&mut self, path: &Path) -> Result<()> {
        let plugin = Plugin::new(path, [], true)?;
        self.plugins.insert(name, plugin);
    }

    pub fn call(
        &self,
        plugin: &str,
        function: &str,
        input: &[u8],
    ) -> Result<Vec<u8>> {
        self.plugins[plugin].call(function, input)
    }
}
```

### Plugin Interface

WASM plugins export functions:

```rust
// Plugin side (Rust → WASM)
#[extism_pdk::plugin_fn]
pub fn my_tool(input: String) -> FnResult<String> {
    let args: MyToolArgs = serde_json::from_str(&input)?;
    let result = do_something(args);
    Ok(serde_json::to_string(&result)?)
}
```

### Limitations

- No filesystem access (sandboxed)
- No network access (sandboxed)
- Memory limited (configurable)
- CPU time limited

---

## Node.js Runtime

**Location**: `src/extension/runtime/nodejs/`

### Architecture

```rust
pub struct NodejsRuntime {
    processes: HashMap<String, Child>,
}

impl NodejsRuntime {
    pub async fn start(&mut self, plugin: &PluginManifest) -> Result<()> {
        let child = Command::new("node")
            .arg(&plugin.entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        self.processes.insert(plugin.name.clone(), child);
    }

    pub async fn call(
        &self,
        plugin: &str,
        method: &str,
        args: Value,
    ) -> Result<Value> {
        // JSON-RPC over stdio
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": args,
            "id": uuid()
        });

        self.send_request(plugin, request).await
    }
}
```

### Plugin Template

```typescript
// index.ts
import { createServer } from '@aleph/plugin-sdk';

const server = createServer({
  name: 'my-plugin',
  tools: {
    my_tool: async (args: { input: string }) => {
      return { result: `Processed: ${args.input}` };
    }
  }
});

server.start();
```

### SDK (TypeScript)

```typescript
// @aleph/plugin-sdk
export interface PluginServer {
  name: string;
  tools: Record<string, ToolHandler>;
}

export type ToolHandler = (args: unknown) => Promise<unknown>;

export function createServer(config: PluginServer): Server {
  return new Server(config);
}
```

---

## Plugin Discovery

**Location**: `src/extension/discovery/`

```rust
pub struct PluginDiscovery {
    search_paths: Vec<PathBuf>,
}

impl PluginDiscovery {
    pub fn discover(&self) -> Result<Vec<PluginManifest>> {
        let mut manifests = vec![];

        for path in &self.search_paths {
            for entry in fs::read_dir(path)? {
                let manifest_path = entry.path().join("aleph_plugin.toml");
                if manifest_path.exists() {
                    manifests.push(parse_manifest(&manifest_path)?);
                }
            }
        }

        manifests
    }
}
```

### Search Paths

1. `~/.aleph/plugins/` (user plugins)
2. `/usr/local/share/aleph/plugins/` (system plugins)
3. `./plugins/` (project plugins)

---

## Plugin Registry

**Location**: `src/extension/registry/`

```rust
pub struct PluginRegistry {
    plugins: HashMap<String, RegisteredPlugin>,
    tools: HashMap<String, ToolRef>,
}

pub struct RegisteredPlugin {
    pub manifest: PluginManifest,
    pub runtime: RuntimeType,
    pub status: PluginStatus,
}

pub enum PluginStatus {
    Loaded,
    Running,
    Stopped,
    Error(String),
}
```

### Registration Flow

```
Plugin Directory Found
    │
    ▼
┌─────────────────────────────────────────┐
│ 1. Parse manifest                        │
│    aleph_plugin.toml or package.json   │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 2. Validate manifest                     │
│    • Required fields                     │
│    • Version compatibility               │
│    • Tool name conflicts                 │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 3. Select runtime                        │
│    WASM → WasmRuntime                   │
│    Node.js → NodejsRuntime              │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 4. Register tools                        │
│    Add to ToolServer registry           │
└─────────────────────────────────────────┘
    │
    ▼
Plugin Ready
```

---

## Hot Reload

**Location**: `src/extension/watcher.rs`

```rust
pub struct PluginWatcher {
    watcher: RecommendedWatcher,
    registry: Arc<RwLock<PluginRegistry>>,
}

impl PluginWatcher {
    pub fn watch(&mut self, path: &Path) -> Result<()> {
        self.watcher.watch(path, RecursiveMode::Recursive)?;
    }

    async fn on_change(&self, event: Event) {
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                self.reload_plugin(&event.paths[0]).await;
            }
            EventKind::Remove(_) => {
                self.unload_plugin(&event.paths[0]).await;
            }
            _ => {}
        }
    }
}
```

---

## Skill Integration

**Location**: `src/extension/skill_tool.rs`

Skills (from `~/.claude/skills/`) are also loaded as extensions:

```rust
pub struct SkillTool {
    name: String,
    definition: SkillDefinition,
}

impl AlephToolDyn for SkillTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn call(&self, args: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            // Execute skill via prompt injection
        })
    }
}
```

---

## Configuration

```json5
{
  "extensions": {
    "enabled": true,
    "searchPaths": [
      "~/.aleph/plugins",
      "./plugins"
    ],
    "runtimes": {
      "wasm": {
        "enabled": true,
        "memoryLimit": "256MB",
        "timeoutMs": 30000
      },
      "nodejs": {
        "enabled": true,
        "nodeVersion": "20"
      }
    },
    "hotReload": true
  }
}
```

---

## Plugin RPC Methods

| Method | Description |
|--------|-------------|
| `plugins.list` | List all plugins |
| `plugins.install` | Install from path/URL |
| `plugins.uninstall` | Remove plugin |
| `plugins.enable` | Enable plugin |
| `plugins.disable` | Disable plugin |
| `plugins.reload` | Reload plugin |

---

## Extension SDK V2

The V2 SDK introduces enhanced manifest format, hook system, and prompt scopes for building powerful extensions.

### Manifest Format (aleph_plugin.toml)

V2 plugins use TOML format for better readability and Rust ecosystem alignment. The manifest priority order is:

1. `aleph_plugin.toml` (V2 TOML format) - **Preferred**
2. `aleph_plugin.json` (V2 JSON format)
3. `package.json` with `alephPlugin` section
4. Legacy manifest formats

#### Complete Example

```toml
[plugin]
id = "my-plugin"                    # Unique identifier
name = "My Plugin"                  # Display name
version = "1.0.0"                   # SemVer version
description = "Does something useful"
author = "Your Name"
kind = "nodejs"                     # nodejs | wasm | static
entry = "dist/index.js"             # Entry point for nodejs/wasm

[permissions]
network = ["connect:https://*"]     # Network permissions
filesystem = ["read:./data", "write:./output"]
env = ["API_KEY", "DEBUG"]          # Environment variables

[prompt]
file = "SKILL.md"                   # Prompt file path
scope = "system"                    # system | tool | standalone | disabled

[[tools]]
name = "my_tool"
description = "Performs a specific task"
handler = "handleMyTool"            # Function name in entry
instruction_file = "docs/INSTRUCTIONS.md"  # Tool-specific instructions

[[tools]]
name = "another_tool"
description = "Another useful tool"
handler = "handleAnotherTool"

[[hooks]]
event = "before_tool_call"
kind = "interceptor"                # interceptor | observer | resolver
priority = "normal"                 # system | high | normal | low
handler = "onBeforeTool"

[[hooks]]
event = "after_tool_call"
kind = "observer"
priority = "low"
handler = "onAfterTool"
```

### Hook Types

Hooks allow plugins to intercept and respond to system events.

| Type | Execution | Behavior |
|------|-----------|----------|
| **Interceptor** | Sequential | Can modify context or block execution. Each hook receives the result of the previous one. |
| **Observer** | Parallel | Fire-and-forget. Errors are logged but don't affect execution. Used for telemetry/logging. |

> A third `Resolver` kind (first-win competition) existed on paper but never
> gained a production fire-site and was removed under YAGNI. Configs that still
> say `"kind": "resolver"` parse to the `Observer` default rather than failing.

**Only some events accept each kind.** The single source is
`HookEvent::supports_matcher()` / `supports_interceptor()`
(`src/extension/types/hooks.rs`); both are surfaced per hook by
`hooks_manage(action="list")` and as a catalogue by
`hooks_manage(action="events")`. Two silent-death shapes to avoid:

- a `matcher` on an event whose context carries no tool name (matchers test
  `tool_name` **only**, so the hook loads and never fires);
- `"kind": "interceptor"` on an event whose fire-site dispatches observers
  only (message / provider / gateway / subagent seams).

Both are warned at load time **and** reported per hook as
`reachable: false` with an `issue` string by the runtime inventory.

#### Available Hook Events

The exhaustive list is `HookEvent::ALL`. Frequently used:

| Event | Description |
|-------|-------------|
| `before_tool_call` | Before any tool is invoked |
| `after_tool_call` | After tool execution completes |
| `session_start` / `session_end` | Session lifecycle |
| `user_prompt_submit` | Before the first provider call of a run; may inject context or halt |
| `stop` | Gate on the loop's stop (veto = keep going, with feedback) |
| `subagent_start` | When a sub-agent is spawned (observer-only; env: `SUBAGENT_ID`, `SUBAGENT_TYPE`, `TASK`, `PARENT_AGENT_ID`, `CHAIN_DEPTH`) |
| `subagent_stop` | When a sub-agent completes (observer-only; env: `SUBAGENT_ID`, `SUBAGENT_TYPE`, `OUTCOME`, `ITERATIONS`, `DURATION_MS`, `TOKENS_USED`, `KEY_FINDINGS`) |
| `message_received` / `message_sending` / `message_sent` | Channel I/O (observer-only) |
| `before_compaction` / `after_compaction` | Around history compaction |
| `pre_api_request` / `post_api_request` | Around a provider call (observer-only) |

#### Limits enforced on every hook

| Limit | Value | Why |
|-------|-------|-----|
| `timeout_secs` ceiling | 300s | Interceptor seams **await** hooks; an unclamped override would wedge the tool gate. Clamped at `HookExecutor::effective_timeout`, covering every config source. |
| stdout / stderr / HTTP body read | 64KB | Truncation is a **hard error** (fail-closed): a `deny:` printed past the cap must never be silently dropped. |
| Injected context per block | ~2500 tokens | Over-budget `context:` text is spilled to `~/.aleph/data/hook_outputs/<session>/` and replaced by a head/tail preview naming the file, so the model can still read it in full on demand. |

#### Hook Example

```typescript
// Interceptor: Can modify or block
async function onBeforeTool(context: HookContext): Promise<HookContext> {
  if (context.toolName === 'dangerous_tool') {
    throw new Error('Tool blocked by security policy');
  }
  // Modify context
  context.args.timestamp = Date.now();
  return context;
}

// Observer: Fire-and-forget
async function onAfterTool(context: HookContext): Promise<void> {
  console.log(`Tool ${context.toolName} executed in ${context.duration}ms`);
}
```

### Hook Priorities

Priorities determine execution order for interceptors.

| Priority | Value | Use Case |
|----------|-------|----------|
| **System** | -1000 | Core system hooks, runs first |
| **High** | -100 | Security checks, validation |
| **Normal** | 0 | Default priority |
| **Low** | 100 | Logging, telemetry, cleanup |

Lower values execute first. Within the same priority, hooks execute in registration order.

### Prompt Scopes

Prompt scopes control when plugin prompts are injected into the agent context.

| Scope | Behavior |
|-------|----------|
| **system** | Always injected when the plugin is active. Use for core functionality. |
| **tool** | Injected when the bound tool is available in the current context. |
| **standalone** | User must explicitly invoke (e.g., `/my-plugin`). Not auto-injected. |
| **disabled** | Never injected. Useful for temporarily disabling prompts. |

#### Prompt File Example (SKILL.md)

```markdown
# My Plugin Instructions

You have access to the my_tool function which can...

## Usage Guidelines
- Always validate input before calling
- Handle errors gracefully

## Examples
User: Do something with X
Assistant: I'll use my_tool to process X...
```

### Static Plugins

Static plugins (`kind = "static"`) contain only prompts and configuration, with no executable code:

```toml
[plugin]
id = "coding-standards"
name = "Coding Standards"
version = "1.0.0"
kind = "static"               # No entry point needed

[prompt]
file = "STANDARDS.md"
scope = "system"
```

### Migration from V1

To migrate from V1 manifest format:

1. Rename `package.json` or `aleph_plugin.json` to `aleph_plugin.toml`
2. Convert JSON structure to TOML
3. Add `kind` field (`nodejs`, `wasm`, or `static`)
4. Update `runtime.type` to `kind` and `runtime.entry` to `entry`
5. Add optional hook and prompt configurations

---

## Direct Commands (P0.5)

Direct commands bypass the LLM and execute plugin functions directly. They are useful for quick actions that don't require AI reasoning.

### What Are Direct Commands?

Unlike tools (which are called by the LLM during conversation), direct commands are invoked explicitly by the user and execute immediately without LLM involvement. This makes them:

- **Fast**: No LLM round-trip required
- **Deterministic**: Same input always produces same output
- **Explicit**: User must explicitly invoke the command

### Manifest Format

```toml
[[commands]]
name = "ping"
description = "Check if the plugin is responsive"
handler = "handlePing"

[[commands]]
name = "status"
description = "Get current plugin status"
handler = "handleStatus"

[[commands]]
name = "config"
description = "Update plugin configuration"
handler = "handleConfig"
```

### Handler Signature

```typescript
interface DirectCommandArgs {
  command: string;      // Command name
  args: string[];       // Positional arguments
  flags: Record<string, string | boolean>;  // Named flags
}

interface DirectCommandResult {
  success: boolean;
  message?: string;
  data?: unknown;
}

async function handlePing(args: DirectCommandArgs): Promise<DirectCommandResult> {
  return {
    success: true,
    message: "pong",
    data: { timestamp: Date.now() }
  };
}

async function handleConfig(args: DirectCommandArgs): Promise<DirectCommandResult> {
  const [key, value] = args.args;
  if (!key) {
    return { success: false, message: "Missing config key" };
  }
  // Update configuration...
  return { success: true, message: `Set ${key} = ${value}` };
}
```

### Gateway RPC

Execute a direct command via the Gateway:

```json
{
  "jsonrpc": "2.0",
  "method": "plugins.executeCommand",
  "params": {
    "plugin": "my-plugin",
    "command": "ping",
    "args": [],
    "flags": {}
  },
  "id": 1
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "result": {
    "success": true,
    "message": "pong",
    "data": { "timestamp": 1706000000000 }
  },
  "id": 1
}
```

---

## Background Services (P1)

Background services allow plugins to run long-lived processes that operate independently of the main request/response cycle.

### Service Lifecycle

```
┌─────────┐      start()      ┌──────────┐
│ Stopped │ ────────────────▶ │ Starting │
└─────────┘                   └──────────┘
     ▲                              │
     │                              │ ready
     │ stop()                       ▼
┌──────────┐                  ┌─────────┐
│ Stopping │ ◀──────────────── │ Running │
└──────────┘      stop()      └─────────┘
```

| State | Description |
|-------|-------------|
| **Stopped** | Service is not running |
| **Starting** | Service is initializing |
| **Running** | Service is active and processing |
| **Stopping** | Service is shutting down gracefully |

Lifecycle wiring (when services start/stop automatically):

| Event | Behavior |
|-------|----------|
| Daemon boot | `auto_start` services of active plugins are loaded and started |
| Hot-reload | Orphaned services (plugin removed/disabled on disk) are stopped; `auto_start` services of the new active set are started |
| `plugins.enable` | `auto_start` services are started |
| `plugins.disable` / `plugins.uninstall` | Plugin runtime is unloaded — its services (and transient MCP servers) are stopped first |
| Daemon shutdown | All running services are stopped (best-effort), alongside heartbeat/ACP/mDNS teardown |

### Manifest Format

> Declaring `[[services]]` requires the `background` permission
> (`[permissions] background = true` in `aleph.plugin.toml`, or
> `[aleph.permissions] background = true` in the CC-format manifest).
> Without it the services are skipped with a warning; the rest of the
> plugin still loads. Services must declare BOTH `start_handler` and
> `stop_handler` — entries missing either are skipped.

```toml
[[services]]
name = "file-watcher"
description = "Watches filesystem for changes"
start_handler = "startFileWatcher"
stop_handler = "stopFileWatcher"
auto_start = true              # Start when plugin loads (default: true)

[[services]]
name = "sync-daemon"
description = "Background sync service"
start_handler = "startSync"
stop_handler = "stopSync"
auto_start = false             # Manual start required
```

### Handler Signatures

```typescript
interface ServiceContext {
  serviceName: string;
  config: Record<string, unknown>;
  signal: AbortSignal;         // For graceful shutdown
}

// Start handler - called when service starts
async function startFileWatcher(ctx: ServiceContext): Promise<void> {
  const watcher = new FileWatcher(ctx.config.paths);

  // Listen for abort signal
  ctx.signal.addEventListener('abort', () => {
    watcher.close();
  });

  // Start watching
  await watcher.start();
}

// Stop handler - called when service stops
async function stopFileWatcher(ctx: ServiceContext): Promise<void> {
  // Cleanup resources, flush buffers, etc.
  console.log('File watcher stopped');
}
```

### ServiceManager API

The ServiceManager coordinates all background services:

```rust
pub struct ServiceManager {
    services: HashMap<String, ServiceHandle>,
}

impl ServiceManager {
    /// Start a service by name
    pub async fn start(&self, plugin: &str, service: &str) -> Result<()>;

    /// Stop a service gracefully
    pub async fn stop(&self, plugin: &str, service: &str) -> Result<()>;

    /// Get service status
    pub fn status(&self, plugin: &str, service: &str) -> Option<ServiceStatus>;

    /// List all services
    pub fn list(&self) -> Vec<ServiceInfo>;
}

pub struct ServiceInfo {
    pub plugin: String,
    pub name: String,
    pub status: ServiceStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_secs: Option<u64>,
}

pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error(String),
}
```

### Gateway RPCs

| Method | Description |
|--------|-------------|
| `services.start` | Start a background service |
| `services.stop` | Stop a running service |
| `services.list` | List all services with status |
| `services.status` | Get status of a specific service |

#### Start Service

```json
{
  "jsonrpc": "2.0",
  "method": "services.start",
  "params": {
    "plugin": "my-plugin",
    "service": "file-watcher"
  },
  "id": 1
}
```

#### Stop Service

```json
{
  "jsonrpc": "2.0",
  "method": "services.stop",
  "params": {
    "plugin": "my-plugin",
    "service": "file-watcher"
  },
  "id": 2
}
```

#### List Services

```json
{
  "jsonrpc": "2.0",
  "method": "services.list",
  "params": {},
  "id": 3
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "result": [
    {
      "plugin": "my-plugin",
      "name": "file-watcher",
      "status": "running",
      "started_at": "2026-02-03T10:00:00Z",
      "uptime_secs": 3600
    },
    {
      "plugin": "my-plugin",
      "name": "sync-daemon",
      "status": "stopped",
      "started_at": null,
      "uptime_secs": null
    }
  ],
  "id": 3
}
```

---

## Channel / Provider / HTTP-Route Plugins — ❌ 不存在（2026-08-19 更正）

这里曾有三节共 ~490 行，描述插件如何贡献 **channel**、**provider** 和
**HTTP route**：manifest 格式、handler 命名约定、`ChannelManager` API、
`PluginProviderAdapter` API、路径参数语法。

**三者在代码里都不存在，一行也没有。**

- `[[channels]]` / `[[aleph.channels]]`：`grep -rn "ChannelDeclaration\|aleph.channels" src/`
  零命中。`AlephExtensionsToml` 只有 `runtime` / `entry` / `permissions` /
  `capabilities` / `services` / `tools` / `hooks` / `commands` / `prompt` /
  `config_schema` / `config_ui_hints` / `memory`。
- `[[providers]]`：同上，`ProviderDeclaration` 零命中。
- `[[http_routes]]`：`http_route` / `HttpRoute` 在 `src/` 下零命中；字段根本不解析。

`CapabilityDeclaration` 的**全部**变体是
`Tool | Hook | Service | Skill | Agent | McpServer` —— 那是一个插件今天能贡献的
完整集合。声明上面任何一段只会被 serde 静默丢弃（未知键），插件照样加载，
而作者会一直等一个永远不会被调用的 handler。

### 那要怎么加一个 channel / provider？

改 core：channel 落在 `src/gateway/channel*` + `interfaces/<channel>/`，
provider 落在 `src/providers/`。两者都要在
`aleph_protocol::channels::CONFIGURABLE_CHANNEL_TYPES` 这类单一源上登记
（见 GATEWAY.md 关于「加了 adapter ≠ 用户能配」那条判据）。

对**外部服务**而言，`runtime = "mcp"` 的插件已经是可用的答案：MCP server 能提供
工具，而工具是模型真正会调的东西。

> 这一节保留而不是删干净，因为一个搜索 `[[channels]]` 的作者会先找到 git 历史里的
> 旧文档；让他在同一个位置读到「这从来没有过」比什么都读不到便宜。

---

## See Also

- [Aleph Hub](ALEPH_HUB.md) - Extension **distribution**: catalog contract, trust rails, install pipeline (this document covers the **runtime** that loads what the Hub installs)
- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How tools work
- [Gateway](GATEWAY.md) - Plugin RPC methods
