# Unified Plugin System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate the dual plugin system (ComponentRegistry + PluginRegistry) by unifying into a single PluginRegistry-based architecture, with independent SkillSystem and thin LegacyAdapter.

**Architecture:** Delete ComponentRegistry and its loader pipeline. All plugin state flows through PluginRegistry. Skills/Agents are managed independently by the existing SkillSystem. A ~100-line LegacyAdapter converts `.claude-plugin/plugin.json` to `PluginManifest` at discovery time. Hook system unified into a single `HookEvent` enum with snake_case serialization.

**Tech Stack:** Rust, tokio, serde, git2

**Design Doc:** `docs/plans/2026-03-09-unified-plugin-system-design.md`

---

### Task 1: Unify HookEvent Enum

Replace the dual hook enum system (HookEvent PascalCase + PluginHookEvent snake_case) with a single unified `HookEvent`.

**Files:**
- Modify: `core/src/extension/types/hooks.rs:40-80` (replace HookEvent enum)
- Modify: `core/src/extension/registry/types.rs:65-96` (delete PluginHookEvent)
- Modify: `core/src/extension/registry/plugin_registry/mod.rs` (HookRegistration uses new HookEvent)
- Modify: `core/src/extension/hooks/mod.rs` (HookExecutor uses new HookEvent)
- Modify: `core/src/extension/runtime/nodejs/mod.rs` (event string parsing)
- Modify: `core/src/extension/mod.rs:89` (re-exports)
- Modify: `core/src/extension/loader.rs:360-398` (hook loading event mapping)
- Modify: `core/src/extension/manifest/aleph_plugin_toml.rs` (HookSection event type)

**Step 1: Replace HookEvent enum in `types/hooks.rs`**

Replace lines 40-80 with the unified enum. Keep `HookKind`, `HookPriority`, `PromptScope`, `HookAction`, and `HookConfig` unchanged for now — they're still used by the shell hook system and will be cleaned up in Task 5.

```rust
/// Unified hook event types for both shell hooks and plugin hooks.
///
/// Uses **snake_case** serialization for all contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    // Agent lifecycle
    /// Before an agent starts
    BeforeAgentStart,
    /// When an agent ends
    AgentEnd,

    // Tool calls
    /// Before a tool is called
    BeforeToolCall,
    /// After a tool call completes
    AfterToolCall,
    /// When a tool result is persisted
    ToolResultPersist,

    // Message flow
    /// When a message is received from user
    MessageReceived,
    /// When a message is about to be sent
    MessageSending,
    /// After a message has been sent
    MessageSent,

    // Session lifecycle
    /// When a session starts
    SessionStart,
    /// When a session ends
    SessionEnd,

    // Context compaction
    /// Before context compaction
    BeforeCompaction,
    /// After context compaction
    AfterCompaction,

    // Gateway lifecycle
    /// When the gateway starts
    GatewayStart,
    /// When the gateway stops
    GatewayStop,

    // From legacy (valuable events)
    /// When a notification is sent
    Notification,
    /// When a permission is requested
    PermissionRequest,
}
```

**Step 2: Delete PluginHookEvent from `registry/types.rs`**

Remove the `PluginHookEvent` enum (lines 65-96). Update `HookRegistration` to use `HookEvent`:

```rust
use crate::extension::types::HookEvent;

pub struct HookRegistration {
    pub event: HookEvent,  // was: PluginHookEvent
    pub priority: i32,
    pub handler: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub plugin_id: String,
}
```

**Step 3: Update PluginRegistry hook methods in `plugin_registry/mod.rs`**

Change all `PluginHookEvent` references to `HookEvent`:
- `get_hooks_for_event(event: HookEvent)` (was `PluginHookEvent`)
- Any imports referencing `PluginHookEvent`

**Step 4: Update HookExecutor in `hooks/mod.rs`**

The HookExecutor already uses `HookEvent`. No changes needed to the executor logic — it filters by `HookEvent` already. But verify all match arms cover the new variants.

**Step 5: Update Node.js runtime event parsing in `runtime/nodejs/mod.rs`**

Update the string-to-enum mapping (lines 231-238) to use the unified `HookEvent`:

```rust
fn parse_hook_event(s: &str) -> Option<HookEvent> {
    match s {
        "before_agent_start" => Some(HookEvent::BeforeAgentStart),
        "agent_end" => Some(HookEvent::AgentEnd),
        "before_tool_call" => Some(HookEvent::BeforeToolCall),
        "after_tool_call" => Some(HookEvent::AfterToolCall),
        "tool_result_persist" => Some(HookEvent::ToolResultPersist),
        "message_received" => Some(HookEvent::MessageReceived),
        "message_sending" => Some(HookEvent::MessageSending),
        "message_sent" => Some(HookEvent::MessageSent),
        "session_start" => Some(HookEvent::SessionStart),
        "session_end" => Some(HookEvent::SessionEnd),
        "before_compaction" => Some(HookEvent::BeforeCompaction),
        "after_compaction" => Some(HookEvent::AfterCompaction),
        "gateway_start" => Some(HookEvent::GatewayStart),
        "gateway_stop" => Some(HookEvent::GatewayStop),
        "notification" => Some(HookEvent::Notification),
        "permission_request" => Some(HookEvent::PermissionRequest),
        _ => None,
    }
}
```

**Step 6: Update re-exports in `extension/mod.rs`**

Line 89: Replace `PluginHookEvent` with `HookEvent` in re-exports:
```rust
pub use registry::{HookRegistration, PluginRegistry, ToolRegistration};
// HookEvent is already exported via types::*
```

**Step 7: Update manifest TOML hook parsing**

In `extension/manifest/aleph_plugin_toml.rs`, ensure `HookSection.event` uses the unified `HookEvent` type (or maps string → `HookEvent`).

**Step 8: Update hook loading in `loader.rs`**

In `load_hooks()` (lines 360-398), the legacy `HooksFileConfig` uses old PascalCase `HookEvent` keys. Add a mapping function:

```rust
fn legacy_event_to_unified(event: &str) -> Option<HookEvent> {
    match event {
        "PreToolUse" => Some(HookEvent::BeforeToolCall),
        "PostToolUse" => Some(HookEvent::AfterToolCall),
        "PostToolUseFailure" => Some(HookEvent::AfterToolCall),
        "SessionStart" => Some(HookEvent::SessionStart),
        "SessionEnd" => Some(HookEvent::SessionEnd),
        "PreCompact" => Some(HookEvent::BeforeCompaction),
        "UserPromptSubmit" => Some(HookEvent::MessageReceived),
        "PermissionRequest" => Some(HookEvent::PermissionRequest),
        "SubagentStart" => Some(HookEvent::BeforeAgentStart),
        "SubagentStop" => Some(HookEvent::AgentEnd),
        "Stop" => Some(HookEvent::AgentEnd),
        "Notification" => Some(HookEvent::Notification),
        "Setup" => Some(HookEvent::GatewayStart),
        "ChatMessage" => Some(HookEvent::MessageReceived),
        "ChatResponse" => Some(HookEvent::MessageSent),
        "CommandExecuteBefore" => Some(HookEvent::BeforeToolCall),
        "CommandExecuteAfter" => Some(HookEvent::AfterToolCall),
        _ => None,
    }
}
```

**Step 9: Update BDD test steps**

In `core/tests/steps/extension_steps.rs`, update `parse_hook_event()` helper (line 45-62) to use unified `HookEvent` variants.

**Step 10: Run tests**

Run: `cargo test -p alephcore --lib`
Run: `cargo check -p alephcore`
Expected: PASS (all hook references use unified enum)

**Step 11: Commit**

```bash
git add -A
git commit -m "extension: unify HookEvent enum, delete PluginHookEvent"
```

---

### Task 2: Create LegacyAdapter

Create the thin adapter that converts `.claude-plugin/plugin.json` → `PluginManifest`.

**Files:**
- Create: `core/src/extension/manifest/legacy_adapter.rs`
- Modify: `core/src/extension/manifest/mod.rs` (add module, export)

**Step 1: Create `legacy_adapter.rs`**

```rust
//! Legacy Adapter — converts .claude-plugin/plugin.json to PluginManifest
//!
//! This thin adapter preserves compatibility with Claude Code plugin format
//! by converting LegacyPluginManifest into the unified PluginManifest at
//! discovery time. No runtime behavior changes.

use std::path::Path;
use super::types::{PluginManifest, AuthorInfo};
use super::legacy::LegacyPluginManifest;
use crate::extension::types::PluginKind;

/// Convert a legacy Claude plugin manifest to a V2 PluginManifest.
///
/// This is called during discovery when a `.claude-plugin/plugin.json` is found
/// but no `aleph.plugin.toml` exists in the same directory.
pub fn adapt_legacy_manifest(
    legacy: &LegacyPluginManifest,
    plugin_dir: &Path,
) -> Result<PluginManifest, String> {
    let kind = detect_plugin_kind(plugin_dir);
    let entry = detect_entry_point(plugin_dir, &kind);

    Ok(PluginManifest {
        id: legacy.name.clone(),
        name: legacy.name.clone(),
        version: legacy.version.clone(),
        description: legacy.description.clone(),
        kind,
        entry: entry.into(),
        root_dir: plugin_dir.to_path_buf(),
        author: legacy.author.as_ref().map(|a| AuthorInfo {
            name: a.name.clone().unwrap_or_default(),
            email: a.email.clone(),
            url: a.url.clone(),
        }),
        homepage: legacy.homepage.clone(),
        repository: None,
        license: legacy.license.clone(),
        keywords: legacy.keywords.clone().unwrap_or_default(),
        // V2-only fields default to None/empty
        config_schema: None,
        config_ui_hints: Default::default(),
        permissions: Vec::new(),
        extensions: Vec::new(),
        tools_v2: None,
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: None,
        capabilities_v2: None,
        wasm_capabilities: None,
        wasm_resource_limits: None,
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
    })
}

/// Detect plugin kind from directory contents.
fn detect_plugin_kind(dir: &Path) -> PluginKind {
    if dir.join("plugin.wasm").exists() || has_wasm_files(dir) {
        PluginKind::Wasm
    } else if dir.join("package.json").exists() || dir.join("index.js").exists() {
        PluginKind::NodeJs
    } else {
        PluginKind::Static
    }
}

/// Detect the entry point file.
fn detect_entry_point(dir: &Path, kind: &PluginKind) -> String {
    match kind {
        PluginKind::Wasm => {
            if dir.join("plugin.wasm").exists() {
                "plugin.wasm".to_string()
            } else {
                // Find first .wasm file
                std::fs::read_dir(dir)
                    .ok()
                    .and_then(|entries| {
                        entries.filter_map(|e| e.ok())
                            .find(|e| e.path().extension().map_or(false, |ext| ext == "wasm"))
                            .map(|e| e.file_name().to_string_lossy().to_string())
                    })
                    .unwrap_or_else(|| "plugin.wasm".to_string())
            }
        }
        PluginKind::NodeJs => "index.js".to_string(),
        PluginKind::Static => ".".to_string(),
    }
}

fn has_wasm_files(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries.filter_map(|e| e.ok())
                .any(|e| e.path().extension().map_or(false, |ext| ext == "wasm"))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_nodejs_plugin() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("package.json"), "{}").unwrap();
        assert!(matches!(detect_plugin_kind(temp.path()), PluginKind::NodeJs));
    }

    #[test]
    fn test_detect_wasm_plugin() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("plugin.wasm"), &[]).unwrap();
        assert!(matches!(detect_plugin_kind(temp.path()), PluginKind::Wasm));
    }

    #[test]
    fn test_detect_static_plugin() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("SKILL.md"), "# Test").unwrap();
        assert!(matches!(detect_plugin_kind(temp.path()), PluginKind::Static));
    }

    #[test]
    fn test_adapt_legacy_manifest() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("index.js"), "").unwrap();
        let legacy = LegacyPluginManifest {
            name: "test-plugin".to_string(),
            version: Some("1.0.0".to_string()),
            description: Some("A test plugin".to_string()),
            ..Default::default()
        };
        let result = adapt_legacy_manifest(&legacy, temp.path()).unwrap();
        assert_eq!(result.id, "test-plugin");
        assert_eq!(result.name, "test-plugin");
        assert!(matches!(result.kind, PluginKind::NodeJs));
    }
}
```

**Step 2: Add module to `manifest/mod.rs`**

Add `pub mod legacy_adapter;` and export the adapter function.

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib legacy_adapter`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/extension/manifest/legacy_adapter.rs core/src/extension/manifest/mod.rs
git commit -m "extension: add LegacyAdapter for .claude-plugin/plugin.json conversion"
```

---

### Task 3: Integrate LegacyAdapter into Discovery

Wire the adapter into the plugin discovery pipeline so legacy plugins are auto-converted at scan time.

**Files:**
- Modify: `core/src/extension/discovery/scanner.rs` (add legacy fallback)
- Modify: `core/src/extension/discovery/mod.rs` (if needed for imports)

**Step 1: Update discovery scanner**

In `extension/discovery/scanner.rs`, find the function that scans for plugin manifests. Add a fallback path:

```rust
// In the plugin scanning logic:
fn scan_plugin_dir(path: &Path) -> Option<PluginManifest> {
    // Priority 1: V2 manifest (aleph.plugin.toml)
    if let Ok(manifest) = crate::extension::manifest::parse_aleph_plugin_toml_sync(path) {
        return Some(manifest);
    }
    // Priority 2: V1 JSON (aleph.plugin.json)
    if let Ok(manifest) = crate::extension::manifest::parse_aleph_plugin_sync(path) {
        return Some(manifest);
    }
    // Priority 3: Legacy Claude format (.claude-plugin/plugin.json)
    let legacy_path = path.join(".claude-plugin").join("plugin.json");
    if legacy_path.exists() {
        if let Ok(legacy) = crate::extension::manifest::parse_legacy_manifest_sync(&legacy_path) {
            if let Ok(manifest) = crate::extension::manifest::legacy_adapter::adapt_legacy_manifest(&legacy, path) {
                return Some(manifest);
            }
        }
    }
    None
}
```

**Step 2: Run tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 3: Commit**

```bash
git add -A
git commit -m "discovery: integrate LegacyAdapter into plugin scanning pipeline"
```

---

### Task 4: Redirect Skills/Agents Queries to SkillSystem

Before deleting ComponentRegistry, redirect all skill/agent queries from ComponentRegistry to SkillSystem. This makes ComponentRegistry's skills/agents storage unused.

**Files:**
- Modify: `core/src/extension/mod.rs:285-320` (redirect skill/agent methods)
- Modify: `core/src/extension/mod.rs:166` (make skill_system non-Option)
- Modify: `core/src/extension/mod.rs:830-850` (primary/sub agent methods)

**Step 1: Make skill_system non-Option**

Change line 166 from `skill_system: Option<crate::skill::SkillSystem>` to:
```rust
skill_system: crate::skill::SkillSystem,
```

Update the constructor to always create SkillSystem:
```rust
skill_system: crate::skill::SkillSystem::new(),
```

Update `skill_system()` accessor:
```rust
pub fn skill_system(&self) -> &crate::skill::SkillSystem {
    &self.skill_system
}
```

Remove `init_skill_system()` method — initialization moves into `load_all()`.

**Step 2: Redirect skill/agent queries**

Replace lines 287-320. Skills now come from SkillSystem, agents still from ComponentRegistry temporarily (agents will be migrated in Task 5):

```rust
/// Get all skills from SkillSystem
pub async fn get_all_skills(&self) -> Vec<ExtensionSkill> {
    // Delegate to SkillSystem — convert SkillManifest → ExtensionSkill
    self.skill_system.list_skills().await
        .into_iter()
        .map(|m| ExtensionSkill::from_manifest(&m))
        .collect()
}

/// Get auto-invocable skills
pub async fn get_auto_invocable_skills(&self) -> Vec<ExtensionSkill> {
    self.get_all_skills().await
        .into_iter()
        .filter(|s| s.is_auto_invocable())
        .collect()
}

/// Get a specific skill by qualified name
pub async fn get_skill(&self, qualified_name: &str) -> Option<ExtensionSkill> {
    let id = crate::domain::skill::SkillId::new(qualified_name);
    self.skill_system.get_skill(&id).await
        .map(|m| ExtensionSkill::from_manifest(&m))
}
```

Note: `ExtensionSkill::from_manifest()` may need to be added as a conversion method. If SkillManifest and ExtensionSkill are too different, use a thin adapter function instead.

**Step 3: Keep agent queries via ComponentRegistry for now**

Agents don't have an independent system yet. Leave `get_all_agents()`, `get_agent()`, `get_primary_agents()`, `get_sub_agents()` reading from ComponentRegistry for now. They'll be migrated when we address agents independently (out of scope for this plan).

**Step 4: Update load_all to init SkillSystem**

In `load_all()`, after the ComponentLoader runs, initialize SkillSystem with discovered skill directories:

```rust
pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
    let summary = self.loader.load_all(
        &self.discovery,
        &self.registry,
        &self.hook_executor,
    ).await?;

    // Initialize SkillSystem with discovered directories
    let skill_dirs = self.discovery.discover_skill_dirs()
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.path)
        .collect();
    if let Err(e) = self.skill_system.init(skill_dirs).await {
        tracing::warn!("Failed to init skill system: {}", e);
    }

    let mut cache = self.cache_state.write().await;
    cache.loaded = true;
    cache.loaded_at = Some(Instant::now());
    Ok(summary)
}
```

**Step 5: Update sync_api.rs**

Update `SyncExtensionManager` to match new signatures (non-Option skill_system).

**Step 6: Run tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 7: Commit**

```bash
git add -A
git commit -m "extension: redirect skill queries to SkillSystem"
```

---

### Task 5: Redirect Plugin Queries to PluginRegistry

Replace all plugin-related queries that go through ComponentRegistry with PluginRegistry queries.

**Files:**
- Modify: `core/src/extension/mod.rs:778-821` (get_plugin_info)
- Modify: `core/src/extension/mod.rs:824-826` (get_plugin)
- Modify: `core/src/extension/mod.rs:764-771` (get_mcp_servers)
- Modify: `core/src/gateway/handlers/plugins/handlers.rs` (remove hack)

**Step 1: Rewrite `get_plugin_info()`**

Replace lines 778-821 with a single-source query:

```rust
/// Get all plugin info — single source from PluginRegistry
pub async fn get_plugin_info(&self) -> Vec<PluginInfo> {
    self.plugin_registry
        .read()
        .await
        .list_plugins()
        .into_iter()
        .map(|record| PluginInfo {
            name: record.id.clone(),
            version: record.version.clone(),
            description: record.description.clone(),
            enabled: record.status.is_active(),
            path: record.root_dir.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            skills_count: 0,
            commands_count: 0,
            agents_count: 0,
            hooks_count: record.hook_count,
            mcp_servers_count: 0,
        })
        .collect()
}
```

**Step 2: Rewrite `get_plugin()`**

```rust
pub async fn get_plugin(&self, name: &str) -> Option<PluginRecord> {
    self.plugin_registry.read().await.get_plugin(name).cloned()
}
```

Note: Return type changes from `Option<ExtensionPlugin>` to `Option<PluginRecord>`. Update callers.

**Step 3: Update `get_mcp_servers()` (lines 764-771)**

Remove the ComponentRegistry loop that reads from `plugin.mcp_servers`. MCP servers from plugins should be registered through PluginRegistry or discovered independently. For now, remove the dead code path:

```rust
// Remove this block:
// for plugin in self.registry.read().await.get_all_plugins() {
//     for (name, config) in &plugin.mcp_servers {
//         ...
//     }
// }
```

**Step 4: Simplify `handle_list()` in handlers.rs**

Remove the `ensure_loaded()` + hack merge. Now it's just:

```rust
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let manager = match get_extension_manager() {
        Some(m) => m,
        Err(e) => return e.with_id(request.id),
    };
    if let Err(e) = manager.ensure_loaded().await {
        tracing::warn!("Failed to load extensions: {}", e);
    }
    let plugins: Vec<PluginInfoJson> = manager
        .get_plugin_info()
        .await
        .into_iter()
        .map(PluginInfoJson::from)
        .collect();
    JsonRpcResponse::success(request.id, serde_json::json!({ "plugins": plugins }))
}
```

**Step 5: Run tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 6: Commit**

```bash
git add -A
git commit -m "extension: redirect plugin queries to PluginRegistry"
```

---

### Task 6: Delete ComponentRegistry

Now that all queries bypass ComponentRegistry, delete it and clean up all references.

**Files:**
- Delete: `core/src/extension/registry/component_registry.rs`
- Modify: `core/src/extension/registry/mod.rs` (remove module)
- Modify: `core/src/extension/mod.rs` (remove `registry` field)
- Modify: `core/src/extension/loader.rs` (remove ComponentRegistry parameter)
- Modify: `core/src/extension/types/plugins.rs` (remove ExtensionPlugin if unused)
- Modify: `core/src/lib.rs:187` (remove ComponentRegistry export)

**Step 1: Delete `component_registry.rs`**

```bash
rm core/src/extension/registry/component_registry.rs
```

**Step 2: Remove from `registry/mod.rs`**

Remove line 36: `mod component_registry;`
Remove line 40: `pub use component_registry::*;`

**Step 3: Remove `registry` field from ExtensionManager**

In `extension/mod.rs`:
- Delete line 145: `registry: Arc<RwLock<ComponentRegistry>>`
- Delete the `registry` initialization in constructor
- Delete `reload()` line that clears registry: `self.registry.write().await.clear();`
- Delete all `self.registry.read().await` and `self.registry.write().await` calls
- Agent queries still needed? If agent queries still read ComponentRegistry, keep a minimal agent storage or move to SkillSystem. For now, add agents to SkillSystem or keep a simple `agents: Arc<RwLock<HashMap<String, ExtensionAgent>>>` field.

**Step 4: Update `ComponentLoader::load_all()`**

Remove the `registry` parameter. Skills go to SkillSystem (already handled in Task 4). Agents need a new home. Plugins go to PluginRegistry. Hooks go to HookExecutor.

New signature:
```rust
pub async fn load_all(
    &self,
    discovery: &DiscoveryManager,
    hook_executor: &Arc<RwLock<super::hooks::HookExecutor>>,
) -> ExtensionResult<LoadSummary>
```

Remove all `registry.write().await.register_*()` calls. The loader now only:
1. Discovers and loads agents (stored somewhere accessible)
2. Loads legacy plugins' hooks into HookExecutor
3. Returns summary

Skills are handled by SkillSystem. Plugins are handled by PluginRegistry/PluginLoader.

**Step 5: Remove ComponentRegistry from `lib.rs` exports**

Line 187: Remove `ComponentRegistry` from the export list.

**Step 6: Fix all compilation errors**

Search for remaining `ComponentRegistry` references and fix them:
- `core/src/bin/aleph/commands/plugins.rs` (uses ComponentLoader)
- `core/src/gateway/handlers/plugins/handlers.rs` (uses ComponentLoader)
- Any test files

**Step 7: Run tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 8: Commit**

```bash
git add -A
git commit -m "extension: delete ComponentRegistry, unify to PluginRegistry"
```

---

### Task 7: Clean Up Discovery System

Delete the standalone `discovery/scanner.rs` (`DirectoryScanner`) and consolidate into the extension discovery system.

**Files:**
- Modify: `core/src/discovery/mod.rs` (remove scanner exports)
- Modify: `core/src/discovery/scanner.rs` (keep if still used by DiscoveryManager, else delete)
- Modify: `core/src/extension/mod.rs` (remove DirectoryScanner hack in get_plugin_info)
- Modify: `core/src/discovery/paths.rs` (move Claude constants to legacy_adapter)

**Step 1: Check if DirectoryScanner is still used**

After Task 5-6, `get_plugin_info()` no longer uses `DirectoryScanner`. Check if `DiscoveryManager` wraps `DirectoryScanner` — if yes, `DirectoryScanner` is still needed for the core discovery pipeline. Only remove the hack usage, not the scanner itself.

The `DiscoveryManager` in `core/src/discovery/mod.rs` wraps `DirectoryScanner` (line 120). It's the core discovery mechanism. **Don't delete it.** Only remove the direct `DirectoryScanner::new()` hack in `get_plugin_info()`.

**Step 2: Remove hack from `get_plugin_info()`**

The old hack (lines 791-817) that directly instantiated `DirectoryScanner` is already gone after Task 5. Verify it's clean.

**Step 3: Move Claude constants**

In `discovery/paths.rs`, `PLUGIN_MANIFEST_DIR` and `PLUGIN_MANIFEST_FILE` are only needed by the legacy adapter now. Move them or keep them — they're small constants and moving could break other references. Keep them but mark with a comment:

```rust
/// Legacy Claude plugin manifest directory (used by LegacyAdapter)
pub const PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";
/// Legacy Claude plugin manifest file (used by LegacyAdapter)
pub const PLUGIN_MANIFEST_FILE: &str = "plugin.json";
```

**Step 4: Run tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "discovery: clean up DirectoryScanner hack, consolidate constants"
```

---

### Task 8: Rename ComponentLoader → ContentLoader

Rename to reflect its new, narrower responsibility: loading Markdown content (skills/agents) only.

**Files:**
- Rename: `core/src/extension/loader.rs` → `core/src/extension/content_loader.rs`
- Modify: `core/src/extension/mod.rs` (module declaration)
- Modify: `core/src/lib.rs` (export name)
- Modify: All files importing `ComponentLoader`

**Step 1: Rename the file**

```bash
mv core/src/extension/loader.rs core/src/extension/content_loader.rs
```

**Step 2: Rename the struct**

In `content_loader.rs`, rename `ComponentLoader` → `ContentLoader` everywhere.

**Step 3: Update module declaration in `mod.rs`**

Change `mod loader;` to `mod content_loader;`

**Step 4: Update re-exports**

In `extension/mod.rs`: `pub use content_loader::*;`
In `lib.rs`: Replace `ComponentLoader` with `ContentLoader`.

**Step 5: Update all importers**

Search for `ComponentLoader` across the codebase and update:
- `core/src/bin/aleph/commands/plugins.rs`
- `core/src/gateway/handlers/plugins/handlers.rs`
- Any test files

**Step 6: Run tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 7: Commit**

```bash
git add -A
git commit -m "extension: rename ComponentLoader to ContentLoader"
```

---

### Task 9: Update Architecture Documentation and Module Comments

Update the module-level documentation to reflect the new unified architecture.

**Files:**
- Modify: `core/src/extension/mod.rs:1-44` (architecture diagram)
- Modify: `core/src/extension/registry/mod.rs:1-35` (module docs)

**Step 1: Update architecture diagram**

Replace lines 1-44 in `extension/mod.rs`:

```rust
//! Extension System - Plugin and Skill Management
//!
//! This module provides a unified extension system for Aleph.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        ExtensionManager                                │
//! │  - Orchestrates discovery, loading, registration, integration          │
//! └────────────────────────────┬───────────────────────────────────────────┘
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          ▼                   ▼                   ▼
//!     PluginRegistry      PluginLoader        SkillSystem
//!   (unified registry)  (Node.js, WASM)    (skills, agents)
//!          │                   │                   │
//!          └───────────────────┼───────────────────┘
//!                              │
//!          ┌───────────────────┴───────────────────┐
//!          ▼                                       ▼
//!     HookExecutor                          ContentLoader
//!     (unified hooks)                    (Markdown parsing)
//! ```
```

**Step 2: Update registry module docs**

Remove references to ComponentRegistry in `registry/mod.rs`.

**Step 3: Commit**

```bash
git add -A
git commit -m "docs: update extension system architecture documentation"
```

---

### Task 10: Sync Enable/Disable with PluginRegistry

Fix the enable/disable handlers to properly synchronize filesystem state with PluginRegistry.

**Files:**
- Modify: `core/src/gateway/handlers/plugins/handlers.rs:267-333`

**Step 1: Update `handle_enable()`**

After removing the `.disabled` file, also update PluginRegistry:

```rust
pub async fn handle_enable(request: JsonRpcRequest) -> JsonRpcResponse {
    // ... existing file system logic ...

    // Sync with PluginRegistry
    if let Some(manager) = get_extension_manager().ok() {
        let mut registry = manager.get_plugin_registry_mut().await;
        registry.enable_plugin(&params.name);
    }

    JsonRpcResponse::success(request.id, serde_json::json!({ "ok": true }))
}
```

**Step 2: Update `handle_disable()` similarly**

```rust
// After creating .disabled file:
if let Some(manager) = get_extension_manager().ok() {
    let mut registry = manager.get_plugin_registry_mut().await;
    registry.disable_plugin(&params.name);
}
```

**Step 3: Add `get_plugin_registry_mut()` to ExtensionManager**

```rust
pub async fn get_plugin_registry_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, PluginRegistry> {
    self.plugin_registry.write().await
}
```

**Step 4: Run tests**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "gateway: sync enable/disable with PluginRegistry"
```

---

### Task 11: Final Verification and Cleanup

Run all tests, clippy, and verify the system works end-to-end.

**Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: PASS (may have pre-existing failures in markdown_skill::loader::tests)

**Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: 0 warnings

**Step 3: Build release**

Run: `just build`
Expected: PASS

**Step 4: Verify plugin listing**

Start the server and test `plugins.list` returns V2 plugins from `~/.aleph/plugins/`.

**Step 5: Commit any final fixes**

```bash
git add -A
git commit -m "extension: final cleanup after plugin system unification"
```
