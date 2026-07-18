# Plugin Adapter Integration & Format Deepening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire AdapterRegistry into the main loading path, replacing legacy_loader, unify skill/agent types, deepen Codex/Cursor format support, and delete ~800 lines of legacy code.

**Architecture:** `load_all()` is rewritten to: collect plugin dirs from discovery → AdapterRegistry.parse_dir() → CapabilityApi writes to PluginRegistry. ExtensionManager's skill/command/agent HashMaps are removed; all queries go through PluginRegistry. Legacy_loader.rs is deleted after migrating v2_prompt to parsers.rs.

**Tech Stack:** Rust, serde, anyhow, tracing. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-03-25-plugin-adapter-integration-design.md`

---

## File Map

### Files to Modify
| File | Change |
|------|--------|
| `src/extension/registry/types.rs` | Extend SkillRegistration + AgentRegistration with all ExtensionSkill/Agent fields |
| `src/extension/registry/plugin_registry/mod.rs` | Update Default derives, add skill/agent query helpers |
| `src/extension/manifest/adapter.rs` | Add `permissions` field to AdapterOutput |
| `src/extension/manifest/adapters/codex.rs` | Commands→skills, default dirs, apps metadata |
| `src/extension/manifest/adapters/cursor.rs` | Commands→skills, default dirs, manifest rules field |
| `src/extension/manifest/cc_plugin_toml.rs` | Commands→skills merge, permissions extraction |
| `src/extension/manifest/cc_plugin_json.rs` | Commands→skills merge |
| `src/extension/manifest/parsers.rs` | Add is_path_inside(), v2_prompt functions, path security |
| `src/extension/mod.rs` | Rewrite load_all(), remove HashMap fields, add collect_plugin_dirs/sync methods |
| `src/extension/skill_ops.rs` | Rewrite all queries to use PluginRegistry |
| `src/extension/sync_api.rs` | Update types from ExtensionSkill → SkillRegistration |
| `src/extension/skill_tool.rs` | Update to use SkillRegistration |
| `src/extension/template.rs` | Update to use SkillRegistration |
| `src/extension/types/skills.rs` | Make ExtensionSkill/ExtensionCommand type aliases |
| `src/extension/types/agents.rs` | Make ExtensionAgent type alias |
| `src/extension/manifest/mod.rs` | Delete parse_manifest_from_dir_sync, auto_discover_manifest |

### Files to Delete
| File | Lines | Reason |
|------|-------|--------|
| `src/extension/legacy_loader.rs` | 506 | Replaced by AdapterRegistry + parsers.rs |

---

## Task 1: Type Unification — Extend SkillRegistration & AgentRegistration

**Files:**
- Modify: `src/extension/registry/types.rs`
- Modify: `src/extension/types/skills.rs`
- Modify: `src/extension/types/agents.rs`

This task extends the Registration types to absorb all fields from ExtensionSkill/ExtensionAgent, then makes the old types into aliases.

- [ ] **Step 1: Read ExtensionSkill and ExtensionAgent to confirm fields**

Read `src/extension/types/skills.rs` (ExtensionSkill at lines 114-146) and `src/extension/types/agents.rs` (ExtensionAgent at lines 49-109). Note all fields and their exact types.

- [ ] **Step 2: Extend SkillRegistration**

In `src/extension/registry/types.rs`, add these fields to `SkillRegistration`:

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
    // NEW from ExtensionSkill
    pub skill_type: crate::extension::types::SkillType,
    pub source_path: Option<std::path::PathBuf>,
    pub scope: crate::extension::types::PromptScope,
    pub bound_tool: Option<String>,
    pub disable_model_invocation: bool,
    pub source: Option<crate::extension::types::DiscoverySource>,
    pub plugin_name: Option<String>,
}
```

Add `Default` impl or derive if needed. New fields should have sensible defaults: `skill_type: SkillType::Skill`, `scope: PromptScope::Standalone`, `disable_model_invocation: false`, `source_path: None`, etc.

- [ ] **Step 3: Add methods to SkillRegistration**

Migrate these methods from ExtensionSkill to SkillRegistration:
- `qualified_name() -> String` — returns `plugin_name:name` or `name`
- `is_auto_invocable() -> bool` — `!disable_model_invocation && skill_type == SkillType::Skill`
- `with_arguments(args: &str) -> Self` — replaces `$ARGUMENTS` in content
- `to_skill_info() -> crate::skills::SkillInfo`
- `base_dir() -> PathBuf` — parent of source_path

- [ ] **Step 4: Extend AgentRegistration**

Add all ExtensionAgent fields:

```rust
pub struct AgentRegistration {
    // Existing
    pub name: String,
    pub description: Option<String>,  // changed from String to Option<String>
    pub content: String,
    pub model: Option<String>,
    pub plugin_id: String,
    // NEW from ExtensionAgent
    pub mode: crate::extension::types::AgentMode,
    pub hidden: bool,
    pub color: Option<String>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub steps: Option<u32>,
    pub tools: Option<std::collections::HashMap<String, bool>>,
    pub permission: Option<std::collections::HashMap<String, crate::extension::types::PermissionRule>>,
    pub options: std::collections::HashMap<String, serde_json::Value>,
    pub system_prompt: Option<String>,
    pub source_path: Option<std::path::PathBuf>,
    pub source: Option<crate::extension::types::DiscoverySource>,
    pub plugin_name: Option<String>,
}
```

Add methods: `qualified_name()`, `is_primary()`, `is_subagent()`.

- [ ] **Step 5: Make ExtensionSkill/ExtensionAgent type aliases**

In `src/extension/types/skills.rs`:
```rust
pub type ExtensionSkill = crate::extension::registry::types::SkillRegistration;
pub type ExtensionCommand = ExtensionSkill;
```

In `src/extension/types/agents.rs`:
```rust
pub type ExtensionAgent = crate::extension::registry::types::AgentRegistration;
```

Move the `SkillType`, `PromptScope`, `DiscoverySource`, `SkillFrontmatter` etc. types to stay in `types/skills.rs` (they're referenced by SkillRegistration). Move `AgentMode`, `PermissionRule`, `PermissionAction` to stay in `types/agents.rs`.

IMPORTANT: This will cause circular dependency if SkillRegistration (in registry/types.rs) imports from types/skills.rs which re-exports from registry/types.rs. Solution: keep the enums (`SkillType`, `PromptScope`, `DiscoverySource`, `AgentMode`, `PermissionRule`) in `types/skills.rs` and `types/agents.rs` — SkillRegistration imports them from there. The type alias goes the other direction. No cycle.

- [ ] **Step 6: Fix compilation**

Run `cargo check -p alephcore`. Fix all type errors — many callers construct `ExtensionSkill { ... }` directly with struct literals. Since `ExtensionSkill` is now an alias for `SkillRegistration`, struct literals need the new fields (with defaults). Add a constructor or `Default` impl to simplify.

- [ ] **Step 7: Run tests**

`cargo test -p alephcore --lib`

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "extension: unify SkillRegistration/AgentRegistration with ExtensionSkill/ExtensionAgent fields"
```

---

## Task 2: AdapterOutput Permissions & Format Deepening

**Files:**
- Modify: `src/extension/manifest/adapter.rs`
- Modify: `src/extension/manifest/adapters/codex.rs`
- Modify: `src/extension/manifest/adapters/cursor.rs`
- Modify: `src/extension/manifest/cc_plugin_toml.rs`
- Modify: `src/extension/manifest/cc_plugin_json.rs`
- Modify: `src/extension/manifest/parsers.rs`

- [ ] **Step 1: Add permissions to AdapterOutput**

In `src/extension/manifest/adapter.rs`, add to `AdapterOutput`:
```rust
pub permissions: Vec<crate::extension::manifest::types::PluginPermission>,
```

Update all adapter `parse()` methods to include `permissions: vec![]` in their `AdapterOutput` construction. For CC TOML adapter, extract from `[aleph.permissions]` if present.

- [ ] **Step 2: Add is_path_inside() to parsers.rs**

Add at the top of `src/extension/manifest/parsers.rs`:
```rust
/// Fail-closed path containment check. Returns false if either path cannot be canonicalized.
pub(crate) fn is_path_inside(root: &Path, target: &Path) -> bool {
    match (root.canonicalize(), target.canonicalize()) {
        (Ok(root), Ok(target)) => target.starts_with(&root),
        _ => false,
    }
}
```

Apply in `parse_skills_dir()`, `parse_commands_dir()`, `parse_agents_dir()`: before scanning a directory, check `is_path_inside(base, resolved_dir)`. Log warning and return empty vec if outside. Apply in `parse_mcp_config_file()`: check command/args paths after variable substitution.

- [ ] **Step 3: Commands→Skills in CC adapters**

In CC TOML adapter's `parse()`: when processing commands directory, call `parsers::parse_skills_dir()` instead of `parsers::parse_commands_dir()`. Same for CC JSON adapter.

- [ ] **Step 4: Complete Codex adapter default directories**

Ensure Codex adapter scans these defaults when manifest doesn't declare them:
- `skills/` if exists
- `hooks/` if exists
- `.mcp.json` if exists

Also handle `apps` field: if present, append to description metadata. Log at debug level.

- [ ] **Step 5: Complete Cursor adapter default directories and rules**

Ensure Cursor adapter scans:
- `skills/` if exists
- `.cursor/commands/` → load as skills
- `.cursor/agents/` → load as agents
- `.cursor/rules/` → load as skills
- `.cursor/hooks.json` → load as hooks
- `.mcp.json` if exists

Enhancement: if manifest has `rules` field with a path, scan that path as skills too.

- [ ] **Step 6: Compile and test**

```bash
cargo check -p alephcore && cargo test -p alephcore --lib adapters
```

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "extension: add permissions to AdapterOutput, deepen Codex/Cursor format support"
```

---

## Task 3: v2_prompt Migration

**Files:**
- Modify: `src/extension/manifest/parsers.rs`
- Read: `src/extension/legacy_loader.rs` (extract v2_prompt logic)

- [ ] **Step 1: Read legacy_loader v2_prompt functions**

Read `src/extension/legacy_loader.rs` and find `load_v2_prompt()` and `load_v2_tool_prompts()`. Understand what they do and what types they return.

- [ ] **Step 2: Implement parse_v2_prompts() in parsers.rs**

Add to `src/extension/manifest/parsers.rs`:
```rust
/// Parse V2 system/tool prompt from plugin directory.
/// Looks for prompt template files specified in [aleph] manifest section.
pub fn parse_v2_prompts(base: &Path, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
```

Mirror the logic from `legacy_loader::load_v2_prompt()` but output `CapabilityDeclaration::Skill` with `scope: PromptScope::System`.

- [ ] **Step 3: Implement parse_v2_tool_prompts() in parsers.rs**

```rust
pub fn parse_v2_tool_prompts(base: &Path, plugin_id: &str) -> Result<Vec<CapabilityDeclaration>>
```

Mirror `legacy_loader::load_v2_tool_prompts()` but output `CapabilityDeclaration::Skill` with `scope: PromptScope::Tool` and `bound_tool` set.

- [ ] **Step 4: Wire into CC TOML adapter**

In the CC TOML adapter's `parse()` method, after parsing standard components, call these v2 functions if the manifest has `[aleph]` extensions.

- [ ] **Step 5: Compile and test**

```bash
cargo check -p alephcore && cargo test -p alephcore --lib parsers
```

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "extension: migrate v2_prompt functions from legacy_loader to parsers.rs"
```

---

## Task 4: Loading Flow Rewrite

**Files:**
- Modify: `src/extension/mod.rs`
- Modify: `src/extension/types/plugins.rs` (LoadSummary)

This is the core integration task. Rewrite `load_all()` to use AdapterRegistry.

- [ ] **Step 1: Add LoadSummary helpers**

In `src/extension/types/plugins.rs`, add to `LoadSummary`:
```rust
pub fn add_error(&mut self, path: &std::path::Path, error: anyhow::Error) {
    self.errors.push(format!("{}: {}", path.display(), error));
}
```

- [ ] **Step 2: Add collect_plugin_dirs() to ExtensionManager**

In `src/extension/mod.rs`, add:
```rust
/// Collect all unique plugin directories from discovery, deduplicated by canonical path.
fn collect_plugin_dirs(&self) -> ExtensionResult<Vec<(PathBuf, PluginOrigin)>> {
    let mut seen = std::collections::HashSet::new();
    let mut dirs = Vec::new();

    // Collect from all discovery sources
    for (path, origin) in self.discovery.discover_plugins()? {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(canonical) {
            dirs.push((path, origin));
        }
    }
    // Also include standalone skill/command/agent directories
    // that aren't part of a plugin (bare directories)
    for path in self.discovery.discover_skill_dirs()? {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(canonical) {
            dirs.push((path, PluginOrigin::Global));
        }
    }
    // ... similar for command_dirs, agent_dirs

    Ok(dirs)
}
```

IMPORTANT: Read the actual `DiscoveryManager` methods first. The function signatures may differ from what's shown here — discover_plugins() might return `Vec<PluginCandidate>` not `Vec<(PathBuf, PluginOrigin)>`. Adapt accordingly.

- [ ] **Step 3: Add sync_hooks_from_registry()**

```rust
/// Extract HookRegistrations from PluginRegistry and inject into HookExecutor.
fn sync_hooks_from_registry(&self, registry: &PluginRegistry) {
    let hooks = registry.list_hooks();
    let mut executor = self.hook_executor.write().unwrap_or_else(|e| e.into_inner());
    executor.clear();
    for hook_reg in hooks {
        // Convert HookRegistration → HookConfig and add to executor
        // Read HookConfig and HookRegistration to understand the mapping
    }
}
```

- [ ] **Step 4: Add sync_mcp_from_registry()**

```rust
/// Extract MCP server configs from PluginRegistry and pass to PluginLoader.
fn sync_mcp_from_registry(&self, registry: &PluginRegistry) {
    // MCP servers are stored as CapabilityDeclaration::McpServer during parsing
    // but McpServer variant is a no-op in CapabilityApi (not stored in registry)
    // So MCP configs must be extracted from AdapterOutput BEFORE capabilities registration
    // OR stored separately during the load_all() loop
}
```

NOTE: This requires careful design. The current CapabilityApi.dispatch() treats McpServer as no-op. MCP configs need to be collected during the load_all loop and passed to PluginLoader separately. May need to accumulate mcp_configs in load_all() alongside the registry loop.

- [ ] **Step 5: Rewrite load_all()**

Replace the current `load_all()` body (which calls `legacy_loader::load_all()`) with the new flow per spec. Key changes:
- Call `collect_plugin_dirs()` instead of legacy_loader
- Iterate with `adapter_registry.parse_dir()`
- Use `CapabilityApi` to register capabilities
- Collect MCP configs separately
- Call sync_hooks_from_registry
- Still call `self.skill_system.init()` for v2 skills (unchanged)

- [ ] **Step 6: Update reload()**

Simplify `reload()` — after HashMap removal, it just needs to call `load_all()` which handles `registry.clear()` internally.

- [ ] **Step 7: Compile check**

```bash
cargo check -p alephcore
```

This WILL fail because skill_ops.rs and sync_api.rs still read from the old HashMaps. That's fixed in Task 5.

- [ ] **Step 8: Commit (may not compile yet)**

```bash
git add -A && git commit -m "extension: rewrite load_all() to use AdapterRegistry, add collect_plugin_dirs/sync methods"
```

---

## Task 5: ExtensionManager Simplification & Caller Migration

**Files:**
- Modify: `src/extension/mod.rs` (remove HashMap fields)
- Modify: `src/extension/skill_ops.rs` (rewrite all queries)
- Modify: `src/extension/sync_api.rs` (update types)
- Modify: `src/extension/skill_tool.rs` (update to SkillRegistration)
- Modify: `src/extension/template.rs` (update types if needed)

- [ ] **Step 1: Remove HashMap fields from ExtensionManager**

In `src/extension/mod.rs`, remove:
```rust
skills: Arc<RwLock<HashMap<String, ExtensionSkill>>>,
commands: Arc<RwLock<HashMap<String, ExtensionCommand>>>,
agents: Arc<RwLock<HashMap<String, ExtensionAgent>>>,
```

Remove their initialization in `new()` and any direct access in `load_all()`.

- [ ] **Step 2: Rewrite skill_ops.rs**

Rewrite ALL query methods to delegate to PluginRegistry:

```rust
pub async fn get_all_skills(&self) -> Vec<SkillRegistration> {
    let reg = self.plugin_registry.read().await;
    reg.list_skills().into_iter().cloned().collect()
}

pub async fn get_skill(&self, qualified_name: &str) -> Option<SkillRegistration> {
    let reg = self.plugin_registry.read().await;
    reg.get_skill(qualified_name).cloned()
}

pub async fn get_auto_invocable_skills(&self) -> Vec<SkillRegistration> {
    let reg = self.plugin_registry.read().await;
    reg.list_skills().into_iter()
        .filter(|s| s.is_auto_invocable())
        .cloned()
        .collect()
}

pub async fn get_all_agents(&self) -> Vec<AgentRegistration> {
    let reg = self.plugin_registry.read().await;
    reg.list_agents().into_iter().cloned().collect()
}

pub async fn get_primary_agents(&self) -> Vec<AgentRegistration> {
    let reg = self.plugin_registry.read().await;
    reg.list_agents().into_iter()
        .filter(|a| a.is_primary() && !a.hidden)
        .cloned()
        .collect()
}
// ... etc for all methods
```

- [ ] **Step 3: Update sync_api.rs**

Update types from `ExtensionSkill` → `SkillRegistration`, `ExtensionAgent` → `AgentRegistration` in function signatures and return types. Since these are now type aliases, this may just compile. If not, fix.

- [ ] **Step 4: Update skill_tool.rs**

The `invoke_skill()` function takes `&ExtensionSkill` — since it's now an alias for `SkillRegistration`, check if it compiles. If new fields are accessed (like `base_dir()`), verify the method exists on SkillRegistration.

- [ ] **Step 5: Compile and fix all errors**

```bash
cargo check -p alephcore
```

This is the big integration step. Expect many errors. Fix them one by one. Common patterns:
- Struct literal construction needs new fields → add defaults
- Missing methods → verify they were added in Task 1
- Import path changes → update `use` statements

- [ ] **Step 6: Run full test suite**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "extension: remove ExtensionManager HashMaps, all queries via PluginRegistry"
```

---

## Task 6: Legacy Code Deletion

**Files:**
- Delete: `src/extension/legacy_loader.rs`
- Modify: `src/extension/mod.rs` (remove module declaration)
- Modify: `src/extension/manifest/mod.rs` (delete old functions)

- [ ] **Step 1: Delete legacy_loader.rs**

```bash
rm src/extension/legacy_loader.rs
```

Remove `mod legacy_loader;` from `src/extension/mod.rs`. Remove any `use` or reference to `legacy_loader`.

- [ ] **Step 2: Slim manifest/mod.rs**

Delete `parse_manifest_from_dir_sync()` and `auto_discover_manifest()` and `has_any_component()` from `src/extension/manifest/mod.rs`.

Keep: module declarations, `parse_frontmatter()`, `sanitize_plugin_id()`, `validate_plugin_id()`.

Check if any remaining code references these deleted functions. The discovery scanner (`discovery/scanner.rs`) likely calls `parse_manifest_from_dir_sync()` — if so, either:
- Remove that call (discovery no longer needs manifest parsing — adapters do it)
- Or replace with `AdapterRegistry::with_defaults().parse_dir()` if discovery still needs metadata

- [ ] **Step 3: Compile and fix**

```bash
cargo check -p alephcore
```

Fix remaining references to deleted functions.

- [ ] **Step 4: Run full test suite**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "extension: delete legacy_loader.rs and parse_manifest_from_dir_sync (~750 lines removed)"
```

---

## Task 7: Final Verification & Cleanup

- [ ] **Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -W clippy::all
```

- [ ] **Step 3: Verify line count reduction**

```bash
find src/extension/ -name "*.rs" -exec wc -l {} + | tail -1
```

Expected: Down from ~21,153 (lower by ~800+).

- [ ] **Step 4: Verify all installed plugins still load**

Restart server and check logs:
```bash
pkill -f "target/release/aleph-server" 2>/dev/null; sleep 2
cargo build -p alephcore --bin aleph-server --release
RUST_LOG=info,alephcore::extension=debug target/release/aleph-server start
```

Check logs for:
- "Extension loading complete: N skills, M commands" should show similar counts to before
- No panics or crashes
- All existing plugins (cli-anything, phone-control, etc.) detected

- [ ] **Step 5: Test plugin.reload RPC**

```bash
target/release/aleph-server gateway call plugin.reload -p '{"pluginId":"cli-anything"}'
```

Expected: `{"ok": true}`

- [ ] **Step 6: Final commit**

```bash
git add -A && git commit -m "extension: plugin adapter integration complete — final verification"
```
