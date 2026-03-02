# Workspace Enhancements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement 4 workspace enhancements: per-request temperature override, cross-workspace memory query, channel→workspace routing, and config hot reload.

**Architecture:** Each enhancement is independent and additive. Temperature flows through `RequestPayload` → protocol adapters. Cross-workspace uses existing `WorkspaceFilter::Multiple`. Channel routing extends `MatchRule` + `ResolvedRoute`. Config reload extends existing `config.reload` handler with subsystem refresh.

**Tech Stack:** Rust, LanceDB, Leptos/WASM (UI), JSON-RPC 2.0

**Key Discovery:** `RequestPayload` (adapter.rs:18-37) already carries per-request params — temperature fits naturally. `config.reload` handler already exists (config.rs:22-55) — just needs subsystem refresh logic. App config is already `Arc<RwLock<Config>>` (start/mod.rs:967).

---

## Task 1: GenerationParams + RequestPayload Extension

**Files:**
- Modify: `core/src/providers/adapter.rs` (L18-37: RequestPayload)
- Modify: `core/src/providers/protocols/openai.rs` (L271-273: temperature in build_request)

**Step 1: Add temperature to RequestPayload**

In `core/src/providers/adapter.rs`, add to `RequestPayload` struct (after L37):

```rust
pub struct RequestPayload<'a> {
    pub input: &'a str,
    pub system_prompt: Option<&'a str>,
    pub image: Option<&'a ImageData>,
    pub attachments: Option<&'a [MediaAttachment]>,
    pub think_level: Option<ThinkLevel>,
    pub force_standard_mode: bool,
    pub temperature: Option<f32>,   // NEW: per-request override
    pub max_tokens: Option<u32>,    // NEW: per-request override
}
```

Update `Default` impl and all builder methods to include `temperature: None, max_tokens: None`.

Add builder methods:
```rust
pub fn with_temperature(mut self, temperature: Option<f32>) -> Self {
    self.temperature = temperature;
    self
}
pub fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
    self.max_tokens = max_tokens;
    self
}
```

**Step 2: Apply payload temperature in OpenAI protocol**

In `core/src/providers/protocols/openai.rs`, change L271-273 from:

```rust
if let Some(temp) = config.temperature {
    body["temperature"] = json!(temp);
}
```

To:
```rust
// Per-request temperature overrides provider config
let temperature = payload.temperature.or(config.temperature);
if let Some(temp) = temperature {
    body["temperature"] = json!(temp);
}
```

Do the same for `max_tokens` if it follows a similar pattern.

Check other protocol adapters (Claude, Gemini, etc.) and apply the same pattern.

**Step 3: Run `cargo check -p alephcore`**

Fix any compilation errors from the new fields.

**Step 4: Commit**

```bash
git commit -m "providers: add per-request temperature and max_tokens to RequestPayload"
```

---

## Task 2: Thinker Temperature Wiring

**Files:**
- Modify: `core/src/thinker/mod.rs` (L203-206: TODO comment, L261-296: call_llm_with_level)

**Step 1: Add resolve_generation_params()**

In `core/src/thinker/mod.rs`, after `resolve_model()` (after L223), add:

```rust
/// Resolve per-request generation parameters from workspace profile.
fn resolve_generation_params(&self) -> (Option<f32>, Option<u32>) {
    match &self.config.active_profile {
        Some(profile) => (profile.temperature, profile.max_tokens),
        None => (None, None),
    }
}
```

**Step 2: Pass temperature to provider call**

In `call_llm_with_level()` (L261-296), find where `RequestPayload` is built (or where provider.process() is called). Modify to:

1. Call `let (temperature, max_tokens) = self.resolve_generation_params();`
2. Set `payload.temperature = temperature;` and `payload.max_tokens = max_tokens;` on the RequestPayload
3. Remove the TODO comment at L203-206

If `call_llm_with_level()` doesn't directly build `RequestPayload`, trace the call path to find where it is built and inject there.

**Step 3: Add test**

```rust
#[test]
fn test_resolve_generation_params_with_profile() {
    let config = ThinkerConfig {
        active_profile: Some(ProfileConfig {
            temperature: Some(0.3),
            max_tokens: Some(4096),
            ..Default::default()
        }),
        ..Default::default()
    };
    // Build thinker with config, call resolve_generation_params
    // Assert temperature = Some(0.3), max_tokens = Some(4096)
}

#[test]
fn test_resolve_generation_params_without_profile() {
    let config = ThinkerConfig::default();
    // Assert temperature = None, max_tokens = None
}
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib thinker`

**Step 5: Commit**

```bash
git commit -m "thinker: wire workspace profile temperature into provider calls"
```

---

## Task 3: Cross-Workspace Memory — FactRetrieval Extension

**Files:**
- Modify: `core/src/memory/fact_retrieval.rs` (L139-193: retrieve_in_workspace)

**Step 1: Add retrieve_with_filter()**

After `retrieve_in_workspace()` (after L193), add:

```rust
/// Retrieve facts across workspaces using a WorkspaceFilter.
/// Supports Single, Multiple, and All workspace scopes.
pub async fn retrieve_with_filter(
    &self,
    query: &str,
    filter: WorkspaceFilter,
) -> Result<RetrievalResult, AlephError> {
    // Same logic as retrieve_in_workspace() but uses the filter directly
    // instead of building WorkspaceFilter::Single
    let search_filter = SearchFilter::valid_only(Some(NamespaceScope::Owner))
        .with_workspace(filter.clone());

    // ... rest follows retrieve_in_workspace() pattern but with filter
}
```

This is a generalization of `retrieve_in_workspace()`. Consider refactoring `retrieve_in_workspace()` to call `retrieve_with_filter(WorkspaceFilter::Single(workspace))` internally for DRY.

**Step 2: Run check**

Run: `cargo check -p alephcore`

**Step 3: Commit**

```bash
git commit -m "memory: add retrieve_with_filter for multi-workspace fact retrieval"
```

---

## Task 4: Cross-Workspace Memory — Tool Args Extension

**Files:**
- Modify: `core/src/builtin_tools/memory_search.rs` (L22-32: args, L167-184: call_impl)

**Step 1: Extend MemorySearchArgs**

Change struct (L22-32):

```rust
pub struct MemorySearchArgs {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub workspaces: Option<Vec<String>>,
    #[serde(default)]
    pub cross_workspace: Option<bool>,
}
```

**Step 2: Update workspace resolution in call_impl()**

Change L167-184 to implement priority rules:

```rust
// Resolve workspace filter
let workspace_filter = if args.cross_workspace.unwrap_or(false) {
    // cross_workspace: true → search all workspaces
    WorkspaceFilter::All
} else if let Some(ref wss) = args.workspaces {
    // workspaces: ["crypto", "health"] → search specific workspaces
    WorkspaceFilter::Multiple(wss.clone())
} else {
    // workspace: "crypto" or default → single workspace
    let default_ws = self.default_workspace.read().await;
    let ws = args.workspace.as_deref().unwrap_or(&default_ws);
    WorkspaceFilter::Single(ws.to_string())
};

// Use the new retrieve_with_filter()
let retrieval_result = self
    .fact_retrieval
    .retrieve_with_filter(&args.query, workspace_filter)
    .await;
```

**Step 3: Update tool description/schema**

Update the tool's JSON schema description to document the new parameters so the LLM knows they exist.

**Step 4: Run tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib builtin_tools::memory_search`

**Step 5: Commit**

```bash
git commit -m "memory: support cross-workspace search via workspaces and cross_workspace params"
```

---

## Task 5: Channel → Workspace — MatchRule Extension

**Files:**
- Modify: `core/src/routing/config.rs` (L30-50: RouteBinding, MatchRule)

**Step 1: Add workspace to MatchRule**

```rust
pub struct MatchRule {
    pub channel: Option<String>,
    pub account_id: Option<String>,
    pub peer: Option<PeerMatchConfig>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    pub workspace: Option<String>,  // NEW: auto-route to this workspace
}
```

Since MatchRule derives `Deserialize`, this is backward-compatible (new field is `Option`, defaults to `None`).

**Step 2: Add workspace to ResolvedRoute**

Find `ResolvedRoute` struct (resolve.rs ~L39-46) and add:

```rust
pub struct ResolvedRoute {
    pub agent_id: String,
    pub session_key: SessionKey,
    pub workspace: Option<String>,  // NEW: workspace from route binding
    // ... other fields
}
```

**Step 3: Propagate workspace in resolve_route()**

In `resolve_route()` (resolve.rs L60-160), when a binding matches, copy its `match_rule.workspace` to the `ResolvedRoute`:

```rust
ResolvedRoute {
    agent_id: binding.agent_id.clone(),
    session_key,
    workspace: binding.match_rule.workspace.clone(),
}
```

**Step 4: Run check**

Run: `cargo check -p alephcore`

**Step 5: Commit**

```bash
git commit -m "routing: add workspace field to MatchRule and ResolvedRoute"
```

---

## Task 6: Channel → Workspace — Engine Integration

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs` (L391-404: workspace resolution)
- Modify: `core/src/gateway/workspace.rs` (add from_workspace_id method)

**Step 1: Add ActiveWorkspace::from_workspace_id()**

In `core/src/gateway/workspace.rs`, add to ActiveWorkspace impl:

```rust
/// Build from a specific workspace ID (used by channel routing).
/// Unlike from_manager() which reads the user's active workspace,
/// this directly loads the specified workspace.
pub async fn from_workspace_id(manager: &WorkspaceManager, workspace_id: &str) -> Self {
    let workspace = manager.get(workspace_id).await
        .ok()
        .flatten()
        .unwrap_or_else(|| Workspace::new(workspace_id, &manager.config.default_profile));

    let profile = manager
        .get_profile(&workspace.profile)
        .unwrap_or_default();

    Self {
        workspace_id: workspace.id.clone(),
        profile,
        memory_filter: WorkspaceFilter::Single(workspace.id),
    }
}
```

**Step 2: Use route workspace in engine**

In `engine.rs` L391-404, change workspace resolution to check for route-resolved workspace:

```rust
// --- Workspace Resolution ---
// Priority: route binding workspace > user active workspace > global
let active_workspace = if let Some(ref ws_manager) = self.workspace_manager {
    if let Some(ref route_ws) = resolved_route_workspace {
        // Channel routing specifies workspace
        ActiveWorkspace::from_workspace_id(ws_manager, route_ws).await
    } else {
        // Use user's active workspace
        ActiveWorkspace::from_manager(ws_manager, "owner").await
    }
} else {
    ActiveWorkspace::default_global()
};
```

Note: You need to find where the route is resolved in the engine and extract the `workspace` field from `ResolvedRoute`. This may require reading more of the engine code to find where route resolution happens relative to the workspace resolution block.

**Step 3: Add test for from_workspace_id**

```rust
#[tokio::test]
async fn test_active_workspace_from_workspace_id() {
    // Setup manager with profiles
    // Create workspace "crypto" with "crypto_advisor" profile
    // Call from_workspace_id("crypto")
    // Assert workspace_id, profile fields match
}
```

**Step 4: Run tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib gateway::workspace`

**Step 5: Commit**

```bash
git commit -m "engine: use route-resolved workspace for channel-based routing"
```

---

## Task 7: Config Reload — Subsystem Refresh

**Files:**
- Modify: `core/src/gateway/handlers/config.rs` (L22-55: existing handle_reload)
- Possibly modify: `core/src/bin/aleph_server/commands/start/mod.rs` (config reloading)

**Step 1: Investigate existing reload handler**

Read `core/src/gateway/handlers/config.rs` L22-55 to understand what `handle_reload()` currently does. It may already reload the gateway config via `ConfigWatcher`. The gap is likely that it doesn't refresh subsystems (providers, profiles, routing rules).

**Step 2: Extend reload to refresh subsystems**

After the config is reloaded from disk, add subsystem refresh:

```rust
// In handle_reload or a new method called by it:
async fn refresh_subsystems(new_config: &Config, state: &AppState) -> Vec<String> {
    let mut reloaded = Vec::new();

    // 1. Reload profiles into WorkspaceManager
    if let Some(ref ws_manager) = state.workspace_manager {
        let profiles = new_config.profiles.clone();
        ws_manager.load_profiles(profiles);
        reloaded.push("profiles".to_string());
    }

    // 2. Reload providers (if ProviderRegistry supports it)
    // Check if provider_registry has a reload/update method

    // 3. Reload routing rules (if routing config changed)

    reloaded
}
```

**Step 3: Return reload result**

Update response format:
```json
{ "ok": true, "reloaded": ["profiles", "providers"] }
```

**Step 4: Run check**

Run: `cargo check -p alephcore`

**Step 5: Commit**

```bash
git commit -m "config: extend reload handler with subsystem refresh"
```

---

## Task 8: Config Reload — UI Button

**Files:**
- Modify: `core/ui/control_plane/src/views/settings/` (add reload button)
- Modify: `core/ui/control_plane/src/api.rs` (add reload API call)

**Step 1: Add ConfigApi::reload() to api.rs**

```rust
impl ConfigApi {
    pub async fn reload(state: &DashboardState) -> Result<Value, String> {
        state.rpc_call("config.reload", json!({})).await
    }
}
```

**Step 2: Add reload button to settings view**

In the appropriate settings view, add a button that calls `ConfigApi::reload()`:

```rust
let on_reload = move |_| {
    let dashboard = expect_context::<DashboardState>();
    spawn_local(async move {
        match ConfigApi::reload(&dashboard).await {
            Ok(result) => { /* show success notification */ }
            Err(e) => { /* show error */ }
        }
    });
};

view! {
    <button class="reload-btn" on:click=on_reload>
        "Reload Configuration"
    </button>
}
```

**Step 3: Run check**

Run: `cargo check -p aleph-control-plane`

**Step 4: Commit**

```bash
git commit -m "ui: add config reload button to settings"
```

---

## Task 9: End-to-End Verification

**Step 1: Full build**

Run: `cargo check -p alephcore && cargo check -p aleph-control-plane && cargo check --bin aleph-server`

**Step 2: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: All pass (except pre-existing markdown_skill failures)

**Step 3: Commit any fixups**

```bash
git commit -m "workspace-enhancements: end-to-end verification fixups"
```

---

## Dependency Order

```
Task 1 (RequestPayload extension) ← independent
    ↓
Task 2 (Thinker temperature wiring) ← depends on Task 1
    ↓
Task 3 (FactRetrieval extension) ← independent
    ↓
Task 4 (Memory search args) ← depends on Task 3
    ↓
Task 5 (MatchRule extension) ← independent
    ↓
Task 6 (Engine routing integration) ← depends on Task 5
    ↓
Task 7 (Config reload subsystems) ← independent
    ↓
Task 8 (UI reload button) ← depends on Task 7
    ↓
Task 9 (End-to-end verification) ← depends on all
```

**Recommended execution order:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9

Tasks 1-2, 3-4, 5-6, 7-8 are paired (foundation → wiring). Pairs are independent of each other.
