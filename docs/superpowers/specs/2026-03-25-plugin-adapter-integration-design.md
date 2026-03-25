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
  → discovery.discover_all()              // existing, returns PluginCandidate list
  → for each candidate:
      adapter_registry.parse_dir(path)    // existing, returns AdapterOutput
      → PluginRecord::from_adapter_output()
      → CapabilityApi::register_capability()  // writes to PluginRegistry
  → extract hooks from PluginRegistry → inject into HookExecutor
  → extract McpServer declarations → inject into PluginLoader (MCP startup)
  → skill_system loads v2 prompt skills (independent path, not via adapter)
```

#### Bridge code

```rust
pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
    let discovered = self.discovery.discover_all()?;
    let mut summary = LoadSummary::default();

    let mut registry = self.plugin_registry.write().await;
    registry.clear();

    for candidate in discovered.active_plugins() {
        match self.adapter_registry.parse_dir(candidate.path()) {
            Ok(output) => {
                let mut record = PluginRecord::from_adapter_output(&output, candidate.path().to_owned());
                record.origin = candidate.origin();
                registry.register_plugin(record);

                let permissions = output.source.permissions.clone();
                let mut api = CapabilityApi::new(&mut registry, &output.plugin_id, &permissions);
                for cap in output.capabilities {
                    if let Err(e) = api.register_capability(cap) {
                        tracing::warn!(plugin = %output.plugin_id, error = %e, "Failed to register capability");
                    }
                }
                summary.plugins_loaded += 1;
            }
            Err(e) => {
                tracing::warn!(path = %candidate.path().display(), error = %e, "Failed to parse plugin");
                summary.add_error(candidate.path(), e);
            }
        }
    }

    // Extract hooks from PluginRegistry → inject into HookExecutor
    self.sync_hooks_from_registry(&registry).await;

    // Extract MCP servers → schedule startup
    self.sync_mcp_from_registry(&registry).await;

    Ok(summary)
}
```

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
fn is_path_inside(root: &Path, target: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let target = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
    target.starts_with(&root)
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

`ExtensionSkill` and `SkillRegistration` are unified into a single type. `SkillRegistration` is extended with fields from `ExtensionSkill`:

```rust
pub struct SkillRegistration {
    // Existing
    pub name: String,
    pub description: String,
    pub content: String,
    pub triggers: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub category: Option<String>,
    pub plugin_id: String,
    // Added from ExtensionSkill
    pub emoji: Option<String>,
    pub source_path: Option<PathBuf>,
    pub disable_model_invocation: bool,
    pub cli_wrapper: bool,
}
```

Same pattern for `AgentRegistration` — add `ExtensionAgent`'s extra fields.

After unification, `ExtensionSkill` and `ExtensionAgent` types in `types/skills.rs` and `types/agents.rs` become deprecated aliases or are deleted outright. All callers migrate to `SkillRegistration`/`AgentRegistration`.

**Affected callers:**
- `skill_ops.rs` — return types
- `sync_api.rs` — return types
- `skill_tool.rs` — skill execution reads
- `template.rs` — template rendering
- `agent_loop/prompt_builder.rs` — prompt construction
- Gateway plugin handlers — serialization

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
