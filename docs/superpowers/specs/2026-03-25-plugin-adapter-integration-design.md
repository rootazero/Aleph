# Plugin Adapter Integration & Format Deepening Design

**Date**: 2026-03-25
**Status**: Draft
**Scope**: Wire AdapterRegistry into main loading path, deepen Codex/Cursor format support, delete legacy code
**Prerequisite**: Capability-driven plugin architecture (completed 2026-03-25)

## Summary

The capability-driven infrastructure (ManifestAdapter trait, CapabilityApi, AdapterRegistry, 5 adapters, parsers, MCP/WASM registrars) is fully built and tested but only used in the `reload_plugin()` hot-reload path. The main startup loading path still uses `legacy_loader` → `parse_manifest_from_dir_sync()`.

This spec:
1. **Wires AdapterRegistry into the main `load_all()` path**, replacing `legacy_loader`
2. **Deepens Codex/Cursor format support** following OpenClaw conventions
3. **Deletes ~800 lines of legacy code** (legacy_loader.rs, old parse functions)
4. **Unifies skill/command/agent storage** in PluginRegistry, removing ExtensionManager's redundant HashMaps

## Design

### 1. Loading Flow Rewrite

#### Current flow (to be replaced)

```
ExtensionManager::load_all()
  → legacy_loader::load_all(&discovery)
    → discovery.discover_skill_dirs() / discover_command_dirs() / ...
    → load into Vec<ExtensionSkill> / Vec<ExtensionCommand> / ...
  → write to ExtensionManager's skills/commands/agents HashMaps
  → hooks → HookExecutor
```

#### New flow

```
ExtensionManager::load_all()
  → collect plugin directories from discovery   // NEW: unified scan
    (discover_skill_dirs + discover_command_dirs + discover_agent_dirs + discover_plugins
     → deduplicate by path → Vec<(PathBuf, PluginOrigin)>)
  → for each plugin_dir:
      adapter_registry.parse_dir(path)           // existing, returns AdapterOutput
      → PluginRecord::from_adapter_output()
      → CapabilityApi::register_capability()     // writes to PluginRegistry
  → sync_hooks_from_registry()                   // NEW: extract hooks → HookExecutor
  → sync_mcp_from_registry()                     // NEW: extract MCP → PluginLoader
  → skill_system loads v2 prompt skills          // independent path, not via adapter
```

#### New methods required

The following methods do NOT currently exist and must be implemented:

1. **`collect_plugin_dirs(&self) -> Vec<(PathBuf, PluginOrigin)>`** — NEW method on ExtensionManager. Calls existing discovery methods (`discover_skill_dirs()`, `discover_command_dirs()`, `discover_agent_dirs()`, `discover_plugins()`), collects all unique root directories, deduplicates by canonical path. Returns `(path, origin)` pairs.

2. **`sync_hooks_from_registry(&self, registry: &PluginRegistry)`** — NEW method. Iterates `registry.list_hooks()`, converts `HookRegistration` to `HookConfig`, injects into `self.hook_executor`.

3. **`sync_mcp_from_registry(&self, registry: &PluginRegistry)`** — NEW method. Iterates registry for McpServer capabilities (via plugin records), schedules MCP server startup via `self.loader`.

4. **`LoadSummary::add_error(&mut self, path: &Path, error: anyhow::Error)`** — NEW method. Pushes formatted error string to `self.errors: Vec<String>`.

#### Permissions handling

`CapabilitySource` currently has no `permissions` field (only `plugin_id`, `origin`, `format`). Permissions must come from the adapter:

**Option chosen**: Add `permissions: Vec<PluginPermission>` field to `AdapterOutput` (not to `CapabilitySource`). Each adapter extracts permissions from the manifest's `[aleph.permissions]` section if present, or returns empty vec (default P0+P1 access). The CC TOML adapter already parses `[aleph]` — it can extract permissions. Other adapters (Codex, Cursor, AutoDiscover) always return empty permissions.

```rust
pub struct AdapterOutput {
    pub plugin_id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub capabilities: Vec<CapabilityDeclaration>,
    pub source: CapabilitySource,
    pub permissions: Vec<PluginPermission>,  // NEW
}
```

#### Bridge code

```rust
pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
    let plugin_dirs = self.collect_plugin_dirs()?;
    let mut summary = LoadSummary::default();

    let mut registry = self.plugin_registry.write().await;
    registry.clear();

    for (dir_path, origin) in &plugin_dirs {
        match self.adapter_registry.parse_dir(dir_path) {
            Ok(output) => {
                let mut record = PluginRecord::from_adapter_output(&output, dir_path.clone());
                record.origin = origin.clone();
                registry.register_plugin(record);

                let plugin_id = output.plugin_id.clone();
                let permissions = output.permissions.clone();
                let mut api = CapabilityApi::new(
                    &mut registry,
                    plugin_id,
                    permissions,
                );
                for cap in output.capabilities {
                    if let Err(e) = api.register_capability(cap) {
                        tracing::warn!(plugin = %output.plugin_id, error = %e, "Capability registration failed");
                    }
                }
                summary.plugins_loaded += 1;
            }
            Err(e) => {
                tracing::warn!(path = %dir_path.display(), error = %e, "Failed to parse plugin");
                summary.errors.push(format!("{}: {}", dir_path.display(), e));
            }
        }
    }

    // Sync hooks and MCP from PluginRegistry to executors
    self.sync_hooks_from_registry(&registry);
    self.sync_mcp_from_registry(&registry);

    Ok(summary)
}
```

Note: `CapabilityApi::new()` takes owned values (`String`, `Vec<PluginPermission>`), not references. The `record.origin` is set from the discovery origin (which knows Config vs Workspace vs Global vs Bundled priority) rather than from `AdapterOutput.source.origin` — discovery is the authoritative source of origin.

#### v2_prompt migration

`legacy_loader` has `load_v2_prompt()` (65 lines) and `load_v2_tool_prompts()` (50 lines) that load `[aleph]` V2 prompt templates and tool instruction files. These are migrated to `parsers.rs`:

```rust
pub fn parse_v2_prompts(base: &Path, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
pub fn parse_v2_tool_prompts(base: &Path, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
```

The CC TOML adapter calls these when `[aleph]` extensions are present.

### 2. Format Deepening

#### 2.1 Commands merged as Skills

Following OpenClaw convention: in Claude and Cursor bundles, `commands` directories are treated identically to `skills` directories. In CC TOML/JSON and Cursor adapters:

```rust
// Commands treated as skills (OpenClaw convention)
if let Some(commands_path) = manifest_commands_dir {
    caps.extend(parsers::parse_skills_dir(dir, commands_path, &plugin_id)?);
}
```

`parse_commands_dir()` is retained for edge cases needing distinct command identity, but platform adapters default to merging commands into skills.

#### 2.2 Platform default directory conventions

When manifest doesn't declare paths, each adapter scans platform-conventional directories:

| Platform | Default directories |
|----------|-------------------|
| **Claude Code** | `skills/`, `commands/` (→skills), `agents/`, `hooks/hooks.json`, `.mcp.json` |
| **Codex** | `skills/`, `hooks/`, `.mcp.json` |
| **Cursor** | `skills/`, `.cursor/commands/` (→skills), `.cursor/agents/`, `.cursor/rules/` (→skills), `.cursor/hooks.json`, `.mcp.json` |

Current adapters partially cover these. This spec completes the coverage.

#### 2.3 MCP path boundary security

Add `is_path_inside(root, target)` to `parsers.rs`:

```rust
/// Fail-closed: returns false if either path cannot be canonicalized.
/// This prevents symlink-based bypass attacks.
fn is_path_inside(root: &Path, target: &Path) -> bool {
    match (root.canonicalize(), target.canonicalize()) {
        (Ok(root), Ok(target)) => target.starts_with(&root),
        _ => false,  // fail-closed: deny if paths can't be resolved
    }
}
```

Applied in:
- `parse_mcp_config_file()`: check `command`, `args`, `cwd` after variable substitution
- `parse_skills_dir()`, `parse_commands_dir()`, `parse_agents_dir()`: check resolved directory paths
- Violations logged as warnings and skipped (not errors — graceful degradation)

#### 2.4 Codex `apps` field

Aleph has no app runtime. Codex `apps` field is parsed for metadata awareness but not executed:
- Record in `AdapterOutput` description: "has apps: N"
- Log at debug level
- No new CapabilityDeclaration variant (YAGNI)

#### 2.5 Codex `instructions` field

Already implemented — Codex adapter converts `instructions` text to `CapabilityDeclaration::Skill`. Tests pass. No changes needed.

#### 2.6 Cursor `rules` loading

Already implemented for `.cursorrules` file and `.cursor/rules/` directory scan. Enhancement needed: if manifest declares `rules` field as a path, scan that path too.

### 3. Legacy Code Cleanup

#### 3.1 Files to delete

| File | Lines | Replacement |
|------|-------|-------------|
| `legacy_loader.rs` | 506 | AdapterRegistry + parsers.rs |
| `manifest/mod.rs` `parse_manifest_from_dir_sync()` | ~250 | AdapterRegistry.parse_dir() |
| `manifest/mod.rs` `auto_discover_manifest()` | ~50 | adapters/auto_discover.rs |

#### 3.2 Functions migrated from legacy_loader to parsers.rs

| Function | Lines | Target |
|----------|-------|--------|
| `load_v2_prompt()` | ~65 | `parsers::parse_v2_prompts()` |
| `load_v2_tool_prompts()` | ~50 | `parsers::parse_v2_tool_prompts()` |

Already-covered functions (no migration needed): `load_skill_internal`, `load_hooks`, `load_mcp_servers`.

#### 3.3 Type unification

`ExtensionSkill` and `SkillRegistration` are unified into a single type. `SkillRegistration` is extended with ALL fields from `ExtensionSkill`:

```rust
pub struct SkillRegistration {
    // Existing fields (from SkillRegistration)
    pub name: String,
    pub description: String,
    pub content: String,
    pub triggers: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub category: Option<String>,
    pub plugin_id: String,
    // From ExtensionSkill — required for skill execution and filtering
    pub skill_type: SkillType,                    // SkillType enum (Skill, Command, Agent)
    pub source_path: Option<PathBuf>,
    pub scope: PromptScope,                       // System, Tool, Standalone, Disabled
    pub bound_tool: Option<String>,               // Tool this skill is bound to
    pub disable_model_invocation: bool,
    pub source: Option<DiscoverySource>,           // Discovery source (enum, not String)
    pub plugin_name: Option<String>,              // Owning plugin name
}
```

Note: `emoji` and `cli_wrapper` do NOT exist on `ExtensionSkill` and are omitted. `source` uses `DiscoverySource` enum (not `String`). `triggers` and `allowed_tools` are SkillRegistration-native fields not from ExtensionSkill.

Methods migrated from `ExtensionSkill` to `SkillRegistration`:
- `is_auto_invocable()` — checks scope and triggers
- `qualified_name()` — returns `plugin_name:name` format
- `to_skill_info()` — serialization for RPC responses
- `base_dir()` — derives base directory from source_path

`AgentRegistration` similarly extended with ALL fields from `ExtensionAgent`:

```rust
pub struct AgentRegistration {
    // Existing fields
    pub name: String,
    pub description: Option<String>,              // Option<String> to match ExtensionAgent
    pub content: String,
    pub model: Option<String>,
    pub plugin_id: String,
    // From ExtensionAgent — required for agent routing and execution
    pub mode: AgentMode,                          // Primary, Sub, Hidden
    pub hidden: bool,
    pub color: Option<String>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub steps: Option<u32>,
    pub tools: Option<HashMap<String, bool>>,     // Per-tool allow/deny map
    pub permission: Option<HashMap<String, PermissionRule>>,  // Per-permission rules
    pub options: HashMap<String, serde_json::Value>,          // Arbitrary options (Value not String)
    pub system_prompt: Option<String>,            // Separate from content
    pub source_path: Option<PathBuf>,
    pub source: Option<DiscoverySource>,
    pub plugin_name: Option<String>,
}
```

Types matched exactly to `ExtensionAgent` in `types/agents.rs`: `tools` is `Option<HashMap<String, bool>>`, `permission` is `Option<HashMap<String, PermissionRule>>`, `options` is `HashMap<String, Value>`, `description` is `Option<String>`.

Methods migrated from `ExtensionAgent`:
- `is_primary()` / `is_subagent()` — mode-based filtering
- `qualified_name()` — returns `plugin_name:name`
- `to_agent_info()` — serialization for RPC responses

After unification, `ExtensionSkill` in `types/skills.rs` and `ExtensionAgent` in `types/agents.rs` become `pub type` aliases pointing to the Registry types, then deleted in a subsequent cleanup pass.

**Affected callers (complete list):**
- `skill_ops.rs` — return types, `get_all_skills()`, `get_skill()`, `install_skill()`, `delete_skill()`
- `sync_api.rs` — return types for sync wrappers
- `skill_tool.rs` — `invoke_skill()` takes `&SkillRegistration`, accesses `source_path`, `base_dir()`, `qualified_name()`, `disable_model_invocation`
- `template.rs` — template rendering, `source_path` access
- `agent_loop/prompt_builder.rs` — prompt construction, reads `content`, `system_prompt`
- Gateway plugin handlers — serialization to `PluginInfoJson`, `SkillInfoJson`
- `mod.rs` — `ExtensionManager::reload()` method, clear + reload logic (must update alongside field removal)

#### 3.4 ExtensionManager field removal

Remove:
```rust
skills: Arc<RwLock<HashMap<String, ExtensionSkill>>>,
commands: Arc<RwLock<HashMap<String, ExtensionCommand>>>,
agents: Arc<RwLock<HashMap<String, ExtensionAgent>>>,
```

Query methods rewritten to delegate to PluginRegistry:
```rust
pub async fn get_all_skills(&self) -> Vec<SkillRegistration> {
    let reg = self.plugin_registry.read().await;
    reg.list_skills().into_iter().cloned().collect()
}
```

`ExtensionManager::reload()` method (which clears `self.skills`, `self.commands`, `self.agents` before calling `load_all()`) must also be updated — after field removal, `reload()` only needs to call `load_all()` which handles `registry.clear()` internally.

#### 3.5 manifest/mod.rs slimming

Target: ~100 lines. Retain:
- Module declarations (`pub mod adapter`, `pub mod adapters`, `pub mod parsers`, `pub mod types`, etc.)
- `parse_frontmatter()` utility function
- `sanitize_plugin_id()` / `validate_plugin_id()`

Delete:
- `parse_manifest_from_dir_sync()` and all fallback chain logic
- `auto_discover_manifest()`
- Legacy `PluginManifest` construction code

`PluginManifest` type in `types.rs` retained if used by validation.rs or other modules, but no longer constructed during the main loading flow.

## Migration Safety

1. **External API preserved**: `plugins.list`, `plugin.reload`, `plugin.install` RPC signatures unchanged
2. **PluginRegistry query API already exists**: `list_skills()`, `get_skill()`, `list_agents()`, etc. (added in previous spec)
3. **Adapter tests**: 46 adapter tests verify format parsing correctness
4. **Full test suite**: 8125 tests as regression safety net
5. **Graceful degradation**: adapter parse failures log warnings, don't crash startup

## Out of Scope

- **settings.json handling**: Deferred to separate spec (Spec 2: Plugin Settings Unification)
- **Plugin SDK crate**: Future work
- **LSP server integration**: Detect-only (existing behavior)
- **Output styles**: Detect-only (existing behavior)
