# Extension System

> Plugin architecture with WASM and Node.js runtimes

---

## Overview

Aether's extension system allows third-party tools via:
- **WASM Plugins**: Fast, sandboxed WebAssembly modules
- **Node.js Plugins**: JavaScript/TypeScript extensions
- **Manifest-driven**: Declarative plugin definitions

**Location**: `core/src/extension/`

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
~/.aether/plugins/
├── my-plugin/
│   ├── aether_plugin.toml    # Plugin manifest
│   ├── package.json          # (Node.js) or
│   ├── plugin.wasm           # (WASM)
│   └── src/
│       └── index.ts
└── another-plugin/
    └── ...
```

### Manifest (aether_plugin.toml)

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

**Location**: `core/src/extension/runtime/wasm/`

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

**Location**: `core/src/extension/runtime/nodejs/`

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
import { createServer } from '@aether/plugin-sdk';

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
// @aether/plugin-sdk
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

**Location**: `core/src/extension/discovery/`

```rust
pub struct PluginDiscovery {
    search_paths: Vec<PathBuf>,
}

impl PluginDiscovery {
    pub fn discover(&self) -> Result<Vec<PluginManifest>> {
        let mut manifests = vec![];

        for path in &self.search_paths {
            for entry in fs::read_dir(path)? {
                let manifest_path = entry.path().join("aether_plugin.toml");
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

1. `~/.aether/plugins/` (user plugins)
2. `/usr/local/share/aether/plugins/` (system plugins)
3. `./plugins/` (project plugins)

---

## Plugin Registry

**Location**: `core/src/extension/registry/`

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
│    aether_plugin.toml or package.json   │
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

**Location**: `core/src/extension/watcher.rs`

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

**Location**: `core/src/extension/skill_tool.rs`

Skills (from `~/.claude/skills/`) are also loaded as extensions:

```rust
pub struct SkillTool {
    name: String,
    definition: SkillDefinition,
}

impl AetherToolDyn for SkillTool {
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
      "~/.aether/plugins",
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

### Manifest Format (aether_plugin.toml)

V2 plugins use TOML format for better readability and Rust ecosystem alignment. The manifest priority order is:

1. `aether_plugin.toml` (V2 TOML format) - **Preferred**
2. `aether_plugin.json` (V2 JSON format)
3. `package.json` with `aetherPlugin` section
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
| **Resolver** | Sequential | First-win competition. Execution stops when a hook returns a non-null result. |

#### Available Hook Events

| Event | Description |
|-------|-------------|
| `before_tool_call` | Before any tool is invoked |
| `after_tool_call` | After tool execution completes |
| `on_message` | When a user message is received |
| `on_response` | Before response is sent to user |
| `on_error` | When an error occurs |

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

// Resolver: First-win
async function resolveConfig(context: HookContext): Promise<Config | null> {
  if (context.key in myConfigs) {
    return myConfigs[context.key];  // Wins, stops chain
  }
  return null;  // Continue to next resolver
}
```

### Hook Priorities

Priorities determine execution order for interceptors and resolvers.

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

1. Rename `package.json` or `aether_plugin.json` to `aether_plugin.toml`
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

### Manifest Format

```toml
[[services]]
name = "file-watcher"
description = "Watches filesystem for changes"
start_handler = "startFileWatcher"
stop_handler = "stopFileWatcher"
auto_start = true              # Start when plugin loads

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

## Channel Plugins (P2)

Channel plugins enable messaging integrations, allowing Aether to communicate through various platforms like Telegram, Discord, Slack, or custom messaging systems.

### What Are Channel Plugins?

Channels are bidirectional messaging integrations. They:

- Receive messages from external platforms
- Send responses back to those platforms
- Maintain connection state
- Handle platform-specific formatting

### Manifest Format

```toml
[[channels]]
name = "telegram"
description = "Telegram bot integration"
connect_handler = "connectTelegram"
disconnect_handler = "disconnectTelegram"
send_handler = "sendTelegram"

[[channels]]
name = "slack"
description = "Slack workspace integration"
connect_handler = "connectSlack"
disconnect_handler = "disconnectSlack"
send_handler = "sendSlack"
```

### Types

```typescript
interface ChannelMessage {
  channel: string;          // Channel name
  chat_id: string;          // Platform-specific chat identifier
  sender_id: string;        // Platform-specific sender identifier
  sender_name?: string;     // Display name
  content: string;          // Message content
  attachments?: Attachment[];
  metadata?: Record<string, unknown>;
  timestamp: number;
}

interface ChannelSendRequest {
  channel: string;
  chat_id: string;
  content: string;
  reply_to?: string;        // Message ID to reply to
  attachments?: Attachment[];
  format?: 'text' | 'markdown' | 'html';
}

interface ChannelState {
  connected: boolean;
  connecting: boolean;
  error?: string;
  last_activity?: number;
}

interface ChannelInfo {
  name: string;
  description: string;
  state: ChannelState;
  capabilities: string[];   // ['markdown', 'attachments', 'reactions']
}

interface Attachment {
  type: 'image' | 'file' | 'audio' | 'video';
  url?: string;
  data?: Uint8Array;
  filename?: string;
  mime_type?: string;
}
```

### Handler Signatures

```typescript
interface ChannelContext {
  channelName: string;
  config: Record<string, unknown>;
  onMessage: (msg: ChannelMessage) => void;  // Callback for incoming messages
}

// Connect to the channel
async function connectTelegram(ctx: ChannelContext): Promise<void> {
  const bot = new TelegramBot(ctx.config.token);

  bot.on('message', (msg) => {
    ctx.onMessage({
      channel: 'telegram',
      chat_id: String(msg.chat.id),
      sender_id: String(msg.from.id),
      sender_name: msg.from.first_name,
      content: msg.text || '',
      timestamp: Date.now()
    });
  });

  await bot.start();
}

// Disconnect from the channel
async function disconnectTelegram(ctx: ChannelContext): Promise<void> {
  // Cleanup, close connections
}

// Send a message through the channel
async function sendTelegram(
  ctx: ChannelContext,
  request: ChannelSendRequest
): Promise<void> {
  const bot = getBot(ctx);
  await bot.sendMessage(request.chat_id, request.content, {
    parse_mode: request.format === 'markdown' ? 'MarkdownV2' : undefined,
    reply_to_message_id: request.reply_to
  });
}
```

### ChannelManager API

```rust
pub struct ChannelManager {
    channels: HashMap<String, ChannelHandle>,
}

impl ChannelManager {
    /// Connect a channel
    pub async fn connect(&self, plugin: &str, channel: &str) -> Result<()>;

    /// Disconnect a channel
    pub async fn disconnect(&self, plugin: &str, channel: &str) -> Result<()>;

    /// Send message through a channel
    pub async fn send(&self, request: ChannelSendRequest) -> Result<()>;

    /// Get channel info
    pub fn info(&self, plugin: &str, channel: &str) -> Option<ChannelInfo>;

    /// List all channels
    pub fn list(&self) -> Vec<ChannelInfo>;
}
```

### Gateway RPCs

| Method | Description |
|--------|-------------|
| `channels.connect` | Connect to a channel |
| `channels.disconnect` | Disconnect from a channel |
| `channels.send` | Send message through a channel |
| `channels.list` | List all channels |
| `channels.info` | Get channel info |

---

## Provider Plugins (P2)

Provider plugins allow custom AI backends to be integrated into Aether, enabling support for self-hosted models, specialized APIs, or proprietary systems.

### What Are Provider Plugins?

Providers handle AI model interactions. A provider plugin:

- Defines available models
- Handles chat completion requests
- Supports streaming responses
- Manages authentication and rate limiting

### Manifest Format

```toml
[[providers]]
name = "local-llama"
description = "Local Llama.cpp server"
list_models_handler = "listModels"
chat_handler = "handleChat"
stream_handler = "handleStream"       # Optional: for streaming support
embed_handler = "handleEmbed"         # Optional: for embeddings
```

### Types

```typescript
interface ProviderModelInfo {
  id: string;                 // Model identifier
  name: string;               // Display name
  provider: string;           // Provider name
  context_length: number;     // Max context window
  capabilities: string[];     // ['chat', 'vision', 'function_calling']
  pricing?: {
    input_per_1k: number;
    output_per_1k: number;
  };
}

interface ProviderChatRequest {
  model: string;
  messages: Message[];
  temperature?: number;
  max_tokens?: number;
  tools?: ToolDefinition[];
  stream?: boolean;
}

interface Message {
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string | ContentPart[];
  tool_calls?: ToolCall[];
  tool_call_id?: string;
}

interface ProviderChatResponse {
  id: string;
  model: string;
  message: Message;
  usage: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
  finish_reason: 'stop' | 'length' | 'tool_calls';
}

interface ProviderStreamChunk {
  id: string;
  delta: {
    content?: string;
    tool_calls?: ToolCallDelta[];
  };
  finish_reason?: 'stop' | 'length' | 'tool_calls';
}
```

### Handler Signatures

```typescript
interface ProviderContext {
  providerName: string;
  config: Record<string, unknown>;
}

// List available models
async function listModels(ctx: ProviderContext): Promise<ProviderModelInfo[]> {
  return [
    {
      id: 'llama-3.1-70b',
      name: 'Llama 3.1 70B',
      provider: ctx.providerName,
      context_length: 128000,
      capabilities: ['chat', 'function_calling']
    }
  ];
}

// Handle chat completion
async function handleChat(
  ctx: ProviderContext,
  request: ProviderChatRequest
): Promise<ProviderChatResponse> {
  const response = await fetch(`${ctx.config.baseUrl}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request)
  });

  return response.json();
}

// Handle streaming chat completion
async function* handleStream(
  ctx: ProviderContext,
  request: ProviderChatRequest
): AsyncGenerator<ProviderStreamChunk> {
  const response = await fetch(`${ctx.config.baseUrl}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ...request, stream: true })
  });

  const reader = response.body.getReader();
  const decoder = new TextDecoder();

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    const lines = decoder.decode(value).split('\n');
    for (const line of lines) {
      if (line.startsWith('data: ') && line !== 'data: [DONE]') {
        yield JSON.parse(line.slice(6));
      }
    }
  }
}
```

### PluginProviderAdapter API

The adapter bridges plugin providers with Aether's provider system:

```rust
pub struct PluginProviderAdapter {
    plugin: String,
    provider: String,
    runtime: Arc<dyn PluginRuntime>,
}

impl AetherProvider for PluginProviderAdapter {
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream>;
}
```

### Gateway RPCs

| Method | Description |
|--------|-------------|
| `providers.list` | List all providers (including plugins) |
| `providers.models` | List models for a provider |
| `providers.chat` | Send chat request |

---

## HTTP Routes (P2)

HTTP routes allow plugins to expose REST endpoints, enabling webhooks, integrations, and custom APIs.

### What Are HTTP Routes?

HTTP routes let plugins:

- Receive webhooks from external services
- Expose custom REST APIs
- Serve static content
- Handle OAuth callbacks

### Manifest Format

```toml
[[http_routes]]
method = "GET"
path = "/api/status"
handler = "handleStatus"

[[http_routes]]
method = "POST"
path = "/webhooks/github"
handler = "handleGithubWebhook"

[[http_routes]]
method = "GET"
path = "/users/{user_id}/profile"
handler = "handleUserProfile"

[[http_routes]]
method = "PUT"
path = "/items/{category}/{item_id}"
handler = "handleUpdateItem"
```

### Path Parameter Syntax

Path parameters use curly braces `{param}`:

- `/users/{id}` - Captures `id` parameter
- `/files/{path*}` - Captures remaining path (wildcard)
- `/api/{version}/items/{id}` - Multiple parameters

### Types

```typescript
interface HttpRequest {
  method: 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';
  path: string;
  params: Record<string, string>;     // Path parameters
  query: Record<string, string>;      // Query parameters
  headers: Record<string, string>;
  body?: unknown;                      // Parsed JSON body
  raw_body?: Uint8Array;              // Raw body bytes
}

interface HttpResponse {
  status: number;
  headers?: Record<string, string>;
  body?: unknown;                      // JSON response
  raw_body?: Uint8Array;              // Raw bytes
}
```

### Handler Signatures

```typescript
interface HttpContext {
  pluginName: string;
  config: Record<string, unknown>;
}

// Simple GET handler
async function handleStatus(
  ctx: HttpContext,
  req: HttpRequest
): Promise<HttpResponse> {
  return {
    status: 200,
    body: { status: 'ok', version: '1.0.0' }
  };
}

// POST handler with body
async function handleGithubWebhook(
  ctx: HttpContext,
  req: HttpRequest
): Promise<HttpResponse> {
  const event = req.headers['x-github-event'];
  const payload = req.body as GithubPayload;

  // Verify signature
  const signature = req.headers['x-hub-signature-256'];
  if (!verifySignature(req.raw_body, signature, ctx.config.secret)) {
    return { status: 401, body: { error: 'Invalid signature' } };
  }

  // Process webhook
  await processGithubEvent(event, payload);

  return { status: 200, body: { received: true } };
}

// Handler with path parameters
async function handleUserProfile(
  ctx: HttpContext,
  req: HttpRequest
): Promise<HttpResponse> {
  const userId = req.params.user_id;
  const user = await getUser(userId);

  if (!user) {
    return { status: 404, body: { error: 'User not found' } };
  }

  return { status: 200, body: user };
}
```

### PluginHttpHandler API

```rust
pub struct PluginHttpHandler {
    plugin: String,
    routes: Vec<RouteDefinition>,
    runtime: Arc<dyn PluginRuntime>,
}

impl PluginHttpHandler {
    /// Handle an HTTP request
    pub async fn handle(&self, req: HttpRequest) -> Result<HttpResponse>;

    /// Check if a route matches
    pub fn matches(&self, method: &str, path: &str) -> bool;

    /// Get all registered routes
    pub fn routes(&self) -> &[RouteDefinition];
}

pub struct RouteDefinition {
    pub method: HttpMethod,
    pub path: String,
    pub handler: String,
}
```

### Gateway Integration

HTTP routes are served under `/plugins/{plugin_name}/`:

```
GET  /plugins/my-plugin/api/status
POST /plugins/my-plugin/webhooks/github
GET  /plugins/my-plugin/users/123/profile
```

The Gateway's HTTP server routes requests to the appropriate plugin handler.

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How tools work
- [Gateway](GATEWAY.md) - Plugin RPC methods
