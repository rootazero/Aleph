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

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How tools work
- [Gateway](GATEWAY.md) - Plugin RPC methods
