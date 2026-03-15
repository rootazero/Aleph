# Provider Config Testing Design

## Summary

Three-layer test pyramid for production-grade validation of the provider configuration architecture after the model→models migration. Layer 1 (Rust logic probes) tests serialization and handler logic directly. Layer 2 (RPC integration probes) tests through real HTTP/WebSocket. Layer 3 (Playwright E2E) tests browser UI interactions.

## Motivation

The recent simplify-model-config refactor touched 4 config types, ~70 call sites, deleted model discovery infrastructure, and rewrote all provider settings UIs. Current test coverage is limited to unit tests and serialization checks. No integration tests verify the RPC layer, and no e2e tests verify the frontend UI. This spec defines comprehensive test coverage to validate the entire stack.

## Architecture

```
┌─────────────────────────────┐
│  Layer 3: Playwright E2E    │  Browser interaction validation
│  (4 settings + wizard)      │
├─────────────────────────────┤
│  Layer 2: RPC Integration   │  Real HTTP/WebSocket network layer
│  (server + JSON-RPC client) │
├─────────────────────────────┤
│  Layer 1: Rust Logic Probes │  Direct handler function calls
│  (serde / CRUD / defaults)  │
└─────────────────────────────┘
```

### Test Data Strategy

Programmatic construction (builder pattern). No fixture files.

- Rust layers: `ProviderConfig::test_config("model")` and programmatic `Config` building
- Playwright: RPC calls inject config state before each test

### External API Strategy

Split into mock and real:
- Default tests: fully isolated, no network required
- Optional `#[ignore]` tests: require `OPENAI_API_KEY` env var, test real API connectivity

---

## Layer 1: Rust Logic Probes

**File:** `core/tests/provider_config_probe.rs`

### Test Harness

`ProviderConfigTestHarness` wraps handler dependencies:

```rust
struct ProviderConfigTestHarness {
    config: Arc<RwLock<Config>>,
    token_manager: Arc<TokenManager>,
    // ... other handler dependencies
}

impl ProviderConfigTestHarness {
    fn new() -> Self { /* programmatic default config */ }
    fn with_provider(mut self, name: &str, config: ProviderConfig) -> Self { /* inject */ }
    async fn call(&self, method: &str, params: Value) -> Value { /* dispatch to handler */ }
}
```

### Scenarios

#### 1.1 Serialization & Backward Compatibility

| Test | Input | Expected |
|------|-------|----------|
| TOML backward compat | `model = "gpt-4o"` | deserializes as `models: ["gpt-4o"]` |
| Multi-model TOML | `models = ["a", "b", "c"]` | round-trip preserves order and values |
| Empty list rejected | `models = []` | deserialization error |
| Empty string filtered | `models = ["", "gpt-4o", " "]` | `models: ["gpt-4o"]` after filtering |
| default_model() | `models: ["first", "second"]` | returns `"first"` |
| Serialization output | programmatic config | serializes as `models = [...]`, never `model = ...` |

Test all four config types: `ProviderConfig`, `EmbeddingProviderConfig`, `RerankConfig`, `GenerationProviderConfig`.

#### 1.2 GenerationProviderConfig Specifics

| Test | Input | Expected |
|------|-------|----------|
| model_aliases access | `model_aliases: {"alias": "real"}` | HashMap accessible as `config.model_aliases` |
| Optional models | `models: []` (empty) | valid for generation (model is optional) |
| default_model() None | empty models vec | returns `None` |

#### 1.3 Provider CRUD via Handlers

| Test | Method | Validation |
|------|--------|------------|
| Create provider | `providers.create` with `models: ["x","y"]` | `providers.get` returns same models |
| List providers | `providers.list` | response includes `models: Vec<String>` field |
| Update models | `providers.update` changing models | get returns new models |
| Delete provider | `providers.delete` | subsequent list excludes it |
| Set default | `providers.setDefault` | default provider's `models[0]` is the default model |

#### 1.4 Edge Cases

| Test | Input | Expected |
|------|-------|----------|
| Unicode model name | `models: ["模型-v1"]` | round-trip correct |
| Long model name | 1000 char string | no panic, stores correctly |
| Special chars | `models: ["org/model-v2.1-beta"]` | TOML and JSON safe |
| Duplicate models | `models: ["a", "a"]` | allowed, stored as-is |
| 100 models | vec of 100 entries | stores and retrieves correctly |

---

## Layer 2: RPC Integration Probes

**File:** `core/tests/provider_rpc_probe.rs`

### Test Harness

`RpcTestServer` starts a real Aleph HTTP server:

```rust
struct RpcTestServer {
    addr: SocketAddr,
    ws_url: String,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl RpcTestServer {
    async fn start(config: Config) -> Self {
        // Bind to 127.0.0.1:0 (random port)
        // Start Aleph server with programmatic config
        // Return server handle
    }

    async fn rpc_call(&self, method: &str, params: Value) -> Value {
        // Connect via tokio-tungstenite WebSocket
        // Send JSON-RPC request
        // Return parsed response
    }
}

impl Drop for RpcTestServer {
    fn drop(&mut self) { /* send shutdown signal */ }
}
```

### Scenarios

#### 2.1 RPC Endpoint Validation

| Test | Method | Validation |
|------|--------|------------|
| WebSocket connects | WS handshake | connection established |
| providers.list | RPC call | valid JSON response with models array |
| providers.create | create + get | created provider accessible |
| providers.update | update models | persisted correctly |
| providers.delete | delete + list | provider removed |
| providers.setDefault | set default | response confirms |
| providers.test (mock) | test with unreachable URL | returns `success: false` with error |

#### 2.2 Deleted Endpoints

| Test | Method | Expected |
|------|--------|----------|
| models.list | RPC call | JSON-RPC error: method not found |
| models.get | RPC call | method not found |
| models.capabilities | RPC call | method not found |
| models.refresh | RPC call | method not found |
| providers.probe | RPC call | method not found |
| embedding_providers.probe | RPC call | method not found |

#### 2.3 Robustness

| Test | Scenario | Expected |
|------|----------|----------|
| Concurrent reads | 10 parallel providers.list | all succeed, no corruption |
| Large models list | provider with 100 models | serializes over WebSocket correctly |
| Invalid JSON | malformed RPC request | error response, no crash |

#### 2.4 Optional Real API Tests (`#[ignore]`)

| Test | Requires | Validation |
|------|----------|------------|
| OpenAI connection | `OPENAI_API_KEY` | `providers.test` returns `success: true` |
| Invalid key | hardcoded bad key | `providers.test` returns `success: false` |

---

## Layer 3: Playwright E2E

### Directory Structure

```
e2e/
├── playwright.config.ts
├── global-setup.ts           # Start Aleph server process
├── global-teardown.ts        # Kill server process
├── helpers/
│   ├── rpc-client.ts         # WebSocket JSON-RPC helper
│   └── test-fixtures.ts      # Programmatic config injection via RPC
└── tests/
    ├── providers.spec.ts
    ├── embedding-providers.spec.ts
    ├── reranking-providers.spec.ts
    ├── generation-providers.spec.ts
    └── setup-wizard.spec.ts
```

### Infrastructure

**global-setup.ts:**
- Build WASM panel if needed (`just wasm`)
- Start Aleph server process on a fixed test port (e.g., 18791)
- Wait for HTTP health check (GET `/` returns 200)
- Store server PID for teardown

**global-teardown.ts:**
- Kill server process by stored PID

**rpc-client.ts:**
- WebSocket connection to `ws://127.0.0.1:18791/ws`
- `call(method: string, params: object): Promise<any>` — JSON-RPC request/response
- Auto-incrementing request IDs

**test-fixtures.ts:**
- `resetConfig()` — RPC call to reset config to minimal state
- `injectProvider(name, config)` — create a provider via RPC
- `getProviders()` — list providers via RPC for verification

**Each test's beforeEach:**
```typescript
test.beforeEach(async ({ page }) => {
  await resetConfig();
  // Inject test-specific data as needed
});
```

### Test Scenarios

#### 3.1 providers.spec.ts — AI Provider Settings

| Test | Steps | Assertions |
|------|-------|------------|
| Page loads | Navigate to providers settings | Page title visible, no JS errors |
| Display existing provider | Inject openai provider, navigate | List shows "openai", models input shows "gpt-4o" |
| Input multiple models | Type "gpt-4o, gpt-4o-mini, o1" in models field | Input value matches |
| Save and persist | Input models → save → reload page | After reload, models field still shows saved value |
| Edit provider | Change models from "a" to "b, c" → save | RPC confirms `models: ["b", "c"]` |
| Delete provider | Click delete → confirm dialog | Provider disappears from list |
| Empty models validation | Clear models field → save | Error message or save prevented |
| Long input (50 models) | Input 50 comma-separated names → save | All 50 stored, no truncation |
| Special characters | Input "org/model-v2.1" → save | Stored correctly without escaping issues |
| Switch default provider | Multiple providers, click set-default on non-default | UI reflects new default |
| Add new provider | Fill form (protocol, key, models) → save | New provider appears in list |

#### 3.2 embedding-providers.spec.ts

| Test | Steps | Assertions |
|------|-------|------------|
| Page loads | Navigate to embedding settings | Page renders correctly |
| Display existing provider | Inject siliconflow embedding provider | Models field shows "BAAI/bge-m3" |
| Edit models | Change to "BAAI/bge-m3, BAAI/bge-large-zh-v1.5" → save | Both models persisted |
| Add custom provider | Fill custom provider form → save | New provider in list |

#### 3.3 reranking-providers.spec.ts

| Test | Steps | Assertions |
|------|-------|------------|
| Page loads | Navigate to reranking settings | Page renders correctly |
| Display existing provider | Inject jina reranking provider | Models field shows model name |
| Edit models | Change to multi-model → save | Persisted correctly |
| No "Discover Models" button | Page loaded | Button does not exist (removed) |

#### 3.4 generation-providers.spec.ts

| Test | Steps | Assertions |
|------|-------|------------|
| Page loads | Navigate to generation settings | Page renders correctly |
| No hardcoded presets | Check model input area | Is text input, not dropdown |
| Input models | Type model name → save | Stored correctly |

#### 3.5 setup-wizard.spec.ts

| Test | Steps | Assertions |
|------|-------|------------|
| Full wizard flow | Select provider → enter key → enter model → finish | Config saved, wizard completes |
| Model input is text | On model step | Input is `<input type="text">`, not `<select>` |
| Multiple models in wizard | Enter "gpt-4o, gpt-4o-mini" → finish | Both models saved |
| Skip optional base_url | Leave base_url empty → finish | Wizard completes successfully |

---

## Test Execution

### CI Integration

```yaml
# Justfile additions
test-probes:
  cargo test --test provider_config_probe --test provider_rpc_probe

test-e2e:
  just wasm
  npx playwright test --project=chromium

test-real-api:
  cargo test --test provider_rpc_probe -- --ignored
```

### Local Development

```bash
# Layer 1 + 2 (fast, no browser needed)
just test-probes

# Layer 3 (needs browser, slower)
just test-e2e

# With real API keys (optional)
OPENAI_API_KEY=sk-... just test-real-api
```

### Dependencies to Add

**Rust (core/Cargo.toml dev-dependencies):**
- `tokio-tungstenite` — WebSocket client for RPC probes

**Node (root package.json):**
- `@playwright/test` — already have `playwright`, need the test runner

---

## Out of Scope

- Performance/load testing
- Visual regression testing (screenshot comparison)
- Mobile viewport testing
- Provider-specific behavior testing (Anthropic thinking, Gemini streaming, etc.)
- Testing the OpenAI-compatible `/v1/models` endpoint (separate concern)
