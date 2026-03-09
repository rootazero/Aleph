# Unified Plugin System Design

## Goal

Eliminate the dual plugin system (ComponentRegistry + PluginRegistry) by unifying into a single PluginRegistry-based architecture. Skills/Agents remain independent. A thin LegacyAdapter preserves Claude plugin format compatibility at near-zero cost.

## Core Decisions

1. **Delete ComponentRegistry** — all plugin state lives in PluginRegistry
2. **Skills/Agents independent** — managed by SkillSystem, not part of plugin system
3. **Unified HookEvent** — single enum replacing both HookEvent and PluginHookEvent
4. **Unified discovery** — single 4-layer priority system, delete `discovery/scanner.rs`
5. **LegacyAdapter** — thin `.claude-plugin/plugin.json` → `PluginManifest` converter (~100 lines)
6. **Net reduction** — ~500-800 lines of code removed

## Architecture

### Before (Current)

```
ExtensionManager
  ├── registry: ComponentRegistry        // Skills, Commands, Agents, Plugins
  ├── plugin_registry: PluginRegistry    // Tools, Hooks, Services, etc. (10 types)
  ├── loader: ComponentLoader            // Markdown loading
  └── plugin_loader: PluginLoader        // Node.js/WASM runtime
```

Two parallel systems with hack-merged `get_plugin_info()`, unsynchronized enable/disable, and two hook enums.

### After (Unified)

```
ExtensionManager
  ├── plugin_registry: PluginRegistry    // Sole plugin registry (10 types)
  ├── plugin_loader: PluginLoader        // Sole plugin loader (Node.js/WASM)
  ├── hook_executor: HookExecutor        // Unified HookEvent enum
  ├── service_manager: ServiceManager
  ├── skill_system: SkillSystem          // Independent Skills + Agents
  ├── discovery: DiscoveryManager        // Unified 4-layer discovery
  ├── config_manager: ConfigManager
  └── cache_state: CacheState
```

## Component Design

### 1. PluginRegistry (Unchanged)

Keeps existing 10 registration types:

| Priority | Types |
|----------|-------|
| P0 Core | Tools, Hooks |
| P1 Important | Channels, Providers, GatewayMethods |
| P2 Useful | HttpRoutes, HttpHandlers, CLI, Services |
| P3 Optional | Commands |

### 2. SkillSystem (Elevated to Required)

```rust
pub struct SkillSystem {
    content_loader: ContentLoader,  // Renamed from ComponentLoader
    skills: HashMap<String, ExtensionSkill>,
    agents: HashMap<String, ExtensionAgent>,
}

impl SkillSystem {
    pub async fn load_all(&self) -> Result<()>;
    pub fn get_skill(&self, name: &str) -> Option<&ExtensionSkill>;
    pub fn get_agent(&self, name: &str) -> Option<&ExtensionAgent>;
    pub fn list_skills(&self) -> Vec<&ExtensionSkill>;
    pub fn list_agents(&self) -> Vec<&ExtensionAgent>;
}
```

Scans `~/.aleph/skills/` and `~/.aleph/workspaces/`. No runtime needed.

### 3. Unified HookEvent

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    // Agent lifecycle
    BeforeAgentStart,
    AgentEnd,
    // Tool calls
    BeforeToolCall,
    AfterToolCall,
    ToolResultPersist,
    // Message flow
    MessageReceived,
    MessageSending,
    MessageSent,
    // Session lifecycle
    SessionStart,
    SessionEnd,
    // Context compaction
    BeforeCompaction,
    AfterCompaction,
    // Gateway lifecycle
    GatewayStart,
    GatewayStop,
    // From legacy (valuable)
    Notification,
    PermissionRequest,
}
```

Deleted legacy events: `Setup`, `PostToolUseFailure`, `CommandExecuteBefore/After`, `ChatMessage/ChatParams/ChatResponse`, `SubagentStart/Stop` — all covered by unified events.

### 4. Unified Discovery

4-layer priority system (kept from `extension/discovery/`):

```
1. Config-specified paths     (highest priority)
2. ~/.aleph/projects/<id>/extensions/
3. ~/.aleph/plugins/          (GitHub bootstrap)
   ~/.aleph/extensions/       (manual install)
   ~/.claude/extensions/      (legacy compatibility)
4. Bundled plugins            (lowest priority)
```

Each directory scanned:
1. Try `aleph.plugin.toml` → parse as PluginManifest
2. Try `.claude-plugin/plugin.json` → LegacyAdapter → PluginManifest
3. Support monorepo (one-level subdirectory scan)

### 5. LegacyAdapter

```rust
// extension/manifest/legacy_adapter.rs (~100 lines)
pub fn adapt_legacy_manifest(
    legacy: &LegacyPluginManifest,
    plugin_dir: &Path,
) -> Result<PluginManifest, ManifestError> {
    // Map legacy fields to V2 PluginManifest
    // Detect plugin kind from directory contents
    // Convert hook configs to unified HookEvent
}
```

Mapping rules:
- `name` → `id` + `name`
- `commands/` directory → `commands_v2` sections
- `hooks/hooks.json` → `hooks_v2` with event mapping
- `.mcp.json` → not mapped (MCP independent)
- `skills/` directory → not mapped (Skills independent)
- `agents/` directory → not mapped (Agents independent)

### 6. ExtensionManager Key Methods

```rust
impl ExtensionManager {
    pub async fn load_all(&self) -> Result<()> {
        let manifests = self.discovery.discover_all().await?;
        for manifest in manifests {
            self.load_runtime_plugin(manifest).await?;
        }
        self.skill_system.load_all().await?;
        self.service_manager.write().await.start_all().await?;
        Ok(())
    }

    pub async fn get_plugin_info(&self) -> Vec<PluginInfo> {
        // Single source — no hack merge
        self.plugin_registry.read().await
            .list_plugins()
            .into_iter()
            .map(|r| r.into())
            .collect()
    }
}
```

### 7. Gateway Handler Simplification

| RPC Method | Change |
|------------|--------|
| `plugins.list` | Direct `plugin_registry.list_plugins()`, no merge |
| `plugins.install` | Parse manifest (V2 or adapter) → `PluginLoader` |
| `plugins.enable/disable` | Filesystem + sync `plugin_registry` |
| `plugins.load/callTool/executeCommand` | Unchanged |

## File Changes

### Delete

| File | Reason |
|------|--------|
| `extension/registry/component_registry.rs` | Replaced by PluginRegistry |
| `discovery/scanner.rs` (DirectoryScanner) | Merged into extension/discovery |
| `discovery/types.rs` (DiscoveredPlugin) | No longer needed |

### Rename

| From | To | Reason |
|------|-----|--------|
| `extension/loader.rs` | `extension/content_loader.rs` | Only Skills/Agents loading remains |
| Legacy manifest in `extension/manifest/mod.rs` | `extension/manifest/legacy.rs` | Clear separation |

### Create

| File | Purpose |
|------|---------|
| `extension/manifest/legacy_adapter.rs` | LegacyManifest → PluginManifest (~100 lines) |

### Modify

| File | Change |
|------|--------|
| `extension/mod.rs` | Remove `registry` field, `skill_system` non-Option, rewrite `load_all()` / `get_plugin_info()` |
| `extension/registry/mod.rs` | Remove ComponentRegistry export |
| `extension/discovery/scanner.rs` | Integrate monorepo + legacy discovery |
| `extension/discovery/mod.rs` | Add `~/.aleph/plugins/` scan path |
| `gateway/handlers/plugins/handlers.rs` | Remove dual-registry hack, simplify all handlers |
| `discovery/paths.rs` | Move Claude constants to LegacyAdapter |
| `discovery/mod.rs` | Remove scanner exports |
| `extension/types/plugins.rs` | Remove ExtensionPlugin, use PluginRecord |
| `extension/types/hooks.rs` | Unified HookEvent enum, remove HookConfig/HookAction |
| `extension/hook_executor.rs` | Adapt to unified HookEvent |

### Unchanged

| File | Reason |
|------|--------|
| `extension/plugin_loader.rs` | Already the unified loader |
| `extension/registry/plugin_registry/` | Already the target registry |
| `extension/runtime/` (nodejs/, wasm/) | Runtimes unchanged |
| `extension/manifest/aleph_plugin_toml.rs` | V2 format parser unchanged |
| `discovery/bootstrap.rs` | GitHub install unchanged |
| `apps/cli/src/commands/plugin_cmd.rs` | Toolchain unchanged |
