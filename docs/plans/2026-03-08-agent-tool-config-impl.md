# Per-Agent Tool Configuration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow users to configure per-agent tool access with group-level and individual-tool-level granularity, via both TOML config and Panel UI.

**Architecture:** Extend existing `AgentDefinition.skills` (whitelist) with `skills_blacklist` field. Add tool group metadata constant. Replace the existing read-only Panel Tools tab with interactive group/tool toggles. All changes backward compatible.

**Tech Stack:** Rust (core), Leptos/WASM (panel), TOML (config), JSON-RPC (RPC)

---

### Task 1: Add `skills_blacklist` to AgentDefinition and AgentDefaults

**Files:**
- Modify: `core/src/config/types/agents_def.rs:83-112` (AgentDefaults) and `core/src/config/types/agents_def.rs:182-231` (AgentDefinition)

**Step 1: Add field to AgentDefaults**

In `AgentDefaults` struct (line ~107, after `skills` field), add:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_blacklist: Option<Vec<String>>,
```

**Step 2: Add field to AgentDefinition**

In `AgentDefinition` struct (line ~207, after `skills` field), add:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_blacklist: Option<Vec<String>>,
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (both structs derive Default, Option<Vec<String>> defaults to None)

**Step 4: Commit**

```bash
git add core/src/config/types/agents_def.rs
git commit -m "config: add skills_blacklist to AgentDefinition and AgentDefaults"
```

---

### Task 2: Add `skills_blacklist` to ResolvedAgent with cascade resolution

**Files:**
- Modify: `core/src/config/agent_resolver.rs:42-78` (ResolvedAgent struct) and `core/src/config/agent_resolver.rs:271-310` (resolve logic)

**Step 1: Add field to ResolvedAgent struct**

After `pub skills: Vec<String>` (line ~74), add:

```rust
    /// Resolved list of blocked skills/tools
    pub skills_blacklist: Vec<String>,
```

**Step 2: Add resolution logic**

After the skills resolution block (line ~276), add:

```rust
        // 5b. Resolve skills_blacklist: agent.skills_blacklist > defaults.skills_blacklist > vec![]
        let skills_blacklist = agent
            .skills_blacklist
            .clone()
            .or_else(|| defaults.skills_blacklist.clone())
            .unwrap_or_default();
```

**Step 3: Add field to ResolvedAgent construction**

In the `ResolvedAgent { ... }` block (line ~296-310), add after `skills,`:

```rust
            skills_blacklist,
```

**Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 5: Fix test — update ResolvedAgent construction in test**

In `test_agent_instance_config_from_resolved` (`core/src/gateway/agent_instance.rs:~750`), add `skills_blacklist: vec![]` to the `ResolvedAgent` construction.

Search for any other test that constructs `ResolvedAgent` directly and add the field there too.

Run: `cargo test -p alephcore --lib agent_resolver agent_instance`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/config/agent_resolver.rs core/src/gateway/agent_instance.rs
git commit -m "config: resolve skills_blacklist with cascade fallback"
```

---

### Task 3: Map `skills_blacklist` to `tool_blacklist` in AgentInstanceConfig

**Files:**
- Modify: `core/src/gateway/agent_instance.rs:63-82` (from_resolved method)

**Step 1: Update from_resolved()**

In `AgentInstanceConfig::from_resolved()` (line ~66-82), change:

```rust
            tool_blacklist: vec![],
```

to:

```rust
            tool_blacklist: agent.skills_blacklist.clone(),
```

**Step 2: Add test for blacklist mapping**

Add a new test after `test_agent_instance_config_from_resolved`:

```rust
    #[test]
    fn test_agent_instance_config_blacklist_from_resolved() {
        use crate::config::agent_resolver::ResolvedAgent;
        use crate::config::types::profile::ProfileConfig;

        let resolved = ResolvedAgent {
            id: "restricted".to_string(),
            name: "Restricted Agent".to_string(),
            is_default: false,
            workspace_path: PathBuf::from("/tmp/test-workspace"),
            agent_dir: PathBuf::from("/tmp/test-agents/restricted"),
            profile: ProfileConfig::default(),
            soul: None,
            agents_md: None,
            memory_md: None,
            model: "claude-sonnet-4-5".to_string(),
            skills: vec!["*".to_string()],
            skills_blacklist: vec!["bash".to_string(), "code_exec".to_string()],
            subagent_policy: None,
        };

        let config = AgentInstanceConfig::from_resolved(&resolved);
        assert_eq!(config.tool_whitelist, vec!["*"]);
        assert_eq!(config.tool_blacklist, vec!["bash", "code_exec"]);
        assert!(config.is_tool_allowed("search"));
        assert!(!config.is_tool_allowed("bash"));
        assert!(!config.is_tool_allowed("code_exec"));
    }
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib agent_instance`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/gateway/agent_instance.rs
git commit -m "gateway: map skills_blacklist to tool_blacklist in AgentInstanceConfig"
```

---

### Task 4: Add tool group definitions

**Files:**
- Create: `core/src/executor/builtin_registry/groups.rs`
- Modify: `core/src/executor/builtin_registry/mod.rs`

**Step 1: Create groups.rs**

```rust
//! Tool group definitions for Panel UI display.
//!
//! Groups are display-only metadata — they don't affect tool filtering.
//! TOML config uses individual tool names/globs, not group IDs.

use serde::Serialize;

/// A logical group of tools for UI display
#[derive(Debug, Clone, Serialize)]
pub struct ToolGroup {
    /// Group identifier (e.g., "search_web")
    pub id: &'static str,
    /// Human-readable group name
    pub name: &'static str,
    /// Tool names belonging to this group
    pub tools: &'static [&'static str],
}

/// All tool groups (ordered for UI display)
pub static TOOL_GROUPS: &[ToolGroup] = &[
    ToolGroup {
        id: "search_web",
        name: "搜索与网络",
        tools: &["search", "web_fetch", "youtube"],
    },
    ToolGroup {
        id: "file_code",
        name: "文件与代码",
        tools: &["file_ops", "bash", "code_exec", "pdf_generate"],
    },
    ToolGroup {
        id: "memory_knowledge",
        name: "记忆与知识",
        tools: &["memory_search", "memory_browse", "read_skill", "list_skills"],
    },
    ToolGroup {
        id: "content_gen",
        name: "内容生成",
        tools: &["generate_image"],
    },
    ToolGroup {
        id: "system_config",
        name: "系统与配置",
        tools: &["desktop", "config_read", "config_update"],
    },
    ToolGroup {
        id: "agent_mgmt",
        name: "Agent 管理",
        tools: &[
            "agent_create",
            "agent_switch",
            "agent_list",
            "agent_delete",
            "sessions_list",
            "sessions_send",
            "subagent_spawn",
            "subagent_steer",
            "subagent_kill",
            "escalate_task",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::builtin_registry::BUILTIN_TOOL_DEFINITIONS;

    #[test]
    fn test_all_builtin_tools_have_a_group() {
        let grouped: Vec<&str> = TOOL_GROUPS
            .iter()
            .flat_map(|g| g.tools.iter().copied())
            .collect();

        for def in BUILTIN_TOOL_DEFINITIONS.iter() {
            assert!(
                grouped.contains(&def.name),
                "Builtin tool '{}' is not in any group",
                def.name
            );
        }
    }

    #[test]
    fn test_no_duplicate_tools_across_groups() {
        let mut seen = std::collections::HashSet::new();
        for group in TOOL_GROUPS {
            for tool in group.tools {
                assert!(
                    seen.insert(tool),
                    "Tool '{}' appears in multiple groups",
                    tool
                );
            }
        }
    }
}
```

**Step 2: Add module to mod.rs**

In `core/src/executor/builtin_registry/mod.rs`, after `mod definitions;` add:

```rust
mod groups;
```

And in the `pub use` block, add:

```rust
pub use groups::{ToolGroup, TOOL_GROUPS};
```

**Step 3: Verify**

Run: `cargo test -p alephcore --lib builtin_registry`
Expected: PASS (all 25 tools covered, no duplicates)

**Step 4: Commit**

```bash
git add core/src/executor/builtin_registry/groups.rs core/src/executor/builtin_registry/mod.rs
git commit -m "tools: add tool group definitions for Panel UI"
```

---

### Task 5: Add `skills_blacklist` to AgentPatch and update handler

**Files:**
- Modify: `core/src/config/agent_manager.rs:45-54` (AgentPatch struct) and `core/src/config/agent_manager.rs:197-288` (update method)

**Step 1: Add field to AgentPatch**

In `AgentPatch` struct (line ~53, after `skills`), add:

```rust
    pub skills_blacklist: Option<Vec<String>>,
```

**Step 2: Add patch application logic in update()**

In `AgentManager::update()` (line ~273, after the `skills` patch block), add:

```rust
        if let Some(skills_blacklist) = &patch.skills_blacklist {
            let mut arr = Array::new();
            for s in skills_blacklist {
                arr.push(s.as_str());
            }
            agent_table["skills_blacklist"] = toml_edit::value(arr);
        }
```

**Step 3: Verify**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/config/agent_manager.rs
git commit -m "config: add skills_blacklist to AgentPatch and update handler"
```

---

### Task 6: Add `agents.tools_schema` RPC handler

**Files:**
- Modify: `core/src/gateway/handlers/agents.rs` (add handler function)
- Modify: `core/src/gateway/handlers/mod.rs` (register placeholder)
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs` (register real handler)

**Step 1: Add handler function**

At the bottom of `core/src/gateway/handlers/agents.rs` (before any `#[cfg(test)]`), add:

```rust
/// Handle agents.tools_schema — return tool group metadata for Panel UI
pub async fn handle_tools_schema(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::executor::builtin_registry::{BUILTIN_TOOL_DEFINITIONS, TOOL_GROUPS};

    let groups: Vec<serde_json::Value> = TOOL_GROUPS
        .iter()
        .map(|group| {
            let tools: Vec<serde_json::Value> = group
                .tools
                .iter()
                .map(|tool_name| {
                    let description = BUILTIN_TOOL_DEFINITIONS
                        .iter()
                        .find(|d| d.name == *tool_name)
                        .map(|d| d.description)
                        .unwrap_or("");
                    json!({
                        "name": tool_name,
                        "description": description,
                    })
                })
                .collect();
            json!({
                "id": group.id,
                "name": group.name,
                "tools": tools,
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "groups": groups }))
}
```

**Step 2: Register placeholder in mod.rs**

In `core/src/gateway/handlers/mod.rs`, after the `agents.files.delete` placeholder (line ~546), add:

```rust
        registry.register("agents.tools_schema", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.tools_schema requires initialization — wire in Gateway startup".to_string())
        });
```

**Step 3: Register real handler in handlers.rs builder**

In `core/src/bin/aleph/commands/start/builder/handlers.rs`, after `agents.files.delete` registration (line ~742), add:

```rust
    register_handler!(server, "agents.tools_schema", agents::handle_tools_schema);
```

Note: This handler is stateless (no `manager` or `event_bus` dependency), so it only takes the request.

**Step 4: Verify**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/agents.rs core/src/gateway/handlers/mod.rs core/src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "gateway: add agents.tools_schema RPC handler"
```

---

### Task 7: Add `tools_schema` API method to Panel

**Files:**
- Modify: `apps/panel/src/api/agents.rs`

**Step 1: Add response types**

After `FilesListResponse` (line ~73), add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGroupInfo {
    pub id: String,
    pub name: String,
    pub tools: Vec<ToolInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsSchemaResponse {
    pub groups: Vec<ToolGroupInfo>,
}
```

**Step 2: Add API method**

In `impl AgentsApi`, after `files_delete` method (line ~149), add:

```rust
    pub async fn tools_schema(state: &DashboardState) -> Result<ToolsSchemaResponse, String> {
        let result = state.rpc_call("agents.tools_schema", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
```

**Step 3: Verify WASM compilation**

Run: `cd apps/panel && cargo check --target wasm32-unknown-unknown`
Expected: PASS

**Step 4: Commit**

```bash
git add apps/panel/src/api/agents.rs
git commit -m "panel: add tools_schema API method and types"
```

---

### Task 8: Replace Panel Tools tab with interactive group/tool toggles

**Files:**
- Rewrite: `apps/panel/src/views/agents/tools.rs`

**Step 1: Rewrite tools.rs**

Replace the entire file with the interactive implementation:

```rust
//! Tools Tab — per-agent tool configuration with group/tool toggles

use std::collections::HashSet;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use crate::api::agents::{AgentsApi, ToolGroupInfo};
use crate::context::DashboardState;

#[component]
pub fn ToolsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State signals
    let groups = RwSignal::new(Vec::<ToolGroupInfo>::new());
    let enabled_tools = RwSignal::new(HashSet::<String>::new());
    let original_tools = RwSignal::new(HashSet::<String>::new());
    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let error_msg = RwSignal::new(Option::<String>::None);
    let success_msg = RwSignal::new(Option::<String>::None);

    // Load tool schema + current agent config
    let id_for_load = agent_id.clone();
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = id_for_load.clone();
        spawn_local(async move {
            // Load schema
            let schema = match AgentsApi::tools_schema(&dash).await {
                Ok(s) => s,
                Err(e) => {
                    error_msg.set(Some(format!("Failed to load tool schema: {}", e)));
                    is_loading.set(false);
                    return;
                }
            };

            // Collect all tool names
            let all_tools: HashSet<String> = schema.groups.iter()
                .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
                .collect();

            // Load agent definition for current skills/blacklist
            let (skills, blacklist) = match AgentsApi::get(&dash, &id).await {
                Ok(detail) => {
                    let def = &detail.definition;
                    let skills: Vec<String> = def.get("skills")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_else(|| vec!["*".to_string()]);
                    let blacklist: Vec<String> = def.get("skills_blacklist")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    (skills, blacklist)
                }
                Err(e) => {
                    error_msg.set(Some(format!("Failed to load agent: {}", e)));
                    is_loading.set(false);
                    return;
                }
            };

            // Compute enabled set from skills whitelist + blacklist
            let blacklist_set: HashSet<String> = blacklist.into_iter().collect();
            let enabled: HashSet<String> = if skills.contains(&"*".to_string()) {
                // Wildcard: all tools enabled except blacklisted
                all_tools.difference(&blacklist_set).cloned().collect()
            } else {
                // Explicit whitelist minus blacklist
                skills.into_iter()
                    .filter(|s| !blacklist_set.contains(s))
                    .collect()
            };

            groups.set(schema.groups);
            enabled_tools.set(enabled.clone());
            original_tools.set(enabled);
            is_loading.set(false);
        });
    });

    // Toggle a single tool
    let toggle_tool = move |tool_name: String| {
        enabled_tools.update(|set| {
            if set.contains(&tool_name) {
                set.remove(&tool_name);
            } else {
                set.insert(tool_name);
            }
        });
        success_msg.set(None);
    };

    // Toggle entire group
    let toggle_group = move |tool_names: Vec<String>| {
        enabled_tools.update(|set| {
            let all_enabled = tool_names.iter().all(|t| set.contains(t));
            if all_enabled {
                for t in &tool_names {
                    set.remove(t);
                }
            } else {
                for t in tool_names {
                    set.insert(t);
                }
            }
        });
        success_msg.set(None);
    };

    // Has changes
    let has_changes = Memo::new(move |_| {
        enabled_tools.get() != original_tools.get()
    });

    // Save
    let id_for_save = agent_id.clone();
    let save = move |_| {
        let id = id_for_save.clone();
        is_saving.set(true);
        error_msg.set(None);
        success_msg.set(None);

        spawn_local(async move {
            let enabled = enabled_tools.get();
            let all_tools: HashSet<String> = groups.get().iter()
                .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
                .collect();

            let disabled: HashSet<String> = all_tools.difference(&enabled).cloned().collect();

            // Choose minimal representation
            let patch = if disabled.is_empty() {
                // All enabled
                json!({ "skills": ["*"], "skills_blacklist": [] })
            } else if disabled.len() <= enabled.len() {
                // Few disabled: wildcard + blacklist
                let mut bl: Vec<String> = disabled.into_iter().collect();
                bl.sort();
                json!({ "skills": ["*"], "skills_blacklist": bl })
            } else {
                // Few enabled: explicit whitelist
                let mut wl: Vec<String> = enabled.iter().cloned().collect();
                wl.sort();
                json!({ "skills": wl, "skills_blacklist": [] })
            };

            match AgentsApi::update(&dash, &id, patch).await {
                Ok(()) => {
                    original_tools.set(enabled_tools.get());
                    success_msg.set(Some("Saved".to_string()));
                }
                Err(e) => {
                    error_msg.set(Some(format!("Failed to save: {}", e)));
                }
            }
            is_saving.set(false);
        });
    };

    // Reset
    let reset = move |_| {
        let all_tools: HashSet<String> = groups.get().iter()
            .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
            .collect();
        enabled_tools.set(all_tools);
        success_msg.set(None);
    };

    view! {
        <div class="space-y-6">
            // Error message
            {move || error_msg.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            // Success message
            {move || success_msg.get().map(|msg| view! {
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{msg}</div>
            })}

            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center">"Loading..."</div>
                    }.into_any();
                }

                let current_groups = groups.get();

                view! {
                    <div class="space-y-4">
                        {current_groups.into_iter().map(|group| {
                            let group_tools: Vec<String> = group.tools.iter().map(|t| t.name.clone()).collect();
                            let group_tools_for_toggle = group_tools.clone();
                            let group_tools_for_state = group_tools.clone();

                            view! {
                                <div class="bg-surface-raised border border-border rounded-xl overflow-hidden">
                                    // Group header
                                    <div class="flex items-center justify-between px-5 py-3 bg-surface-sunken/50 border-b border-border">
                                        <span class="text-sm font-semibold text-text-primary">{group.name.clone()}</span>
                                        <button
                                            class="relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none"
                                            class=("bg-primary", {
                                                let gt = group_tools_for_state.clone();
                                                move || gt.iter().all(|t| enabled_tools.get().contains(t))
                                            })
                                            class=("bg-border", {
                                                let gt = group_tools_for_state.clone();
                                                move || !gt.iter().all(|t| enabled_tools.get().contains(t))
                                            })
                                            on:click=move |_| toggle_group(group_tools_for_toggle.clone())
                                        >
                                            <span
                                                class="inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform"
                                                class=("translate-x-4.5", {
                                                    let gt = group_tools_for_state.clone();
                                                    move || gt.iter().all(|t| enabled_tools.get().contains(t))
                                                })
                                                class=("translate-x-0.5", {
                                                    let gt = group_tools_for_state.clone();
                                                    move || !gt.iter().all(|t| enabled_tools.get().contains(t))
                                                })
                                            />
                                        </button>
                                    </div>
                                    // Tool list
                                    <div class="divide-y divide-border/50">
                                        {group.tools.into_iter().map(|tool| {
                                            let tool_name = tool.name.clone();
                                            let tool_name_for_toggle = tool_name.clone();
                                            let tool_name_for_state = tool_name.clone();
                                            let tool_name_for_state2 = tool_name.clone();

                                            view! {
                                                <div class="flex items-center justify-between px-5 py-2.5">
                                                    <div class="flex-1 min-w-0">
                                                        <span class="text-sm font-medium text-text-primary">{tool_name.clone()}</span>
                                                        <p class="text-xs text-text-tertiary truncate mt-0.5">{tool.description.clone()}</p>
                                                    </div>
                                                    <button
                                                        class="relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none ml-4 flex-shrink-0"
                                                        class=("bg-primary", move || enabled_tools.get().contains(&tool_name_for_state))
                                                        class=("bg-border", move || !enabled_tools.get().contains(&tool_name_for_state2))
                                                        on:click=move |_| toggle_tool(tool_name_for_toggle.clone())
                                                    >
                                                        <span
                                                            class="inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform"
                                                            class=("translate-x-4.5", {
                                                                let tn = tool_name.clone();
                                                                move || enabled_tools.get().contains(&tn)
                                                            })
                                                            class=("translate-x-0.5", {
                                                                let tn = tool_name.clone();
                                                                move || !enabled_tools.get().contains(&tn)
                                                            })
                                                        />
                                                    </button>
                                                </div>
                                            }
                                        }).collect_view()}
                                    </div>
                                </div>
                            }
                        }).collect_view()}

                        // Action buttons
                        <div class="flex justify-end gap-3 pt-2">
                            <button
                                class="px-4 py-2 text-sm font-medium text-text-secondary bg-surface-raised border border-border rounded-lg hover:bg-surface-sunken transition-colors"
                                on:click=reset
                            >
                                "Reset to All"
                            </button>
                            <button
                                class="px-4 py-2 text-sm font-medium text-white bg-primary rounded-lg hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                disabled=move || !has_changes.get() || is_saving.get()
                                on:click=save
                            >
                                {move || if is_saving.get() { "Saving..." } else { "Save" }}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
```

**Step 2: Build WASM panel**

Run: `cd apps/panel && cargo check --target wasm32-unknown-unknown`
Expected: PASS

**Step 3: Commit**

```bash
git add apps/panel/src/views/agents/tools.rs
git commit -m "panel: replace read-only tools tab with interactive group/tool toggles"
```

---

### Task 9: Full build and verification

**Step 1: Build core**

Run: `cargo build -p alephcore`
Expected: PASS

**Step 2: Run all affected tests**

Run: `cargo test -p alephcore --lib agent_resolver agent_instance builtin_registry`
Expected: PASS

**Step 3: Build release binary**

Run: `cargo build --bin aleph --release`
Expected: PASS

**Step 4: Build panel WASM**

Run: `cd apps/panel && trunk build` (or `cargo check --target wasm32-unknown-unknown`)
Expected: PASS

**Step 5: Final commit if any fixups needed**
