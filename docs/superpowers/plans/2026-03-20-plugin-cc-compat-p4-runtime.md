# Plugin CC Compat: P4 Runtime Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** For plugins with `runtime = "mcp"`, launch their `.mcp.json` servers via the existing MCP client system instead of custom Node.js IPC. Retain WASM runtime. Remove custom Node.js IPC code.

**Architecture:** PluginLoader gains an MCP integration path: when loading an MCP-type plugin, it reads `.mcp.json` from the plugin directory and registers the servers with `McpManagerHandle`. Tool calls for MCP plugins route through `mcp.callTool` instead of custom IPC. The Node.js IPC runtime (`NodeJsRuntime`, `NodeProcess`, `plugin-host.js`) is deprecated and eventually removed.

**Key insight:** Aleph already has a full MCP client at `src/mcp/` with transport abstraction, tool discovery, and lifecycle management. We reuse it entirely.

**Tech Stack:** Existing MCP system, existing PluginLoader, serde_json

---

## File Structure

### Modified Files
| File | Changes |
|------|---------|
| `src/extension/plugin_loader.rs` | Add MCP loading path for `runtime=mcp` plugins |
| `src/extension/plugin_ops.rs` | Route MCP plugin tool calls through MCP client |
| `src/extension/mod.rs` | Provide MCP handle to PluginLoader |

### Eventually Deprecated (P5)
| File | Status |
|------|--------|
| `src/extension/runtime/nodejs/mod.rs` | Deprecated (keep for transition) |
| `src/extension/runtime/nodejs/process.rs` | Deprecated |
| `src/extension/runtime/nodejs/ipc.rs` | Deprecated |
| `src/extension/runtime/nodejs/plugin-host.js` | Deprecated |

---

## Task 1: Read .mcp.json from plugin directory

**Files:**
- Create or add to: `src/extension/manifest/` or `src/extension/marketplace/`

- [ ] **Step 1: Add .mcp.json parser**

Create a simple function (can live in plugin_loader.rs or a new file):

```rust
/// Read .mcp.json from a plugin directory and return MCP server configs
pub fn read_plugin_mcp_config(plugin_dir: &Path) -> Result<HashMap<String, McpServerConfig>, String> {
    let mcp_path = plugin_dir.join(".mcp.json");
    if !mcp_path.exists() {
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(&mcp_path)
        .map_err(|e| format!("Failed to read .mcp.json: {}", e))?;

    // Standard MCP config format: { "mcpServers": { "name": { "command": "...", "args": [...] } } }
    let wrapper: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Invalid .mcp.json: {}", e))?;

    // Extract mcpServers field
    let servers = wrapper.get("mcpServers")
        .ok_or("Missing 'mcpServers' field in .mcp.json")?;

    // Parse into the existing McpServerConfig type (check what the MCP system uses)
    // ...
}
```

The key is to match the existing MCP server config format used by `McpManagerHandle::start_external_servers()`. Read the MCP config types to understand the exact structure.

Also handle `${CLAUDE_PLUGIN_ROOT}` and `${ALEPH_PLUGIN_ROOT}` variable substitution in command/args/env paths — replace with the actual plugin directory path.

- [ ] **Step 2: Test with a mock .mcp.json**

- [ ] **Step 3: Commit**

---

## Task 2: PluginLoader MCP integration

**Files:**
- Modify: `src/extension/plugin_loader.rs`

- [ ] **Step 1: Add MCP loading path**

When `load_plugin()` is called with a plugin whose `aleph_extensions.runtime == AlephRuntime::Mcp` (or has no aleph_extensions and has `.mcp.json`):

1. Read `.mcp.json` from plugin directory
2. Substitute `${CLAUDE_PLUGIN_ROOT}` / `${ALEPH_PLUGIN_ROOT}` with actual plugin path
3. Register each MCP server with the MCP system
4. Track which MCP servers belong to this plugin (for unload)

The PluginLoader needs access to the MCP system. Two approaches:
- **A)** Pass `McpManagerHandle` to PluginLoader at construction time
- **B)** Use the global MCP manager (if accessible via a static)

Choose whichever matches existing patterns in the codebase.

```rust
impl PluginLoader {
    /// Load a plugin via MCP (launch .mcp.json servers)
    async fn load_mcp_plugin(&mut self, manifest: &PluginManifest) -> Result<(), String> {
        let mcp_configs = read_plugin_mcp_config(&manifest.root_dir)?;
        if mcp_configs.is_empty() {
            return Ok(()); // Static plugin, no servers to launch
        }

        let plugin_root = manifest.root_dir.to_string_lossy().to_string();

        for (server_name, mut config) in mcp_configs {
            // Substitute plugin root in command/args
            substitute_plugin_vars(&mut config, &plugin_root);

            // Prefix server name with plugin ID to avoid conflicts
            let scoped_name = format!("plugin_{}_{}", manifest.id, server_name);

            // Register with MCP system
            // Use McpManagerHandle or similar to start the server
            // ...
        }

        self.loaded_plugins.insert(manifest.id.clone(), PluginKind::NodeJs);
        Ok(())
    }
}
```

- [ ] **Step 2: Update `load_plugin()` dispatch**

In the main `load_plugin()` method, check the runtime type:
- `AlephRuntime::Mcp` or `PluginKind::NodeJs` with `.mcp.json` → `load_mcp_plugin()`
- `AlephRuntime::Wasm` → existing WASM path
- `AlephRuntime::Static` → no runtime loading needed

- [ ] **Step 3: Handle unload**

When unloading an MCP plugin, stop its MCP servers.

- [ ] **Step 4: Compile check, commit**

---

## Task 3: Route plugin tool calls through MCP

**Files:**
- Modify: `src/extension/plugin_ops.rs`
- Possibly modify: `src/gateway/handlers/plugins/handlers.rs`

- [ ] **Step 1: Update call_plugin_tool**

When `call_plugin_tool()` is called for an MCP-type plugin, the tool should be invoked via the MCP client (not the old NodeJsRuntime IPC).

Since MCP tools are already registered in the unified tool system (via McpToolWrapper), the call might already work through the normal tool invocation path. Check if:
1. MCP servers started in Task 2 automatically register their tools
2. Those tools are callable via the normal agent loop

If yes, `call_plugin_tool()` for MCP plugins can simply delegate to the MCP tool call path.

If not, add explicit routing:
```rust
pub async fn call_plugin_tool(...) {
    let loader = self.plugin_loader.read().await;
    match loader.get_plugin_kind(plugin_id) {
        Some(PluginKind::NodeJs) if is_mcp_plugin(plugin_id) => {
            // Route through MCP client
            // mcp_handle.call_tool(server_name, tool_name, args)
        }
        Some(PluginKind::NodeJs) => {
            // Legacy IPC path (deprecated, still works for old plugins)
        }
        Some(PluginKind::Wasm) => {
            // WASM path (unchanged)
        }
        _ => Err(...)
    }
}
```

- [ ] **Step 2: Deprecation marker for Node.js IPC**

Add `#[deprecated(note = "Use MCP runtime instead")]` to NodeJsRuntime methods.

- [ ] **Step 3: Compile check, commit**

---

## Task 4: Environment variable substitution

**Files:**
- Add to plugin_loader.rs or a utility module

- [ ] **Step 1: Implement variable substitution**

```rust
fn substitute_plugin_vars(value: &mut serde_json::Value, plugin_root: &str) {
    match value {
        serde_json::Value::String(s) => {
            *s = s.replace("${CLAUDE_PLUGIN_ROOT}", plugin_root)
                  .replace("${ALEPH_PLUGIN_ROOT}", plugin_root);
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                substitute_plugin_vars(item, plugin_root);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                substitute_plugin_vars(v, plugin_root);
            }
        }
        _ => {}
    }
}
```

Apply to `.mcp.json` config values (command, args, env) before passing to MCP manager.

- [ ] **Step 2: Test substitution**

- [ ] **Step 3: Commit**

---

## Task 5: Final verification

- [ ] **Step 1: Full compile and test**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib
cargo clippy -p alephcore -- -W clippy::all
```

- [ ] **Step 2: Commit any fixes**

---

## Notes

### What this plan does NOT do (deferred to P5/P4b):
- **Remove Node.js IPC code** — just deprecated, not removed (P5)
- **Migrate Aleph-plugins repo** — the 7 plugins' directory structure and code changes (P4b, separate repo)
- **Implement `${CLAUDE_PLUGIN_DATA}` / `${ALEPH_PLUGIN_DATA}`** — data directory creation (minor, can add later)

### Complexity assessment:
Task 2 (PluginLoader MCP integration) is the most complex — it requires understanding the MCP manager's API for starting servers. The implementer should read `src/mcp/manager/handle.rs` and `src/mcp/external/` to understand how external MCP servers are started.
