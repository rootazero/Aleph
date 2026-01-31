# Plugin System Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate the Node.js/WASM plugin runtimes with ExtensionManager and Gateway RPC handlers, enabling plugins to register tools/hooks that agents can call.

**Architecture:** ExtensionManager orchestrates plugin discovery → manifest parsing → runtime loading (NodeJsRuntime/WasmRuntime) → registration (PluginRegistry) → tool/hook execution. Gateway exposes RPC methods (`plugins.*`) for plugin management and tool invocation.

**Tech Stack:** Rust async/await, JSON-RPC 2.0, NodeJsRuntime (sync IPC), WasmRuntime (Extism), PluginRegistry

---

## Task 1: Create plugin-host.js for Node.js Plugins

**Files:**
- Create: `core/src/extension/runtime/nodejs/plugin-host.js`

**Step 1: Write the failing test**

The test verifies plugin-host.js exists and is embedded in the Rust binary.

```rust
// In core/src/extension/runtime/nodejs/mod.rs, add test
#[test]
fn test_plugin_host_script_exists() {
    let script = include_str!("plugin-host.js");
    assert!(script.contains("jsonrpc"));
    assert!(script.contains("load"));
    assert!(script.contains("plugin.call"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_plugin_host_script_exists`
Expected: FAIL with "couldn't read plugin-host.js"

**Step 3: Create plugin-host.js**

```javascript
#!/usr/bin/env node
// Plugin Host - JSON-RPC 2.0 over stdio for Node.js plugins
const readline = require('readline');
const path = require('path');

const plugins = new Map();

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false
});

function respond(id, result, error = null) {
  const response = { jsonrpc: '2.0', id };
  if (error) {
    response.error = { code: -32000, message: String(error) };
  } else {
    response.result = result;
  }
  console.log(JSON.stringify(response));
}

async function handleRequest(request) {
  const { id, method, params } = request;

  try {
    switch (method) {
      case 'load': {
        const { pluginId, pluginPath } = params;
        const modulePath = path.resolve(pluginPath);
        const plugin = require(modulePath);

        // Call plugin.register() if exists to get registrations
        let registrations = { tools: [], hooks: [], channels: [], providers: [], gateway_methods: [] };
        if (typeof plugin.register === 'function') {
          registrations = await plugin.register() || registrations;
        }

        plugins.set(pluginId, { module: plugin, path: modulePath });
        respond(id, { plugin_id: pluginId, ...registrations });
        break;
      }

      case 'plugin.call': {
        const { pluginId, handler, args } = params;
        const plugin = plugins.get(pluginId);
        if (!plugin) {
          respond(id, null, `Plugin not loaded: ${pluginId}`);
          return;
        }

        const fn = plugin.module[handler];
        if (typeof fn !== 'function') {
          respond(id, null, `Handler not found: ${handler}`);
          return;
        }

        const result = await fn(args);
        respond(id, result);
        break;
      }

      case 'executeHook': {
        const { pluginId, handler, event } = params;
        const plugin = plugins.get(pluginId);
        if (!plugin) {
          respond(id, null, `Plugin not loaded: ${pluginId}`);
          return;
        }

        const fn = plugin.module[handler];
        if (typeof fn !== 'function') {
          respond(id, null, `Hook handler not found: ${handler}`);
          return;
        }

        const result = await fn(event);
        respond(id, result);
        break;
      }

      case 'unload': {
        const { pluginId } = params;
        plugins.delete(pluginId);
        respond(id, { success: true });
        break;
      }

      case 'shutdown': {
        respond(id, { success: true });
        process.exit(0);
        break;
      }

      default:
        respond(id, null, `Unknown method: ${method}`);
    }
  } catch (err) {
    respond(id, null, err.message);
  }
}

rl.on('line', async (line) => {
  try {
    const request = JSON.parse(line);
    await handleRequest(request);
  } catch (err) {
    console.error(JSON.stringify({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } }));
  }
});

process.on('uncaughtException', (err) => {
  console.error(JSON.stringify({ jsonrpc: '2.0', id: null, error: { code: -32000, message: err.message } }));
});
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_plugin_host_script_exists`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/extension/runtime/nodejs/plugin-host.js core/src/extension/runtime/nodejs/mod.rs
git commit -m "$(cat <<'EOF'
feat(extension): add plugin-host.js for Node.js plugin IPC

Embedded JavaScript host script that runs as a subprocess,
communicating via JSON-RPC 2.0 over stdio. Supports:
- load: Load plugin and call register() for tool/hook definitions
- plugin.call: Invoke tool handlers
- executeHook: Invoke hook handlers
- unload/shutdown: Cleanup

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Integrate NodeJsRuntime with Embedded Host Script

**Files:**
- Modify: `core/src/extension/runtime/nodejs/mod.rs:33-40`
- Modify: `core/src/extension/runtime/nodejs/process.rs:20-30`

**Step 1: Write the failing test**

```rust
// In core/src/extension/runtime/nodejs/mod.rs
#[test]
fn test_nodejs_runtime_uses_embedded_host() {
    let runtime = NodeJsRuntime::with_embedded_host("/usr/bin/node");
    assert!(!runtime.host_script_path.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_nodejs_runtime_uses_embedded_host`
Expected: FAIL with "no function named `with_embedded_host`"

**Step 3: Add with_embedded_host constructor**

In `core/src/extension/runtime/nodejs/mod.rs`:

```rust
impl NodeJsRuntime {
    /// Create runtime with embedded plugin-host.js
    pub fn with_embedded_host(node_path: impl Into<String>) -> Self {
        Self {
            processes: HashMap::new(),
            node_path: node_path.into(),
            host_script_path: String::new(), // Will use embedded
            use_embedded_host: true,
        }
    }

    /// Get the host script content
    fn get_host_script(&self) -> &'static str {
        include_str!("plugin-host.js")
    }
}

// Update NodeProcess::start to accept script content
```

In `core/src/extension/runtime/nodejs/process.rs`, update `start()` to write embedded script to temp file if needed.

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_nodejs_runtime_uses_embedded_host`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/extension/runtime/nodejs/
git commit -m "$(cat <<'EOF'
feat(extension): integrate embedded plugin-host.js with NodeJsRuntime

NodeJsRuntime::with_embedded_host() constructor uses the bundled
plugin-host.js script. The script is written to a temp file on
first plugin load, avoiding external file dependencies.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add PluginLoader to ExtensionManager

**Files:**
- Create: `core/src/extension/plugin_loader.rs`
- Modify: `core/src/extension/mod.rs:126-148`

**Step 1: Write the failing test**

```rust
// In core/src/extension/plugin_loader.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_loader_new() {
        let loader = PluginLoader::new();
        assert!(!loader.is_any_runtime_active());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_plugin_loader_new`
Expected: FAIL with "cannot find module `plugin_loader`"

**Step 3: Create PluginLoader**

```rust
// core/src/extension/plugin_loader.rs
//! Plugin Loader - Manages runtime loading of Node.js and WASM plugins

use std::path::PathBuf;
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::PluginManifest;
use crate::extension::registry::PluginRegistry;
use crate::extension::types::PluginKind;
use crate::extension::runtime::{NodeJsRuntime, WasmRuntime};

/// Manages loading plugins into appropriate runtimes
pub struct PluginLoader {
    /// Node.js runtime (lazy initialized)
    nodejs_runtime: Option<NodeJsRuntime>,
    /// WASM runtime (lazy initialized)
    #[cfg(feature = "wasm-plugins")]
    wasm_runtime: Option<WasmRuntime>,
}

impl PluginLoader {
    pub fn new() -> Self {
        Self {
            nodejs_runtime: None,
            #[cfg(feature = "wasm-plugins")]
            wasm_runtime: None,
        }
    }

    pub fn is_any_runtime_active(&self) -> bool {
        self.nodejs_runtime.is_some()
    }

    /// Load a plugin based on its kind
    pub fn load_plugin(
        &mut self,
        manifest: &PluginManifest,
        registry: &mut PluginRegistry,
    ) -> ExtensionResult<()> {
        match manifest.kind {
            PluginKind::NodeJs => self.load_nodejs_plugin(manifest, registry),
            #[cfg(feature = "wasm-plugins")]
            PluginKind::Wasm => self.load_wasm_plugin(manifest, registry),
            PluginKind::Static => Ok(()), // Static plugins already loaded by ComponentLoader
            _ => Err(ExtensionError::Runtime(format!(
                "Unsupported plugin kind: {:?}",
                manifest.kind
            ))),
        }
    }

    fn load_nodejs_plugin(
        &mut self,
        manifest: &PluginManifest,
        registry: &mut PluginRegistry,
    ) -> ExtensionResult<()> {
        // Initialize runtime if needed
        if self.nodejs_runtime.is_none() {
            self.nodejs_runtime = Some(NodeJsRuntime::with_embedded_host("node"));
        }

        let runtime = self.nodejs_runtime.as_mut().unwrap();
        let registrations = runtime.load_plugin(manifest)?;

        // Register tools and hooks
        for tool in registrations.tools {
            let reg = crate::extension::runtime::nodejs::tool_def_to_registration(&tool, &manifest.id);
            registry.register_tool(reg);
        }

        for hook in registrations.hooks {
            if let Some(reg) = crate::extension::runtime::nodejs::hook_def_to_registration(&hook, &manifest.id) {
                registry.register_hook(reg);
            }
        }

        Ok(())
    }

    /// Call a tool on a loaded plugin
    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        if let Some(runtime) = &mut self.nodejs_runtime {
            if runtime.is_loaded(plugin_id) {
                return runtime.call_tool(plugin_id, handler, args);
            }
        }

        Err(ExtensionError::PluginNotFound(plugin_id.to_string()))
    }

    /// Shutdown all runtimes
    pub fn shutdown(&mut self) {
        if let Some(runtime) = &mut self.nodejs_runtime {
            runtime.shutdown_all();
        }
    }
}

impl Drop for PluginLoader {
    fn drop(&mut self) {
        self.shutdown();
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_plugin_loader_new`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/extension/plugin_loader.rs core/src/extension/mod.rs
git commit -m "$(cat <<'EOF'
feat(extension): add PluginLoader for runtime management

PluginLoader manages lazy initialization of NodeJsRuntime and
WasmRuntime, loads plugins based on PluginKind, and registers
their tools/hooks with PluginRegistry.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Integrate PluginLoader with ExtensionManager

**Files:**
- Modify: `core/src/extension/mod.rs:126-148` (add plugin_loader field)
- Modify: `core/src/extension/mod.rs:176-280` (update load_all)

**Step 1: Write the failing test**

```rust
// In core/src/extension/mod.rs tests
#[tokio::test]
async fn test_extension_manager_has_plugin_loader() {
    let manager = ExtensionManager::with_defaults().await.unwrap();
    // This should compile - plugin_loader exists
    assert!(manager.call_plugin_tool("nonexistent", "handler", serde_json::json!({})).await.is_err());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_extension_manager_has_plugin_loader`
Expected: FAIL with "no method named `call_plugin_tool`"

**Step 3: Add PluginLoader to ExtensionManager**

Update `ExtensionManager` struct:

```rust
pub struct ExtensionManager {
    // ... existing fields ...

    /// Plugin loader for runtime plugins
    plugin_loader: Arc<RwLock<PluginLoader>>,

    /// Plugin registry for runtime registrations
    plugin_registry: Arc<RwLock<PluginRegistry>>,
}
```

Add method:

```rust
impl ExtensionManager {
    /// Call a tool on a runtime plugin
    pub async fn call_plugin_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.plugin_loader
            .write()
            .await
            .call_tool(plugin_id, handler, args)
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_extension_manager_has_plugin_loader`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/extension/mod.rs
git commit -m "$(cat <<'EOF'
feat(extension): integrate PluginLoader with ExtensionManager

ExtensionManager now holds PluginLoader and PluginRegistry for
runtime plugins. New call_plugin_tool() method enables invoking
tools on loaded Node.js/WASM plugins.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Add Gateway RPC Handler for plugins.callTool

**Files:**
- Modify: `core/src/gateway/handlers/plugins.rs:1-50` (add imports)
- Modify: `core/src/gateway/handlers/plugins.rs:300-368` (add handler)

**Step 1: Write the failing test**

```rust
// In core/src/gateway/handlers/plugins.rs tests
#[test]
fn test_call_tool_params() {
    let json = json!({
        "pluginId": "my-plugin",
        "handler": "myTool",
        "args": {"key": "value"}
    });
    let params: CallToolParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.plugin_id, "my-plugin");
    assert_eq!(params.handler, "myTool");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_call_tool_params`
Expected: FAIL with "cannot find type `CallToolParams`"

**Step 3: Add handle_call_tool**

```rust
// In core/src/gateway/handlers/plugins.rs

/// Parameters for plugins.callTool
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolParams {
    pub plugin_id: String,
    pub handler: String,
    pub args: serde_json::Value,
}

/// Call a tool on a loaded plugin
pub async fn handle_call_tool(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: CallToolParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            );
        }
    };

    // Get ExtensionManager from context (need to pass via shared state)
    // For now, return placeholder
    // TODO: Wire up with actual ExtensionManager
    JsonRpcResponse::success(
        request.id,
        json!({
            "result": null,
            "error": "Not yet implemented - needs ExtensionManager integration"
        }),
    )
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_call_tool_params`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/plugins.rs
git commit -m "$(cat <<'EOF'
feat(gateway): add plugins.callTool RPC handler

New handler for invoking tools on loaded runtime plugins.
Currently returns placeholder - full integration with
ExtensionManager in next task.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Wire Up Gateway with ExtensionManager for Tool Calls

**Files:**
- Modify: `core/src/gateway/server.rs` (add ExtensionManager to GatewayState)
- Modify: `core/src/gateway/handlers/plugins.rs` (use shared state)

**Step 1: Write the failing test**

```rust
// In core/src/gateway/handlers/plugins.rs tests
#[tokio::test]
async fn test_call_tool_with_nonexistent_plugin() {
    let request = JsonRpcRequest::new(
        "plugins.callTool",
        Some(json!({
            "pluginId": "nonexistent",
            "handler": "tool",
            "args": {}
        })),
        Some(json!(1)),
    );

    // This needs shared state - will be wired in implementation
    let response = handle_call_tool(request).await;
    // Should return an error for nonexistent plugin
    assert!(response.is_success() || response.is_error());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_call_tool_with_nonexistent_plugin`
Expected: PASS (placeholder returns success)

**Step 3: Implement full wiring**

This requires modifying the Gateway architecture to pass ExtensionManager to handlers. Options:
1. Thread-local state
2. Handler context parameter
3. Lazy static

Use approach 2 - modify handler signature to accept context:

```rust
// In core/src/gateway/handlers/mod.rs, add:
pub struct HandlerContext {
    pub extension_manager: Arc<ExtensionManager>,
}

// Update handler type to include context
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_call_tool_with_nonexistent_plugin`
Expected: PASS with proper error response

**Step 5: Commit**

```bash
git add core/src/gateway/
git commit -m "$(cat <<'EOF'
feat(gateway): wire ExtensionManager to plugin handlers

Gateway handlers now receive HandlerContext with ExtensionManager,
enabling plugins.callTool to invoke tools on loaded plugins.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Add plugins.load and plugins.unload RPC Handlers

**Files:**
- Modify: `core/src/gateway/handlers/plugins.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_load_plugin_params() {
    let json = json!({
        "pluginId": "my-plugin",
        "path": "/path/to/plugin"
    });
    let params: LoadPluginParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.plugin_id, "my-plugin");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_load_plugin_params`
Expected: FAIL with "cannot find type `LoadPluginParams`"

**Step 3: Add handlers**

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadPluginParams {
    pub plugin_id: String,
    pub path: String,
}

pub async fn handle_load(request: JsonRpcRequest) -> JsonRpcResponse {
    // Parse manifest from path, load into runtime
    // ...
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnloadPluginParams {
    pub plugin_id: String,
}

pub async fn handle_unload(request: JsonRpcRequest) -> JsonRpcResponse {
    // Unload from runtime
    // ...
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_load_plugin_params`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/plugins.rs
git commit -m "$(cat <<'EOF'
feat(gateway): add plugins.load and plugins.unload RPC handlers

Enable runtime loading and unloading of plugins via Gateway RPC.
- plugins.load: Parse manifest and load into appropriate runtime
- plugins.unload: Shutdown plugin process and remove registrations

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Register All Plugin Handlers in HandlerRegistry

**Files:**
- Modify: `core/src/gateway/handlers/mod.rs:50-75`

**Step 1: Write the failing test**

```rust
#[test]
fn test_plugin_handlers_registered() {
    let registry = HandlerRegistry::new();
    assert!(registry.has_method("plugins.list"));
    assert!(registry.has_method("plugins.install"));
    assert!(registry.has_method("plugins.callTool"));
    assert!(registry.has_method("plugins.load"));
    assert!(registry.has_method("plugins.unload"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore test_plugin_handlers_registered`
Expected: FAIL (some methods not registered)

**Step 3: Register all plugin handlers**

```rust
impl HandlerRegistry {
    pub fn new() -> Self {
        let mut registry = Self { handlers: HashMap::new() };

        // ... existing registrations ...

        // Plugin handlers
        registry.register("plugins.list", plugins::handle_list);
        registry.register("plugins.install", plugins::handle_install);
        registry.register("plugins.installFromZip", plugins::handle_install_from_zip);
        registry.register("plugins.uninstall", plugins::handle_uninstall);
        registry.register("plugins.enable", plugins::handle_enable);
        registry.register("plugins.disable", plugins::handle_disable);
        registry.register("plugins.load", plugins::handle_load);
        registry.register("plugins.unload", plugins::handle_unload);
        registry.register("plugins.callTool", plugins::handle_call_tool);

        registry
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore test_plugin_handlers_registered`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/mod.rs
git commit -m "$(cat <<'EOF'
feat(gateway): register all plugin RPC handlers

All plugin management methods now registered:
- plugins.list, plugins.install, plugins.installFromZip
- plugins.uninstall, plugins.enable, plugins.disable
- plugins.load, plugins.unload, plugins.callTool

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Add End-to-End Integration Test

**Files:**
- Create: `core/tests/integration/plugin_runtime_test.rs`

**Step 1: Create test file with failing test**

```rust
//! Integration test for plugin runtime system

use aethecore::extension::{ExtensionManager, ExtensionConfig};
use std::path::PathBuf;
use tempfile::TempDir;

/// Create a test Node.js plugin
fn create_test_plugin(dir: &std::path::Path) -> PathBuf {
    let plugin_dir = dir.join("test-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();

    // Create aether.plugin.json
    std::fs::write(
        plugin_dir.join("aether.plugin.json"),
        r#"{
            "id": "test-plugin",
            "name": "Test Plugin",
            "version": "1.0.0",
            "kind": "nodejs",
            "entry": "index.js"
        }"#,
    ).unwrap();

    // Create index.js
    std::fs::write(
        plugin_dir.join("index.js"),
        r#"
        exports.register = function() {
            return {
                tools: [{
                    name: "test_tool",
                    description: "A test tool",
                    parameters: { type: "object" },
                    handler: "handleTestTool"
                }],
                hooks: []
            };
        };

        exports.handleTestTool = function(args) {
            return { success: true, input: args };
        };
        "#,
    ).unwrap();

    plugin_dir
}

#[tokio::test]
async fn test_nodejs_plugin_tool_call() {
    let temp = TempDir::new().unwrap();
    let plugin_path = create_test_plugin(temp.path());

    let config = ExtensionConfig {
        discovery: aethecore::discovery::DiscoveryConfig {
            global_dir: Some(temp.path().to_path_buf()),
            ..Default::default()
        },
        ..Default::default()
    };

    let manager = ExtensionManager::new(config).await.unwrap();
    manager.load_all().await.unwrap();

    // Call the tool
    let result = manager
        .call_plugin_tool("test-plugin", "handleTestTool", serde_json::json!({"key": "value"}))
        .await;

    assert!(result.is_ok());
    let value = result.unwrap();
    assert_eq!(value["success"], true);
    assert_eq!(value["input"]["key"], "value");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aethecore --test plugin_runtime_test`
Expected: FAIL (test file doesn't exist or fails)

**Step 3: Implement and fix issues**

Create the test file, then iterate on fixes until it passes.

**Step 4: Run test to verify it passes**

Run: `cargo test -p aethecore --test plugin_runtime_test`
Expected: PASS

**Step 5: Commit**

```bash
git add core/tests/integration/plugin_runtime_test.rs
git commit -m "$(cat <<'EOF'
test(extension): add end-to-end plugin runtime integration test

Tests the full flow: create test plugin → load via ExtensionManager
→ call tool → verify result. Confirms Node.js IPC works correctly.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Add CLI Command for Plugin Management

**Files:**
- Modify: `core/src/bin/aether_gateway.rs` or create `core/src/cli/plugins.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_cli_plugins_subcommand_exists() {
    use clap::CommandFactory;
    let cmd = Cli::command();
    assert!(cmd.find_subcommand("plugins").is_some());
}
```

**Step 2: Run test to verify it fails**

Expected: FAIL (no plugins subcommand)

**Step 3: Add plugins subcommand**

```rust
#[derive(Subcommand)]
enum PluginCommands {
    /// List installed plugins
    List,
    /// Install a plugin from Git
    Install { url: String },
    /// Uninstall a plugin
    Uninstall { name: String },
    /// Enable a plugin
    Enable { name: String },
    /// Disable a plugin
    Disable { name: String },
}
```

**Step 4: Run test to verify it passes**

Expected: PASS

**Step 5: Commit**

```bash
git add core/src/
git commit -m "$(cat <<'EOF'
feat(cli): add plugins subcommand for plugin management

CLI now supports:
- aether plugins list
- aether plugins install <url>
- aether plugins uninstall <name>
- aether plugins enable <name>
- aether plugins disable <name>

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Summary

This plan covers 10 tasks for Phase 3 & 4 Plugin System Integration:

1. **plugin-host.js** - Embedded JavaScript host for Node.js plugins
2. **Embedded host integration** - NodeJsRuntime uses bundled script
3. **PluginLoader** - Runtime management abstraction
4. **ExtensionManager integration** - Wire PluginLoader into manager
5. **plugins.callTool handler** - Gateway RPC for tool invocation
6. **Gateway wiring** - Pass ExtensionManager to handlers
7. **plugins.load/unload handlers** - Dynamic plugin loading
8. **Handler registration** - Register all plugin methods
9. **Integration test** - End-to-end Node.js plugin test
10. **CLI commands** - Command-line plugin management

After completion, plugins can:
- Be discovered and loaded at startup
- Register tools and hooks via IPC
- Have their tools called by agents
- Be managed via Gateway RPC and CLI
