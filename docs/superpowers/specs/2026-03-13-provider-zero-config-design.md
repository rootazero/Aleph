# Provider Zero-Config UX Design

## Goal

Achieve zero-learning-cost AI provider configuration through a first-run setup wizard and enhanced settings panel, leveraging the model discovery system (ModelRegistry) to replace manual model input with automatic probe + grouped dropdown selection.

## Context

- **ModelRegistry** is merged and production-ready: API probe → preset fallback → empty, with 24h cache TTL
- **`models.*` RPC handlers** exist: `list`, `get`, `refresh`, `set_model`, `set_default`, `capabilities`
- **Panel settings page** has preset grid + custom provider dual-track layout
- **Model selection** is currently a manual text input — the primary UX gap
- **No first-run guidance** — new users land on empty dashboard with no direction

## Architecture: Hybrid (Plan C)

Frontend (Leptos/WASM) owns wizard step orchestration and UI flow control. Core (Gateway) provides aggregated RPC endpoints. No backend wizard session management — wizard state lives in Leptos reactive signals.

### New RPC Endpoints

#### `providers.probe`

Combines connection test + model discovery in a single call. Used by both wizard and enhanced settings form.

**Request:**
```json
{
  "protocol": "openai",
  "api_key": "sk-...",
  "base_url": null
}
```

**Response:**
```json
{
  "success": true,
  "latency_ms": 234,
  "models": [
    {"id": "gpt-4o", "name": "GPT-4o", "capabilities": ["chat", "vision", "tools"]},
    {"id": "gpt-4o-mini", "name": "GPT-4o Mini", "capabilities": ["chat", "vision", "tools"]}
  ],
  "model_source": "api",
  "error": null
}
```

Note: `models` items are `DiscoveredModel` from `core/src/providers/adapter.rs`. The `name` field is `Option<String>` — frontend must fall back to `id` when `name` is `None`.
```

**Building the temporary `ProviderConfig`:** The probe request carries only `protocol`, `api_key`, and `base_url`. The handler builds a `ProviderConfig` using `ProviderConfig::test_config(protocol)` as the base (sets sensible defaults for `model`, `timeout_seconds`, etc.), then overrides `api_key` and `base_url` from request params. The `model` field is irrelevant for probing — it's only needed for chat requests, not for listing models or ping tests.

**Implementation logic:**
1. Build temporary `ProviderConfig` from params (as described above)
2. Get protocol adapter from `ProtocolRegistry`
3. Attempt model discovery via `ModelRegistry::list_models()` — this calls the provider's list models API endpoint
4. If model discovery returns results → connection is implicitly verified; optionally measure latency via a lightweight ping
5. If model discovery fails (network error, auth error) → fall back to preset models from `model-presets.toml` and include the error message in response
6. `model_source` reflects actual source: `"api"` if models came from the provider, `"preset"` if from fallback

**Relation to `providers.test`:** `providers.test` is kept as-is (existing callers depend on it). `providers.probe` is its superset.

#### `providers.needs_setup`

Panel calls this on startup to decide whether to show the wizard.

**Request:** (no params)

**Response:**
```json
{
  "needs_setup": true,
  "provider_count": 0,
  "has_verified": false
}
```

**Logic:** Check `config.providers` for at least one provider where `enabled == true && verified == true`. If none found, `needs_setup = true`. On any error reading config, default to `needs_setup: true` (defensive — better to show wizard than to silently skip it).

### New Shared UI Logic API

#### `ModelsApi`

Added to `shared_ui_logic/src/api/models.rs`:

```rust
pub struct ModelsApi<C: AlephConnector> { rpc: RpcClient<C> }

impl<C: AlephConnector> ModelsApi<C> {
    pub async fn list(&self, provider: Option<&str>, refresh: bool) -> Result<Vec<ModelInfo>>;
    pub async fn refresh(&self, provider: Option<&str>) -> Result<Vec<ModelInfo>>;
}
```

## Setup Wizard

### Trigger

Panel startup → connect to Gateway → call `providers.needs_setup` → if `needs_setup == true`, show wizard overlay.

### Steps

#### Step 1: Select Provider

- Display preset grid (same `ProviderPreset` data as settings page, single source)
- Click preset card → advance to Step 2
- "Custom Provider" entry at bottom → shows protocol + base_url form, then Step 2
- Ollama preset: no API key needed → skip Step 2, go directly to Step 3

#### Step 2: Enter Credentials

- Shows selected provider name + icon
- API key input (placeholder from preset, e.g. "sk-...")
- OAuth type (e.g. Codex): detected via `ProviderPreset.auth_type == "oauth"`. Shows existing OAuth login button (reuse `SubscriptionLoginSection` component from providers.rs) instead of API key input. OAuth token storage handled by existing `oauth.*` RPC handlers — no new OAuth logic needed.
- On paste: **auto-trigger** `providers.probe` with loading indicator
- Success → green checkmark + latency, auto-advance to Step 3
- Failure → red error, user can edit and retry
- "Skip verification" link available → proceeds with preset model list

#### Step 3: Select Model

- Grouped dropdown: models grouped by capability (Chat / Vision / Tools / Other)
- Models with capabilities outside the three primary groups appear in "Other"
- Same model may appear in multiple groups
- Preset-recommended model highlighted as default
- Source label per model: `API` or `Preset`
- "Refresh" button → calls `models.refresh`
- Confirm button → `providers.create` + `providers.setDefault`

#### Completion

- Success animation
- Prompt: "Configure image generation too?" → Yes: navigate to Generation Providers settings / No: close wizard, enter main interface

### Frontend State

```rust
struct WizardState {
    step: RwSignal<WizardStep>,                    // Step1 | Step2 | Step3 | Complete
    selected_preset: RwSignal<Option<ProviderPreset>>,
    api_key: RwSignal<String>,
    probe_result: RwSignal<Option<ProbeResult>>,
    selected_model: RwSignal<Option<String>>,
    is_probing: RwSignal<bool>,
}
```

No backend session — all state in Leptos signals, discarded on completion or cancellation.

### Cancellation

- Close button (X) in top-right corner of wizard overlay; Escape key also closes
- Cancellation at any step discards all wizard state — no partial provider is created
- Provider is only persisted on Step 3 confirm (single atomic `providers.create` call)
- After cancellation, user can re-trigger wizard from settings or by reloading Panel

## Enhanced Settings Form

Settings page layout (preset grid + custom provider list + detail panel) remains unchanged. Three enhancements to `ProviderDetailPanel`:

### Enhancement 1: Model Grouped Dropdown

- Replace `<input>` with grouped dropdown component (shared with wizard Step 3)
- On provider load or API key change → auto-fetch `models.list`
- Groups: Chat / Vision / Tools, sorted by name within group
- Fallback: "Enter custom model name" option at dropdown bottom → switches to text input
- Refresh icon button → `models.refresh`

### Enhancement 2: API Key Auto-Probe

- Debounced (500ms) → auto-call `providers.probe` on key change
- Status indicator right of input: spinner / green check + latency / red X + error
- Success → auto-refresh model dropdown
- Non-blocking: user can save before probe completes

### Enhancement 3: Provider Status Visualization

- Provider list item shows model source tag (API / Preset / Manual)
- Verified provider: green dot; unverified: gray dot (existing `verified` field)
- Hover tooltip: last probe time. Note: `ModelRegistry::last_refreshed` returns `Instant` (monotonic), which cannot be displayed as wall-clock time. The `providers.probe` handler should record a `chrono::Utc::now()` timestamp in the cache entry or probe response. Alternatively, the tooltip can show relative time ("5 minutes ago") computed from the `Instant` elapsed duration.

## Shared Components

Three components extracted for reuse between wizard and settings form:

```
apps/panel/src/components/
  model_selector.rs      — Grouped dropdown with capability sections, search, custom input fallback
  probe_indicator.rs     — Probe status display (loading/success/error)
  api_key_input.rs       — API key field with auto-probe and status indicator
```

## File Map

### Core (New/Modified)

| File | Action | Description |
|------|--------|-------------|
| `core/src/gateway/handlers/providers.rs` | Modify | Add `handle_probe`, `handle_needs_setup` |
| `core/src/gateway/handlers/mod.rs` | Modify | Register new RPC methods |

### Shared UI Logic (New/Modified)

| File | Action | Description |
|------|--------|-------------|
| `shared_ui_logic/src/api/models.rs` | Create | `ModelsApi` RPC client |
| `shared_ui_logic/src/api/providers.rs` | Modify | Add `probe()` method to `ProvidersApi`, add `verified` field to `ProviderInfo` |
| `shared_ui_logic/src/api/mod.rs` | Modify | Export `ModelsApi` |

### Panel (New/Modified)

| File | Action | Description |
|------|--------|-------------|
| `apps/panel/src/components/model_selector.rs` | Create | Grouped model dropdown |
| `apps/panel/src/components/probe_indicator.rs` | Create | Probe status indicator |
| `apps/panel/src/components/api_key_input.rs` | Create | API key input with auto-probe |
| `apps/panel/src/components/mod.rs` | Create | Component module exports |
| `apps/panel/src/views/settings/providers.rs` | Modify | Use new components in detail panel |
| `apps/panel/src/views/wizard/setup_wizard.rs` | Create | Setup wizard view (3 steps) |
| `apps/panel/src/views/wizard/mod.rs` | Create | Wizard module exports |
| `apps/panel/src/app.rs` | Modify | Add wizard trigger on startup |
| `apps/panel/src/api.rs` | Modify | Add `ModelsApi` wrappers (probe is part of `ProvidersApi`) |

## Testing Strategy

- **L1 Unit tests**: `handle_probe` response building, `handle_needs_setup` logic
- **L2 Integration tests (wiremock)**: `providers.probe` with mock API responses, fallback scenarios
- **L3 BDD**: Wizard flow scenarios (needs_setup detection, probe success/failure, model selection)
- **Manual**: Visual verification of wizard and enhanced form UX

## Out of Scope

- Wizard for generation/embedding providers (only prompted as optional next step)
- Model capability metadata from provider APIs (continue using heuristic inference)
- Modifying existing `providers.test` behavior
- Restructuring settings page layout
