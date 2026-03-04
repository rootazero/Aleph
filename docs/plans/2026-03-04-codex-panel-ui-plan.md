# Codex Panel UI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Codex one-click login preset to Panel AI Providers settings with separate group and OAuth flow UI.

**Architecture:** Extend existing `providers.rs` with a new "Subscription Login" section above "Quick Setup". Add OAuth-specific detail panel view. Add placeholder RPC methods in `api.rs`.

**Tech Stack:** Leptos 0.8 (CSR), Tailwind CSS classes, WebSocket JSON-RPC 2.0

---

### Task 1: Add OAuth API types and placeholder RPC methods

**Files:**
- Modify: `core/ui/control_plane/src/api.rs`

**Step 1: Add OAuthStatus type**

After `TestResult` struct (around line 398), add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

**Step 2: Add OAuth RPC methods to ProvidersApi**

After `test_connection` method (around line 502), add:

```rust
/// Trigger OAuth browser login for a subscription provider
pub async fn oauth_login(state: &DashboardState, provider: String) -> Result<OAuthStatus, String> {
    let params = serde_json::json!({ "provider": provider });
    let result = state.rpc_call("providers.oauthLogin", params).await?;
    serde_json::from_value(result)
        .map_err(|e| format!("Failed to parse OAuth status: {}", e))
}

/// Clear OAuth token for a subscription provider
pub async fn oauth_logout(state: &DashboardState, provider: String) -> Result<(), String> {
    let params = serde_json::json!({ "provider": provider });
    state.rpc_call("providers.oauthLogout", params).await?;
    Ok(())
}

/// Get OAuth connection status
pub async fn oauth_status(state: &DashboardState, provider: String) -> Result<OAuthStatus, String> {
    let params = serde_json::json!({ "provider": provider });
    let result = state.rpc_call("providers.oauthStatus", params).await?;
    serde_json::from_value(result)
        .map_err(|e| format!("Failed to parse OAuth status: {}", e))
}
```

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/api.rs
git commit -m "panel: add OAuth API types and placeholder RPC methods"
```

---

### Task 2: Add Codex preset with auth_type and Subscription Login section

**Files:**
- Modify: `core/ui/control_plane/src/views/settings/providers.rs`

**Step 1: Add `auth_type` field to `ProviderPreset`**

```rust
struct ProviderPreset {
    name: &'static str,
    protocol: &'static str,
    model: &'static str,
    base_url: &'static str,
    description: &'static str,
    api_key_placeholder: &'static str,
    icon_color: &'static str,
    needs_api_key: bool,
    auth_type: &'static str,  // "api_key" | "oauth"
}
```

Add `auth_type: "api_key"` to all 12 existing presets in `PRESETS`.

**Step 2: Add `OAUTH_PRESETS` constant**

```rust
const OAUTH_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        name: "codex",
        protocol: "chatgpt",
        model: "codex-mini-latest",
        base_url: "https://chatgpt.com",
        description: "OpenAI Codex via ChatGPT subscription",
        api_key_placeholder: "",
        icon_color: "#10A37F",
        needs_api_key: false,
        auth_type: "oauth",
    },
];
```

Update `find_preset` to also search `OAUTH_PRESETS`:

```rust
fn find_preset(name: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().chain(OAUTH_PRESETS.iter()).find(|p| p.name == name)
}
```

**Step 3: Add `SubscriptionLoginSection` component**

New component rendered above `PresetGrid` in `ProvidersView`:

```rust
#[component]
fn SubscriptionLoginSection(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                "Subscription Login"
            </h2>
            <div class="space-y-2">
                {OAUTH_PRESETS.iter().map(|preset| {
                    // Similar to PresetGrid card but with Login badge
                    // ...
                }).collect_view()}
            </div>
        </div>
    }
}
```

Insert into `ProvidersView` scrollable content before `<PresetGrid>`:

```rust
<SubscriptionLoginSection providers=providers selected=selected />
```

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/views/settings/providers.rs
git commit -m "panel: add Codex preset with Subscription Login section"
```

---

### Task 3: Add OAuth detail panel for Codex provider

**Files:**
- Modify: `core/ui/control_plane/src/views/settings/providers.rs`
- Modify: `core/ui/control_plane/src/api.rs` (import OAuthStatus)

**Step 1: Modify `ProviderDetailPanel` for OAuth providers**

When the selected provider's preset has `auth_type == "oauth"`, render an OAuth-specific view instead of the API key form:

- **Connection Status card**: Shows connected/not connected with green/gray indicator
- **Login button**: "Login with ChatGPT" — calls `ProvidersApi::oauth_login`
- **Logout button** (when connected): Calls `ProvidersApi::oauth_logout`
- **Model input**: Still editable
- **Timeout input**: Still editable
- **No API Key / Base URL fields**

The key logic branch in `ProviderDetailPanel`:

```rust
let is_oauth = preset_info.map(|p| p.auth_type == "oauth").unwrap_or(false);

if is_oauth {
    // Render OAuth login view
    view! { <OAuthProviderView ... /> }
} else {
    // Existing API key form
    view! { ... }
}
```

**Step 2: Add `OAuthProviderView` component**

```rust
#[component]
fn OAuthProviderView(
    preset: &'static ProviderPreset,
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
    // form signals for model, timeout, etc.
) -> impl IntoView {
    let oauth_status = RwSignal::new(Option::<OAuthStatus>::None);
    let logging_in = RwSignal::new(false);

    // Check OAuth status on mount
    // ...

    view! {
        // Connection Status card
        // Login/Logout button
        // Model + Timeout configuration card
        // Set Default + Delete buttons
    }
}
```

**Step 3: Add "chatgpt" to protocol dropdown**

In the existing protocol `<select>`, add:
```rust
<option value="chatgpt">"ChatGPT (Codex)"</option>
```

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/views/settings/providers.rs
git commit -m "panel: add OAuth detail panel for Codex provider"
```

---

### Task 4: Verify build and final cleanup

**Step 1: Build WASM**

```bash
cd core/ui/control_plane && trunk build 2>&1 | head -50
```

Or if trunk is not available:
```bash
cd core && cargo check -p alephcore 2>&1 | tail -20
```

**Step 2: Fix any compilation errors**

**Step 3: Commit any fixes**

```bash
git add -A
git commit -m "panel: fix codex UI compilation issues"
```
