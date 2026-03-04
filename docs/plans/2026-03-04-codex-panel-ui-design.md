# Codex One-Click Login Preset UI Design

> Add a Codex subscription provider to the Panel AI Providers settings with a separate "Subscription Login" group and OAuth-based login flow.

## Background

The existing AI Providers settings page (`providers.rs`) has 12 preset providers in a flat "Quick Setup" grid. All use API key authentication. The new Codex subscription provider uses OAuth browser login (no API key), so it needs distinct UI treatment.

## Architecture

### Left Panel: Separate Group

A new "Subscription Login" section appears **above** the existing "Quick Setup" grid:

```
┌─ AI Providers ──────────────────┐
│                                  │
│  SUBSCRIPTION LOGIN              │
│  ┌─────────────────────────┐    │
│  │ ● Codex   [Connected]   │    │
│  │   codex-mini-latest      │    │
│  └─────────────────────────┘    │
│                                  │
│  QUICK SETUP                     │
│  [Anthropic] [OpenAI] [Gemini]  │
│  [DeepSeek]  [Moonshot] ...     │
│                                  │
│  CONFIGURED PROVIDERS            │
│  ...                             │
└──────────────────────────────────┘
```

### Right Panel: OAuth Detail View

When the Codex preset is selected, the right panel shows an OAuth-specific layout instead of the standard API key form:

```
┌─────────────────────────────────┐
│  ● OpenAI Codex                 │
│  Use your ChatGPT subscription  │
│                                 │
│  ┌───────────────────────────┐  │
│  │ CONNECTION STATUS          │  │
│  │ ○ Not connected            │  │
│  │                            │  │
│  │ ┌──────────────────────┐  │  │
│  │ │  Login with ChatGPT  │  │  │
│  │ └──────────────────────┘  │  │
│  └───────────────────────────┘  │
│                                 │
│  ┌───────────────────────────┐  │
│  │ CONFIGURATION              │  │
│  │ Model: [codex-mini-latest] │  │
│  │ Timeout: [120]             │  │
│  └───────────────────────────┘  │
│                                 │
│  [ Set Default ]    [ Delete ]  │
└─────────────────────────────────┘
```

After login:

```
│  ┌───────────────────────────┐  │
│  │ CONNECTION STATUS          │  │
│  │ ● Connected                │  │
│  │   Expires: 2h 30m          │  │
│  │                            │  │
│  │ [ Logout ]                 │  │
│  └───────────────────────────┘  │
```

## Data Model Changes

### ProviderPreset Extension

Add `auth_type` field to `ProviderPreset`:

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
    auth_type: &'static str,  // "api_key" (default) | "oauth"
}
```

### Codex Preset Definition

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

### Protocol Dropdown Update

Add `"chatgpt"` to the protocol `<select>`:

```html
<option value="chatgpt">"ChatGPT (Codex)"</option>
```

## API Changes

### New RPC Methods (Placeholder)

```rust
impl ProvidersApi {
    /// Trigger OAuth browser login for a subscription provider
    pub async fn oauth_login(state: &DashboardState, provider: String) -> Result<OAuthStatus, String>;

    /// Clear OAuth token for a subscription provider
    pub async fn oauth_logout(state: &DashboardState, provider: String) -> Result<(), String>;

    /// Get OAuth connection status
    pub async fn oauth_status(state: &DashboardState, provider: String) -> Result<OAuthStatus, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

These RPC methods call placeholder endpoints for now. The backend handler implementation (connecting to `chatgpt/auth.rs` OAuth flow) is a separate future task.

## Scope

### In Scope (This PR)

| File | Change |
|------|--------|
| `providers.rs` | Add `auth_type` to `ProviderPreset`, add `OAUTH_PRESETS`, new `SubscriptionLoginSection` component, modify `ProviderDetailPanel` for OAuth view |
| `api.rs` | Add `OAuthStatus` type, add `oauth_login`/`oauth_logout`/`oauth_status` RPC placeholder methods |

### Out of Scope

- Backend RPC handler implementation for OAuth methods
- Token persistence and refresh logic
- OAuth flow integration with `chatgpt/auth.rs`

## References

- Existing providers.rs: 12 presets, split-pane layout, PresetGrid + ConfiguredProviders + ProviderDetailPanel
- Codex protocol adapter: `core/src/providers/protocols/chatgpt.rs`
- OAuth auth flow: `core/src/providers/chatgpt/auth.rs`
