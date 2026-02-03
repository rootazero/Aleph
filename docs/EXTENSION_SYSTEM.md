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

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How tools work
- [Gateway](GATEWAY.md) - Plugin RPC methods
