# Provider Config Testing Design

## Summary

Three-layer test pyramid for production-grade validation of the provider configuration architecture after the model→models migration. Layer 1 (Rust logic probes) tests serialization and config manipulation directly. Layer 2 (RPC integration probes) tests through a child-process server via WebSocket. Layer 3 (Playwright E2E) tests browser UI interactions using the same child-process server approach.

## Motivation

The recent simplify-model-config refactor touched 4 config types, ~70 call sites, deleted model discovery infrastructure, and rewrote all provider settings UIs. Current test coverage is limited to unit tests and serialization checks. No integration tests verify the RPC layer, and no e2e tests verify the frontend UI. This spec defines comprehensive test coverage to validate the entire stack.

## Architecture

```
┌─────────────────────────────┐
│  Layer 3: Playwright E2E    │  Browser interaction validation
│  (4 settings + wizard)      │  ↕ child-process server
├─────────────────────────────┤
│  Layer 2: RPC Integration   │  WebSocket JSON-RPC validation
│  (child-process + WS client)│  ↕ child-process server
├─────────────────────────────┤
│  Layer 1: Rust Logic Probes │  Pure logic, no I/O side effects
│  (serde / Config struct)    │  ↕ direct struct manipulation
└─────────────────────────────┘
```

### Test Data Strategy

Programmatic construction (builder pattern). No fixture files.

- Layer 1: direct `Config` struct building + serialization assertions
- Layers 2 & 3: server launched with a temp config in `TempDir`; state managed via `providers.create` / `providers.delete` / `providers.update` RPC calls

### External API Strategy

Split into mock and real:
- Default tests: fully isolated, no network required
- Optional `#[ignore]` tests: require `OPENAI_API_KEY` env var, test real API connectivity

### Server Startup Strategy (Layers 2 & 3)

Both Layer 2 and Layer 3 use a **child-process model**: spawn the real `aleph` binary with `--config <tempdir>/config.toml --port 0` (random port). This avoids the complexity of programmatically wiring up all server dependencies (token manager, session manager, extension manager, memory backend, etc.).

The child-process approach:
- Writes a programmatic config to a `TempDir`
- Spawns `cargo run --bin aleph -- start --config <path> --port <port>`
- Waits for HTTP health check
- Connects via WebSocket for RPC calls
- Kills the process on Drop/teardown

For Layer 2, each test group starts its own server instance. For Layer 3 (Playwright), the global-setup starts one server instance shared across all browser tests.

---

## Layer 1: Rust Logic Probes

**Directory:** `tests/provider_config_probe/` (follow existing `session_probe/` module pattern)

### Scope

Pure logic tests only — no handler calls, no file I/O, no server startup. Tests validate serialization, deserialization, struct manipulation, and config invariants directly on `Config` and `ProviderConfig` structs.

Mutating handler tests (create/update/delete) are deferred to Layer 2 because handlers call `save_config()` which writes to disk — testing them directly would require injecting a temp config path into the handler internals.

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

#### 1.3 Config Struct CRUD (no handlers, no I/O)

| Test | Operation | Validation |
|------|-----------|------------|
| Add provider to config | `config.providers.insert("openai", provider_config)` | config.providers contains "openai" |
| Multiple models in config | insert provider with `models: ["a","b","c"]` | all_models() returns 3 items |
| Remove provider from config | `config.providers.remove("openai")` | no longer in providers |
| Default provider resolution | set default_provider, get its default_model() | returns models[0] of default provider |
| Full Config round-trip | serialize to TOML string → deserialize back | all providers and models match |

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

**Directory:** `tests/provider_rpc_probe/` (follow `session_probe/` pattern)

### Test Harness

`AlephTestServer` spawns a child process:

```rust
struct AlephTestServer {
    child: std::process::Child,
    port: u16,
    ws_url: String,
    config_dir: TempDir,
}

impl AlephTestServer {
    async fn start() -> Self {
        let config_dir = TempDir::new().unwrap();
        // Write minimal config.toml to config_dir
        let port = find_available_port();
        let child = Command::new("cargo")
            .args(["run", "--bin", "aleph", "--", "start",
                   "--config", &config_dir.path().join("config.toml").display().to_string(),
                   "--port", &port.to_string()])
            .spawn().unwrap();
        // Wait for health check: GET http://127.0.0.1:{port}/
        Self { child, port, ws_url: format!("ws://127.0.0.1:{port}/ws"), config_dir }
    }

    async fn rpc_call(&self, method: &str, params: Value) -> Value {
        // Connect via tokio-tungstenite
        // Send JSON-RPC request, await response
    }
}

impl Drop for AlephTestServer {
    fn drop(&mut self) {
        self.child.kill().ok();
    }
}
```

### Scenarios

#### 2.1 RPC Endpoint Validation

| Test | Method | Validation |
|------|--------|------------|
| WebSocket connects | WS handshake | connection established |
| providers.list | RPC call | valid JSON response with models array |
| providers.create | create + get | created provider accessible with correct models |
| providers.update | update models | get returns new models |
| providers.delete | delete + list | provider removed |
| providers.setDefault | set default on verified provider | response confirms |
| providers.test (unreachable) | test with unreachable URL | returns `success: false` with error |

#### 2.2 Deleted Endpoints

Verify removed RPC methods return "method not found":

| Test | Method |
|------|--------|
| models.list | method not found |
| providers.probe | method not found |
| arbitrary.nonexistent | method not found |

(One or two representative examples suffice — no need to test every previously-deleted method individually.)

#### 2.3 Error Paths

| Test | Scenario | Expected |
|------|----------|----------|
| Update non-existent provider | `providers.update` with unknown name | error response |
| Delete non-existent provider | `providers.delete` with unknown name | error response |
| Create duplicate name | two `providers.create` same name | second fails |
| setDefault unverified | setDefault on unverified provider | error (requires verified) |

#### 2.4 Robustness

| Test | Scenario | Expected |
|------|----------|----------|
| Concurrent reads | 10 parallel providers.list | all succeed, no corruption |
| Large models list | provider with 100 models | serializes over WebSocket correctly |
| Invalid JSON | malformed RPC request | error response, no crash |

#### 2.5 Optional Real API Tests (`#[ignore]`)

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
├── global-setup.ts           # Start Aleph server child process
├── global-teardown.ts        # Kill server process
├── helpers/
│   ├── rpc-client.ts         # WebSocket JSON-RPC helper
│   └── test-fixtures.ts      # Config state management via RPC
└── tests/
    ├── providers.spec.ts
    ├── embedding-providers.spec.ts
    ├── reranking-providers.spec.ts
    ├── generation-providers.spec.ts
    └── setup-wizard.spec.ts
```

### Infrastructure

**global-setup.ts:**
- Expects WASM panel to be pre-built (CI pipeline runs `just wasm` as a separate step before e2e)
- Check for existing build: skip `just wasm` if `apps/panel/dist/aleph_panel_bg.wasm` exists
- Write a minimal test config to a temp directory
- Start Aleph server process: `cargo run --bin aleph -- start --config <tempdir>/config.toml --port 18791`
- Wait for HTTP health check (GET `http://127.0.0.1:18791/` returns 200)
- Store server PID and temp dir path for teardown

**global-teardown.ts:**
- Kill server process by stored PID
- Clean up temp directory

**rpc-client.ts:**
- WebSocket connection to `ws://127.0.0.1:18791/ws`
- `call(method: string, params: object): Promise<any>` — JSON-RPC request/response
- Auto-incrementing request IDs
- Connection pooling / reconnect logic

**test-fixtures.ts:**
Test state management via RPC (no `config.reset` endpoint needed):
- `cleanProviders()` — calls `providers.list`, then `providers.delete` for each existing provider
- `injectProvider(name, config)` — `providers.create` via RPC
- `getProviders()` — `providers.list` via RPC for verification

**Each test's beforeEach:**
```typescript
test.beforeEach(async ({ page }) => {
  await cleanProviders(); // delete all existing providers
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

```bash
# Justfile additions
test-probes:
  cargo test --test provider_config_probe --test provider_rpc_probe

test-e2e:
  npx playwright test --project=chromium

test-real-api:
  cargo test --test provider_rpc_probe -- --ignored
```

CI pipeline order:
1. `cargo build --bin aleph` (needed for child-process tests)
2. `just wasm` (build panel, needed for e2e)
3. `just test-probes` (Layer 1 + 2)
4. `just test-e2e` (Layer 3)

### Local Development

```bash
# Layer 1 only (fast, no server needed)
cargo test --test provider_config_probe

# Layer 1 + 2 (needs built binary)
just test-probes

# Layer 3 (needs browser + built binary + WASM panel)
just test-e2e

# With real API keys (optional)
OPENAI_API_KEY=sk-... just test-real-api
```

### Dependencies

**Rust (Cargo.toml dev-dependencies):**
- `tokio-tungstenite` — already present (v0.26), no action needed

**Node (root package.json):**
- `@playwright/test` — test runner (existing `playwright` dependency provides the browser binaries)

---

## Out of Scope

- Performance/load testing
- Visual regression testing (screenshot comparison)
- Mobile viewport testing
- Provider-specific behavior testing (Anthropic thinking, Gemini streaming, etc.)
- Testing the OpenAI-compatible `/v1/models` endpoint (separate concern)
- New RPC endpoints for test state management (use existing CRUD endpoints instead)
