# Workspace Wiring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the existing Workspace, Profile, and Memory filtering components into Aleph's execution path so that switching workspaces changes memory scope, persona overlay, tool access, and model selection.

**Architecture:** Context Injection pattern. An `ActiveWorkspace` struct is built in `ExecutionEngine::execute()` from WorkspaceManager data, then flows through the agent loop via `ThinkerConfig` and `RunContext`. Each subsystem reads from this context to apply workspace-specific behavior.

**Tech Stack:** Rust, Leptos/WASM (UI), SQLite (WorkspaceManager), LanceDB (Memory), JSON-RPC 2.0 (Gateway)

**Key Discovery:** Two parallel workspace systems exist:
- `gateway/workspace.rs`: SQLite-based `WorkspaceManager` with `ProfileConfig` binding (the "management" layer)
- `memory/workspace.rs` + `memory/workspace_store.rs`: LanceDB-based workspace metadata (the "memory isolation" layer)

The plan uses `WorkspaceManager` (gateway) as the source of truth for workspace ↔ profile binding, and `WorkspaceFilter` (memory) for memory query scoping.

---

## Task 1: ActiveWorkspace Struct

**Files:**
- Modify: `core/src/gateway/workspace.rs` (after L188, near UserActiveWorkspace)

**Step 1: Write the test**

Add a test to `core/src/gateway/workspace.rs` at the bottom of the test module (after L833):

```rust
#[tokio::test]
async fn test_active_workspace_from_manager() {
    let dir = tempdir().unwrap();
    let config = WorkspaceManagerConfig {
        db_path: dir.path().join("ws.db"),
        default_profile: "default".to_string(),
        archive_after_days: None,
    };
    let manager = WorkspaceManager::new(config).unwrap();

    // Load a test profile
    let mut profiles = HashMap::new();
    profiles.insert("test_profile".to_string(), ProfileConfig {
        description: Some("Test".to_string()),
        model: Some("claude-sonnet".to_string()),
        tools: vec!["memory_*".to_string()],
        system_prompt: Some("You are a test assistant.".to_string()),
        temperature: Some(0.5),
        max_tokens: Some(4096),
        ..Default::default()
    });
    manager.load_profiles(profiles);

    // Create workspace with profile
    manager.create("test-ws", "test_profile", Some("Test workspace")).await.unwrap();

    // Build ActiveWorkspace
    let active = ActiveWorkspace::from_manager(&manager, "user1").await;
    // Falls back to global since user1 has no active workspace set
    assert_eq!(active.workspace_id, "global");

    // Set active workspace
    manager.set_active("user1", "test-ws").await.unwrap();
    let active = ActiveWorkspace::from_manager(&manager, "user1").await;
    assert_eq!(active.workspace_id, "test-ws");
    assert_eq!(active.profile.model, Some("claude-sonnet".to_string()));
    assert!(active.profile.is_tool_allowed("memory_search"));
    assert!(!active.profile.is_tool_allowed("shell_exec"));
    assert!(matches!(active.memory_filter, WorkspaceFilter::Single(ref s) if s == "test-ws"));
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::workspace::tests::test_active_workspace_from_manager -- --nocapture`
Expected: FAIL — `ActiveWorkspace` not defined.

**Step 3: Implement ActiveWorkspace**

Add to `core/src/gateway/workspace.rs` after `UserActiveWorkspace` (after L188):

```rust
use crate::memory::workspace::WorkspaceFilter;

/// Runtime-resolved workspace context that flows through the execution pipeline.
/// Built once per request in ExecutionEngine, consumed by Thinker, Memory, and Executor.
pub struct ActiveWorkspace {
    pub workspace_id: String,
    pub profile: ProfileConfig,
    pub memory_filter: WorkspaceFilter,
}

impl ActiveWorkspace {
    /// Build from WorkspaceManager state for a given user.
    /// Falls back to "global" workspace with default profile if no active workspace is set.
    pub async fn from_manager(manager: &WorkspaceManager, user_id: &str) -> Self {
        let workspace = match manager.get_active(user_id).await {
            Ok(ws) => ws,
            Err(_) => {
                // Fallback: use global workspace
                manager.get("global").await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| Workspace {
                        id: "global".to_string(),
                        profile: manager.config.default_profile.clone(),
                        created_at: 0,
                        last_active_at: 0,
                        cache_state: CacheState::None,
                        env_vars: HashMap::new(),
                        description: None,
                    })
            }
        };

        let profile = manager
            .get_profile(&workspace.profile)
            .unwrap_or_default();

        let memory_filter = WorkspaceFilter::Single(workspace.id.clone());

        Self {
            workspace_id: workspace.id,
            profile,
            memory_filter,
        }
    }

    /// Create a default (global) ActiveWorkspace with empty profile.
    pub fn default_global() -> Self {
        Self {
            workspace_id: "default".to_string(),
            profile: ProfileConfig::default(),
            memory_filter: WorkspaceFilter::Single("default".to_string()),
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib gateway::workspace::tests::test_active_workspace_from_manager -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/workspace.rs
git commit -m "workspace: add ActiveWorkspace struct with from_manager constructor"
```

---

## Task 2: workspace.switch RPC Handler

**Files:**
- Modify: `core/src/gateway/handlers/workspace.rs` (add new handler)
- Modify: `core/src/bin/aleph_server/commands/start/builder/handlers.rs` (register route)

**Step 1: Write the handler**

Add to `core/src/gateway/handlers/workspace.rs` after `handle_archive` (after L249):

```rust
/// Handle workspace.switch — set the active workspace for the current user.
/// Params: { "workspace_id": "string" }
/// Returns: { "ok": true, "workspace": { ... } }
pub async fn handle_switch(
    request: JsonRpcRequest,
    db: MemoryBackend,
    workspace_manager: Option<Arc<WorkspaceManager>>,
) -> JsonRpcResponse {
    let params = match request.params.as_ref() {
        Some(p) => p,
        None => return JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing params".to_string(),
        ),
    };

    let workspace_id = match params.get("workspace_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing workspace_id parameter".to_string(),
        ),
    };

    let manager = match workspace_manager {
        Some(m) => m,
        None => return JsonRpcResponse::error(
            request.id.clone(),
            INTERNAL_ERROR,
            "WorkspaceManager not available".to_string(),
        ),
    };

    // Verify workspace exists
    match manager.get(workspace_id).await {
        Ok(Some(_ws)) => {},
        Ok(None) => return JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace not found: {}", workspace_id),
        ),
        Err(e) => return JsonRpcResponse::error(
            request.id.clone(),
            INTERNAL_ERROR,
            format!("Failed to get workspace: {}", e),
        ),
    }

    // Set active workspace (use "owner" as default user_id for single-user mode)
    let user_id = params.get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("owner");

    if let Err(e) = manager.set_active(user_id, workspace_id).await {
        return JsonRpcResponse::error(
            request.id.clone(),
            INTERNAL_ERROR,
            format!("Failed to switch workspace: {}", e),
        );
    }

    // Touch the workspace to update last_active_at
    let _ = manager.touch(workspace_id).await;

    JsonRpcResponse::success(
        request.id,
        json!({
            "ok": true,
            "workspace_id": workspace_id,
        }),
    )
}

/// Handle workspace.getActive — get the current active workspace for a user.
/// Params: { "user_id"?: "string" }
/// Returns: { "workspace_id": "string", "profile": "string" }
pub async fn handle_get_active(
    request: JsonRpcRequest,
    _db: MemoryBackend,
    workspace_manager: Option<Arc<WorkspaceManager>>,
) -> JsonRpcResponse {
    let manager = match workspace_manager {
        Some(m) => m,
        None => return JsonRpcResponse::error(
            request.id.clone(),
            INTERNAL_ERROR,
            "WorkspaceManager not available".to_string(),
        ),
    };

    let user_id = request.params.as_ref()
        .and_then(|p| p.get("user_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("owner");

    let workspace_id = manager.get_active_id(user_id).await;

    let workspace = manager.get(&workspace_id).await
        .ok()
        .flatten();

    JsonRpcResponse::success(
        request.id,
        json!({
            "workspace_id": workspace_id,
            "profile": workspace.as_ref().map(|w| &w.profile),
        }),
    )
}
```

**Step 2: Register the new RPC methods**

In `core/src/bin/aleph_server/commands/start/builder/handlers.rs`, find where `workspace.archive` is registered and add after it:

```rust
"workspace.switch" => { /* route to handle_switch with workspace_manager */ },
"workspace.getActive" => { /* route to handle_get_active with workspace_manager */ },
```

Note: The exact registration pattern depends on the handler dispatch table. Follow the existing pattern for `workspace.archive`. The handler needs access to both `MemoryBackend` and `WorkspaceManager` — check how the dispatch table passes state.

**Step 3: Run `cargo check` to verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore`
Expected: No errors

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/workspace.rs core/src/bin/aleph_server/commands/start/builder/handlers.rs
git commit -m "gateway: add workspace.switch and workspace.getActive RPC handlers"
```

---

## Task 3: Memory Isolation — Query Side

**Files:**
- Modify: `core/src/builtin_tools/memory_search.rs` (L154: workspace parameter usage)
- Modify: `core/src/builtin_tools/memory_browse.rs` (L109: workspace parameter usage)

**Context:** Currently `memory_search` reads `workspace` from args but doesn't pass it to `fact_retrieval.retrieve()`. `memory_browse` correctly passes workspace to store operations.

**Step 1: Investigate fact_retrieval.retrieve() signature**

Before making changes, read `core/src/memory/fact_retrieval.rs` (or wherever `retrieve()` is defined) to understand if it accepts a workspace/filter parameter. If not, it needs to be extended.

The key change: `retrieve()` must accept a `SearchFilter` (or at minimum a `workspace: &str` param) and apply `WorkspaceFilter::Single(workspace)` to all its internal queries.

**Step 2: Modify memory_search to pass workspace**

In `core/src/builtin_tools/memory_search.rs`, change L154 area:

Current:
```rust
let workspace = args.workspace.as_deref().unwrap_or("default");
// ... workspace only used in log
let results = fact_retrieval.retrieve(&args.query).await?;
```

Target:
```rust
let workspace = args.workspace.as_deref().unwrap_or(&active_workspace_id);
let filter = SearchFilter::new().with_workspace(WorkspaceFilter::Single(workspace.to_string()));
let results = fact_retrieval.retrieve_with_filter(&args.query, &filter).await?;
```

Note: If `retrieve_with_filter()` doesn't exist, add it as a wrapper that applies the filter. Check the actual `FactRetrieval` struct and its methods.

**Step 3: Pass active_workspace_id to the tool**

The built-in tool needs access to the active workspace ID. Check how other tools get context — likely through a `ToolContext` or `ExecutionContext` that is passed to `execute()`. Add `workspace_id` to whatever context struct the tool receives.

Follow the pattern of how `working_directory` is passed to tools through `ExecutionContext.extra`.

**Step 4: Run `cargo check` and existing tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore && cargo test -p alephcore --lib builtin_tools::memory_search`
Expected: Compilation passes, existing tests pass (may need to update test fixtures).

**Step 5: Commit**

```bash
git add core/src/builtin_tools/memory_search.rs core/src/builtin_tools/memory_browse.rs
git commit -m "memory: wire workspace filter into memory_search and memory_browse tools"
```

---

## Task 4: Memory Isolation — Store Side

**Files:**
- Investigate: `core/src/memory/` for fact creation paths
- Modify: Any tool or module that creates MemoryFacts

**Context:** When storing new facts (via AI tool calls or implicit operations), the `workspace` field on `MemoryFact` must be set to the active workspace ID instead of defaulting to `"default"`.

**Step 1: Find all fact creation sites**

Search for `workspace:` assignments on MemoryFact creation, and `DEFAULT_WORKSPACE` usage:

```bash
cd /Users/zouguojun/Workspace/Aleph && grep -rn "workspace:" core/src/ --include="*.rs" | grep -v test | grep -v "workspace_" | head -30
cd /Users/zouguojun/Workspace/Aleph && grep -rn "DEFAULT_WORKSPACE" core/src/ --include="*.rs" | head -20
```

**Step 2: Update each creation site**

For each site that creates a MemoryFact with `workspace: "default"` or `workspace: DEFAULT_WORKSPACE`:
- If it has access to tool context / execution context → use `context.workspace_id`
- If it's a system-level operation (workspace metadata itself) → keep `DEFAULT_WORKSPACE`

The `profile_update` tool and any `memory_add` / `memory_store` tools are the primary targets. Also check `scratchpad.rs` if it stores facts.

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory`
Expected: PASS

**Step 4: Commit**

```bash
git add -A core/src/
git commit -m "memory: tag new facts with active workspace ID instead of default"
```

---

## Task 5: ProfileLayer for Thinker Prompt

**Files:**
- Create: `core/src/thinker/prompt_builder/layers/profile.rs`
- Modify: `core/src/thinker/prompt_builder/layers/mod.rs` (export new module)
- Modify: `core/src/thinker/prompt_pipeline.rs` (~L62-86, register in default_layers)
- Modify: `core/src/thinker/prompt_layer.rs` (L32-40, add profile to LayerInput)

**Step 1: Add profile field to LayerInput**

In `core/src/thinker/prompt_layer.rs`, add to `LayerInput` struct (after L40):

```rust
pub struct LayerInput<'a> {
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub hydration: Option<&'a HydrationResult>,
    pub soul: Option<&'a SoulManifest>,
    pub context: Option<&'a ResolvedContext>,
    pub poe: Option<&'a PoePromptContext>,
    pub profile: Option<&'a ProfileConfig>,  // NEW: workspace profile overlay
}
```

Update all constructor functions (`basic()`, `soul()`, `context()`, etc.) to include `profile: None`.

Add a builder method:
```rust
pub fn with_profile(mut self, profile: Option<&'a ProfileConfig>) -> Self {
    self.profile = profile;
    self
}
```

**Step 2: Create ProfileLayer**

Create `core/src/thinker/prompt_builder/layers/profile.rs`:

```rust
use super::super::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

/// Injects workspace profile system_prompt as a role overlay on top of Soul.
/// Priority 75: after Soul (50), before Role (100).
pub struct ProfileLayer;

impl PromptLayer for ProfileLayer {
    fn name(&self) -> &'static str {
        "profile"
    }

    fn priority(&self) -> u32 {
        75
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let profile = match input.profile {
            Some(p) => p,
            None => return,
        };

        if let Some(ref prompt) = profile.system_prompt {
            if !prompt.is_empty() {
                output.push_str("\n\n## Current Role Context\n\n");
                output.push_str(prompt);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::profile::ProfileConfig;
    use crate::thinker::prompt_builder::prompt_layer::PromptConfig;

    #[test]
    fn test_profile_layer_injects_system_prompt() {
        let layer = ProfileLayer;
        let profile = ProfileConfig {
            system_prompt: Some("You are a crypto advisor.".to_string()),
            ..Default::default()
        };
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, None).with_profile(Some(&profile));

        let mut output = String::from("Base soul content.");
        layer.inject(&mut output, &input);

        assert!(output.contains("## Current Role Context"));
        assert!(output.contains("You are a crypto advisor."));
    }

    #[test]
    fn test_profile_layer_skips_when_no_prompt() {
        let layer = ProfileLayer;
        let profile = ProfileConfig::default(); // No system_prompt
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, None).with_profile(Some(&profile));

        let mut output = String::from("Base content.");
        layer.inject(&mut output, &input);

        assert!(!output.contains("Current Role Context"));
    }

    #[test]
    fn test_profile_layer_skips_when_no_profile() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, None); // No profile

        let mut output = String::from("Base content.");
        layer.inject(&mut output, &input);

        assert_eq!(output, "Base content.");
    }
}
```

**Step 3: Register ProfileLayer in pipeline**

In `core/src/thinker/prompt_pipeline.rs`, in `default_layers()` (~L62-86), add `ProfileLayer` after `SoulLayer`:

```rust
fn default_layers() -> Vec<Box<dyn PromptLayer>> {
    vec![
        Box::new(layers::soul::SoulLayer),
        Box::new(layers::profile::ProfileLayer),  // NEW: priority 75
        Box::new(layers::role::RoleLayer),
        // ... rest unchanged
    ]
}
```

**Step 4: Export the module**

In `core/src/thinker/prompt_builder/layers/mod.rs`, add:

```rust
pub mod profile;
```

**Step 5: Wire profile into prompt building**

In `core/src/thinker/prompt_builder/mod.rs`, update `build_system_prompt_with_soul()` (L122):

Current signature likely:
```rust
pub fn build_system_prompt_with_soul(&self, tools: &[ToolInfo], soul: &SoulManifest) -> String
```

Add optional profile parameter:
```rust
pub fn build_system_prompt_with_soul(
    &self,
    tools: &[ToolInfo],
    soul: &SoulManifest,
    profile: Option<&ProfileConfig>,
) -> String {
    let input = LayerInput::soul(&self.config, tools, soul)
        .with_profile(profile);
    self.pipeline.execute(AssemblyPath::Soul, &input)
}
```

Update all callers of this method (in `thinker/mod.rs`) to pass profile.

**Step 6: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_builder::layers::profile`
Expected: All 3 tests PASS

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore`
Expected: No errors

**Step 7: Commit**

```bash
git add core/src/thinker/prompt_builder/layers/profile.rs core/src/thinker/prompt_builder/layers/mod.rs core/src/thinker/prompt_pipeline.rs core/src/thinker/prompt_layer.rs core/src/thinker/prompt_builder/mod.rs
git commit -m "thinker: add ProfileLayer for workspace persona overlay (priority 75)"
```

---

## Task 6: Tool Whitelist Wiring

**Files:**
- Modify: `core/src/thinker/mod.rs` (L163 area: Thinker::new or think method)
- Modify: `core/src/gateway/execution_engine/engine.rs` (pass profile to Thinker)

**Context:** `ToolFilter` already has `with_profile()` and `set_profile()` methods (tool_filter.rs L127-135). The filtering logic in `filter()` (L166-226) already applies profile whitelist as "Layer 1". The problem is: profile is never set.

**Step 1: Pass ActiveWorkspace profile to ThinkerConfig**

In `core/src/thinker/mod.rs`, add an `active_profile` field to `ThinkerConfig`:

```rust
pub struct ThinkerConfig {
    pub prompt: PromptConfig,
    pub tool_filter: ToolFilterConfig,
    pub model_routing: ModelRoutingConfig,
    pub compression: CompressionConfig,
    pub think_level: ThinkLevel,
    pub soul: Option<SoulManifest>,
    pub active_profile: Option<ProfileConfig>,  // NEW
}
```

**Step 2: Apply profile to ToolFilter on Thinker construction**

In `Thinker::new()` (or wherever ToolFilter is built), add:

```rust
let tool_filter = ToolFilter::new(config.tool_filter.clone())
    .with_profile(config.active_profile.clone());
```

**Step 3: In ExecutionEngine, set active_profile on ThinkerConfig**

In `core/src/gateway/execution_engine/engine.rs`, in the ThinkerConfig assembly (~L435-451):

```rust
let thinker_config = ThinkerConfig {
    prompt: PromptConfig { ... },
    soul,
    active_profile: Some(active_workspace.profile.clone()),  // NEW
    ..ThinkerConfig::default()
};
```

**Step 4: Run `cargo check`**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/thinker/mod.rs core/src/gateway/execution_engine/engine.rs
git commit -m "thinker: wire workspace profile into ToolFilter for tool whitelist enforcement"
```

---

## Task 7: Model and Temperature Override

**Files:**
- Modify: `core/src/thinker/mod.rs` (model selection in think_with_level, ~L357)

**Context:** `ProfileConfig` has `effective_model(&self, default: &str) -> String` (profile.rs L161) and `effective_temperature() -> f32` (L166). These need to be consulted during `think_with_level()`.

**Step 1: Override model selection**

In `Thinker::think_with_level()` at L357 (model selection), change:

Current:
```rust
let model_id = self.select_model(&observation);
```

Target:
```rust
let model_id = match &self.config.active_profile {
    Some(profile) if profile.model.is_some() => {
        profile.effective_model(&self.select_model(&observation))
    }
    _ => self.select_model(&observation),
};
```

**Step 2: Pass temperature to provider call**

Find where `call_llm_with_level()` is invoked (~L370) and check if temperature can be passed. If the provider interface supports temperature override:

```rust
let temperature = self.config.active_profile
    .as_ref()
    .and_then(|p| p.temperature);
// Pass temperature to provider.process() or the call parameters
```

Note: This depends on the Provider trait's API. If `process()` doesn't accept temperature, we may need to add it to the request params or skip this for now.

**Step 3: Run `cargo check`**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore`
Expected: No errors

**Step 4: Commit**

```bash
git add core/src/thinker/mod.rs
git commit -m "thinker: apply workspace profile model and temperature overrides"
```

---

## Task 8: ExecutionEngine Integration — Wire It All Together

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs` (L377-451 area)

**This is the critical integration task** that ties Tasks 1-7 together.

**Step 1: Build ActiveWorkspace in run_agent_loop()**

In `engine.rs`, after identity resolution (~L395) and before ThinkerConfig assembly (~L435), add:

```rust
// --- Workspace Resolution ---
let active_workspace = if let Some(ref ws_manager) = self.workspace_manager {
    ActiveWorkspace::from_manager(ws_manager, "owner").await
} else {
    ActiveWorkspace::default_global()
};
```

Note: Check if `self` has a `workspace_manager` field. If not, it needs to be added to `ExecutionEngine` struct and initialized during server startup.

**Step 2: Inject workspace into ThinkerConfig**

```rust
let thinker_config = ThinkerConfig {
    prompt: PromptConfig {
        skill_instructions,
        runtime_capabilities,
        custom_instructions,
        ..PromptConfig::default()
    },
    soul,
    active_profile: Some(active_workspace.profile.clone()),
    ..ThinkerConfig::default()
};
```

**Step 3: Pass workspace ID through to tool execution context**

Find how `ExecutionContext` is built for tool calls. Add workspace_id:

```rust
// In RequestContext → ExecutionContext conversion or wherever tools get context
execution_context.extra.insert(
    "workspace_id".to_string(),
    serde_json::Value::String(active_workspace.workspace_id.clone()),
);
```

**Step 4: Ensure WorkspaceManager is accessible in ExecutionEngine**

Check how `ExecutionEngine` is constructed in server initialization. The `WorkspaceManager` needs to be available — likely through `AppState` or similar shared state.

Look at `core/src/bin/aleph_server/server_init.rs` for where `ExecutionEngine` is built and ensure `WorkspaceManager` is passed.

**Step 5: Run full `cargo check`**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore`
Expected: No errors

**Step 6: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs core/src/bin/aleph_server/server_init.rs
git commit -m "engine: integrate ActiveWorkspace into execution pipeline"
```

---

## Task 9: Control Plane UI — WorkspaceApi

**Files:**
- Modify: `core/ui/control_plane/src/api.rs` (add WorkspaceApi struct)

**Step 1: Add WorkspaceApi**

Follow the pattern of existing APIs (e.g., `ProvidersApi`). Add to `api.rs`:

```rust
pub struct WorkspaceApi;

impl WorkspaceApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<serde_json::Value>, String> {
        let result = state.rpc_call("workspace.list", json!({})).await?;
        let workspaces = result.get("workspaces")
            .cloned()
            .unwrap_or(json!([]));
        serde_json::from_value(workspaces).map_err(|e| e.to_string())
    }

    pub async fn get_active(state: &DashboardState) -> Result<serde_json::Value, String> {
        let result = state.rpc_call("workspace.getActive", json!({})).await?;
        Ok(result)
    }

    pub async fn switch(state: &DashboardState, workspace_id: &str) -> Result<(), String> {
        state.rpc_call("workspace.switch", json!({
            "workspace_id": workspace_id
        })).await?;
        Ok(())
    }

    pub async fn create(
        state: &DashboardState,
        id: &str,
        name: &str,
        profile: &str,
        description: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let result = state.rpc_call("workspace.create", json!({
            "id": id,
            "name": name,
            "profile": profile,
            "description": description,
        })).await?;
        Ok(result)
    }
}
```

**Step 2: Run `cargo check` for UI crate**

Run: `cd /Users/zouguojun/Workspace/Aleph/core/ui/control_plane && cargo check`
Expected: No errors

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/api.rs
git commit -m "ui: add WorkspaceApi for workspace management RPC calls"
```

---

## Task 10: Control Plane UI — TopBar Workspace Selector

**Files:**
- Modify: `core/ui/control_plane/src/components/top_bar.rs`

**Step 1: Add workspace selector to TopBar**

In `top_bar.rs`, add a workspace dropdown between the logo and the "New Chat" button:

```rust
// Workspace state signals
let (workspaces, set_workspaces) = signal(Vec::<serde_json::Value>::new());
let (active_ws, set_active_ws) = signal(String::from("default"));
let (ws_dropdown_open, set_ws_dropdown_open) = signal(false);

// Load workspaces on mount
leptos::task::spawn_local({
    let dashboard = expect_context::<DashboardState>();
    async move {
        if let Ok(ws_list) = WorkspaceApi::list(&dashboard).await {
            set_workspaces.set(ws_list);
        }
        if let Ok(active) = WorkspaceApi::get_active(&dashboard).await {
            if let Some(id) = active.get("workspace_id").and_then(|v| v.as_str()) {
                set_active_ws.set(id.to_string());
            }
        }
    }
});

// Switch handler
let on_switch = move |ws_id: String| {
    let dashboard = expect_context::<DashboardState>();
    set_ws_dropdown_open.set(false);
    set_active_ws.set(ws_id.clone());
    leptos::task::spawn_local(async move {
        let _ = WorkspaceApi::switch(&dashboard, &ws_id).await;
    });
};
```

Add the dropdown HTML in the view:

```rust
// In the TopBar view, after the logo div
<div class="workspace-selector">
    <button
        class="workspace-btn"
        on:click=move |_| set_ws_dropdown_open.update(|v| *v = !*v)
    >
        {move || active_ws.get()}
        <span class="dropdown-arrow">"▾"</span>
    </button>
    <Show when=move || ws_dropdown_open.get()>
        <div class="workspace-dropdown">
            <For
                each=move || workspaces.get()
                key=|ws| ws.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string()
                let:ws
            >
                {
                    let ws_id = ws.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let ws_name = ws.get("name").and_then(|v| v.as_str()).unwrap_or(&ws_id).to_string();
                    let ws_id_clone = ws_id.clone();
                    view! {
                        <button
                            class="workspace-item"
                            class:active=move || active_ws.get() == ws_id
                            on:click=move |_| on_switch(ws_id_clone.clone())
                        >
                            {ws_name}
                        </button>
                    }
                }
            </For>
        </div>
    </Show>
</div>
```

**Step 2: Run `cargo check` for UI crate**

Run: `cd /Users/zouguojun/Workspace/Aleph/core/ui/control_plane && cargo check`
Expected: No errors

**Step 3: Build the WASM bundle**

Run: `cd /Users/zouguojun/Workspace/Aleph/core/ui/control_plane && wasm-pack build --target web` (or whatever the project's WASM build command is)
Expected: Build succeeds

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/top_bar.rs
git commit -m "ui: add workspace selector dropdown to TopBar"
```

---

## Task 11: End-to-End Verification

**Step 1: Build the full project**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore`
Expected: No errors

**Step 2: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib`
Expected: All tests pass (except pre-existing failures in markdown_skill::loader)

**Step 3: Manual smoke test**

1. Start server: `cargo run --bin aleph-server --features control-plane`
2. Open Control Plane UI
3. Create a workspace via RPC: `{"method":"workspace.create","params":{"id":"test","name":"Test","profile":"default"}}`
4. Switch workspace: `{"method":"workspace.switch","params":{"workspace_id":"test"}}`
5. Verify active workspace: `{"method":"workspace.getActive","params":{}}`
6. Send a message and check that memory facts are tagged with workspace "test"

**Step 4: Final commit if any fixups needed**

```bash
git add -A && git commit -m "workspace: end-to-end wiring fixups"
```

---

## Dependency Order

```
Task 1 (ActiveWorkspace struct)
    ↓
Task 2 (workspace.switch RPC) ← independent, can parallel with Task 3-5
    ↓
Task 8 (ExecutionEngine integration) ← depends on Task 1
    ↓
Task 3 (Memory query isolation) ← depends on Task 8 for context passing
Task 4 (Memory store isolation) ← depends on Task 8 for context passing
Task 5 (ProfileLayer) ← independent, can parallel with Task 3-4
Task 6 (Tool whitelist) ← depends on Task 8
Task 7 (Model override) ← depends on Task 8
    ↓
Task 9 (UI WorkspaceApi) ← depends on Task 2
Task 10 (UI TopBar) ← depends on Task 9
    ↓
Task 11 (End-to-end verification) ← depends on all
```

**Recommended execution order:** 1 → 8 → 2 → [3, 4, 5, 6, 7 in parallel] → 9 → 10 → 11
