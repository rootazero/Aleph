# Tool Service Façade — Phase 2 Design

**Date**: 2026-04-18
**Status**: Design approved — ready for implementation plan
**Parent**: [managed-agents-refactor-roadmap](./2026-04-18-managed-agents-refactor-roadmap.md) §7 Phase 2
**Scope**: Aleph Core — `src/tools/` new façade, `agent_loop/**` migration

---

## 1. Goal

Introduce a `ToolService` façade that unifies builtin / MCP / extension tool dispatch behind a single `execute(name, input) → Result<ToolOutput, ToolError>` surface. `agent_loop` depends only on `Arc<dyn ToolService>`, never reaches into tool sources directly. Policies (SmartFilter, ContextRule, approval gate, timeout, audit) become a decorator chain of `ToolService` implementations.

## 2. Non-Goals

- Not rewriting the author-side `AlephTool` trait; tool authors continue to implement it unchanged
- Not migrating Gateway `tools.*` RPC methods (they stay on `ToolServer` for now)
- Not changing the MCP or Extension runtime layers themselves — only wrapping them in `ToolHandler`
- Not introducing new subscription APIs on `ToolService` (`list()` is a snapshot; change notifications are YAGNI in v1)
- Not touching `SessionService` — Phase 2 keeps the tool and session façades decoupled

## 3. Decisions Locked (from brainstorming Q1–Q5)

| Axis | Choice | Rationale |
|------|--------|-----------|
| Façade shape | **`ToolService` as consumer-side trait; `AlephTool` stays as author-side** | Minimal change; 50+ existing tool impls unchanged; matches Anthropic "brain ignorant of source" |
| Return type | **`Result<ToolOutput, ToolError>`; reuse Phase 1's `ToolOutput`** | Aligns with `SessionEvent::ToolResult` / `ToolError`; idiomatic Rust |
| Middleware pattern | **Decorator chain — each layer is a `ToolService` wrapping an `inner: Arc<dyn ToolService>`** | Zero new traits; natural short-circuit; tower-style without tower types |
| Listing | **Flat `list()` snapshot, `ArcSwap`-backed registry** | LLM consumes flat list; hot-reload via atomic swap; no subscribe API in v1 |
| Event emission | **agent_loop emits `ToolCallRequested` / `ToolResult` / `ToolError`; ToolService does NOT depend on SessionService** | Two façades stay orthogonal; agent_loop already owns `session_id` and `turn_id` |
| Layer order | `Audit → Permission → ContextRule → Timeout → Core` | Denied calls short-circuit early; approval waits are NOT counted in tool timeout; latency measured at outermost layer |

## 4. Architecture

```
┌──────────────────────────────────────────────────┐
│ agent_loop (consumer)                             │
│                                                    │
│  tool_svc: Arc<dyn ToolService>   ← only dep      │
│  session_svc: Arc<dyn SessionService>             │
│                                                    │
│  helper: invoke_with_session_trace(...)           │
│    1. emit ToolCallRequested                       │
│    2. tool_svc.execute(name, input).await          │
│    3. emit ToolResult | ToolError | ToolCallDenied │
└────────────────────────┬─────────────────────────┘
                         │
              ┌──────────▼──────────┐
              │ ExecAuditLayer       │  tracing + latency_ms
              └──────────┬──────────┘
                         │
              ┌──────────▼──────────┐
              │ PermissionLayer      │  SmartFilter + ApprovalGate
              │                      │  (not timed)
              └──────────┬──────────┘
                         │
              ┌──────────▼──────────┐
              │ ContextRuleLayer     │  rewrite / deny by context
              └──────────┬──────────┘
                         │
              ┌──────────▼──────────┐
              │ TimeoutLayer         │  per-tool / default timeout
              └──────────┬──────────┘
                         │
              ┌──────────▼──────────┐
              │ CoreDispatch         │
              │                      │
              │  ToolRegistry:       │
              │    ArcSwap<HashMap<  │
              │      String,         │
              │      Arc<dyn         │
              │      ToolHandler>>>  │
              │                      │
              │  BuiltinHandler      │
              │  McpHandler          │
              │  ExtensionHandler    │
              └─────────────────────┘
```

**Invariants**:
- Every layer is itself a `ToolService` impl; consumers cannot distinguish
- `ToolService` never imports `SessionService` — the two façades are vertically orthogonal
- Registry mutations (MCP connect/disconnect, extension load/unload) are atomic `ArcSwap::store` operations; live dispatches continue against stable snapshots
- Name collisions across sources return `ToolError::Other` on register; never silent overwrite
- Author-side `AlephTool` / `AlephToolDyn` are preserved exactly; `BuiltinHandler` adapts them to the new interface

## 5. Public API — `ToolService` trait

```rust
#[async_trait::async_trait]
pub trait ToolService: Send + Sync + 'static {
    async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError>;

    async fn list(&self) -> Vec<ToolDefinition>;

    async fn describe(&self, name: &str) -> Option<ToolDefinition>;
}
```

**Notes**:
- `ToolOutput` is reused verbatim from `src/session/events.rs` (Phase 1)
- `list()` returns a snapshot; no `subscribe()` in v1
- `describe(name)` is a convenience that avoids callers scanning a full list
- No `register()` / `unregister()` on the trait — registration is constructor-time plus internal hot-reload wiring; callers never mutate the registry

### 5.1 `ToolDefinition` (source-agnostic)

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,         // JSON Schema
    pub source: ToolSource,
    pub metadata: ToolDefinitionMetadata,
}

pub enum ToolSource {
    Builtin,
    Mcp { server_id: String },
    Extension { plugin_id: String },
}

pub struct ToolDefinitionMetadata {
    pub hidden_from_llm: bool,
    pub requires_approval: bool,
    pub tags: Vec<String>,
}
```

## 6. `ToolError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {name}")]
    NotFound { name: String },

    #[error("permission denied for tool {name}: {reason}")]
    PermissionDenied { name: String, reason: String },

    #[error("invalid input for tool {name}: {cause}")]
    ValidationFailed { name: String, cause: String },

    #[error("tool {name} execution failed: {cause}")]
    Execution { name: String, cause: String },

    #[error("tool {name} timed out after {elapsed_ms}ms")]
    Timeout { name: String, elapsed_ms: u64 },

    #[error("tool {name} transport error: {cause}")]
    Transport { name: String, cause: String },

    #[error("{0}")]
    Other(String),
}

impl ToolError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::Transport { .. })
    }
}
```

**Event mapping**:
- `Err(ToolError::PermissionDenied)` → `SessionEvent::ToolCallDenied { reason }`
- `Err(_)` (other variants) → `SessionEvent::ToolError { error }`
- `Ok(output)` → `SessionEvent::ToolResult { output }`

Internal-path and PID leaks are scrubbed at `thiserror::Display` time (keep messages LLM-safe per the project's security rules).

## 7. Decorator Chain (Middleware Layers)

### 7.1 `CoreDispatch` (bottom of chain)

```rust
pub struct CoreDispatch {
    registry: Arc<ToolRegistry>,
}

#[async_trait::async_trait]
impl ToolService for CoreDispatch {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let snapshot = self.registry.snapshot();
        let handler = snapshot.get(name).ok_or_else(|| ToolError::NotFound { name: name.into() })?;
        handler.invoke(input).await
    }
    // list() and describe() implemented via registry snapshot
}
```

`ToolHandler` is the source-agnostic internal trait:

```rust
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn invoke(&self, input: serde_json::Value) -> Result<ToolOutput, ToolError>;
    fn definition(&self) -> ToolDefinition;
}
```

Three implementations live under `src/tools/handlers/`:

- **`BuiltinHandler`** — wraps `Arc<dyn AlephToolDyn>`; calls existing `AlephTool::call()` and adapts the result
- **`McpHandler`** — holds `Arc<McpClient>` + `server_id` + `tool_name`; forwards to MCP `tools/call`
- **`ExtensionHandler`** — holds `plugin_id` + runtime ref; dispatches through ExtensionRuntime

### 7.2 `TimeoutLayer`

```rust
pub struct TimeoutLayer {
    inner: Arc<dyn ToolService>,
    default_timeout: Duration,
    per_tool_override: HashMap<String, Duration>,
}
```

Wraps `inner.execute(...)` in `tokio::time::timeout`. On elapse, returns `ToolError::Timeout { elapsed_ms }`. Per-tool overrides come from `ToolServiceConfig`.

### 7.3 `ContextRuleLayer`

```rust
pub struct ContextRuleLayer {
    inner: Arc<dyn ToolService>,
    rules: Arc<ArcSwap<Vec<ContextRule>>>,
}
```

Port of the existing `ContextRule` logic from `agent_loop/` to this layer. Rules can rewrite input or deny outright (`ToolError::PermissionDenied`). `ArcSwap` lets config reload swap the rule list atomically.

### 7.4 `PermissionLayer`

```rust
pub struct PermissionLayer {
    inner: Arc<dyn ToolService>,
    smart_filter: Arc<SmartFilter>,
    approval_gate: Arc<ApprovalGate>,  // preserved at src/agent_loop/exec_approval/
}
```

Flow:
1. `smart_filter.classify(name)` → `Allow` / `Confirm` / `Deny`
2. `Deny` → `Err(ToolError::PermissionDenied)`
3. `Confirm` → `approval_gate.ask(name, &input).await`; if denied, `Err(PermissionDenied)`
4. `Allow` → `inner.execute(...).await`

`ApprovalGate` is **outside** `TimeoutLayer` by construction — user waiting time does not count against tool execution time.

### 7.5 `ExecAuditLayer` (outermost)

```rust
pub struct ExecAuditLayer {
    inner: Arc<dyn ToolService>,
}
```

Measures latency from the outermost boundary, stamps `ToolOutput.metadata.latency_ms`, emits `tracing::info!` spans for tool call start/end. Does **not** emit `SessionEvent`s — that's the caller's responsibility (agent_loop).

### 7.6 Assembly

```rust
pub fn build_tool_service(
    server: Arc<ToolServer>,
    smart_filter: Arc<SmartFilter>,
    approval: Arc<ApprovalGate>,
    rules: Arc<ArcSwap<Vec<ContextRule>>>,
    config: &ToolServiceConfig,
) -> Arc<dyn ToolService> {
    let registry = Arc::new(ToolRegistry::new());
    // Builtin tools registered synchronously from server
    register_builtins(&registry, &server);
    // MCP + extension register asynchronously as connections arrive

    let core    = Arc::new(CoreDispatch::new(registry));
    let timeout = Arc::new(TimeoutLayer::new(core,     config.default_timeout, config.per_tool.clone()));
    let ctxrule = Arc::new(ContextRuleLayer::new(timeout, rules));
    let perm    = Arc::new(PermissionLayer::new(ctxrule,  smart_filter, approval));
    let audit   = Arc::new(ExecAuditLayer::new(perm));
    audit
}
```

## 8. Registry model & hot-reload

```rust
pub struct ToolRegistry {
    inner: Arc<ArcSwap<HashMap<String, Arc<dyn ToolHandler>>>>,
    change_tx: broadcast::Sender<RegistryChange>,  // internal, not exposed on ToolService
}

pub enum RegistryChange {
    Registered   { name: String, source: ToolSource },
    Unregistered { name: String, source: ToolSource },
}

impl ToolRegistry {
    pub fn register(&self, name: String, handler: Arc<dyn ToolHandler>) -> Result<(), ToolError>;
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ToolHandler>>;
    pub fn snapshot(&self) -> Arc<HashMap<String, Arc<dyn ToolHandler>>>;
}
```

### 8.1 Source flow

- **Builtin** — registered at boot from `src/executor/builtin_registry/`; static set, never mutates at runtime
- **MCP** — `McpClientManager::connect(server)` loads `tools/list` → wraps each in `McpHandler` → `registry.register("{server_id}__{tool}", handler)`; `disconnect` iterates + `unregister`
- **Extension** — plugin load registers declared tools as `ExtensionHandler`; unload reverses

### 8.2 Naming & collisions

- Builtin tools keep short names (first-class citizens)
- MCP tools registered as `{server_id}__{tool_name}`
- Extension tools registered as `ext__{plugin_id}__{tool_name}`
- Duplicate `register(name, _)` returns `ToolError::Other("duplicate tool name: ...")` — never silently overwrite

### 8.3 Writes under concurrency

- `register` / `unregister` clone the current map, mutate, and `ArcSwap::store(Arc::new(new_map))`
- Readers always `snapshot()` (lock-free) and operate against a stable `Arc<HashMap<_,_>>`
- In-flight `execute()` continues on its snapshot even if the registry gets swapped mid-call

## 9. Migration Strategy (Strangler)

Each step is independently shippable; `cargo test` must be green at every step.

### 9.1 Types scaffold
New files only: `src/tools/service.rs`, `src/tools/handlers/{mod,builtin,mcp,extension}.rs`, `src/tools/registry.rs`, `src/tools/middleware/{mod,timeout,context_rule,permission,audit}.rs`, `src/tools/dispatch.rs`. Register module in `src/tools/mod.rs` or a new `src/tools/service_facade.rs` re-export. `cargo check` green; no runtime wiring.

### 9.2 `CoreDispatch` + `BuiltinHandler`
Implement `CoreDispatch` + `BuiltinHandler`. On boot, register all builtin tools from `ToolServer` into the new `ToolRegistry`. Unit test: `execute("memory_search", {...})` returns correct output.

### 9.3 `McpHandler` + `ExtensionHandler`
Wire `McpClientManager` to call `registry.register` on tool discovery, `unregister` on disconnect. Same for `ExtensionLoader`. Unit tests with mock MCP/extension runtimes.

### 9.4 Middleware layers (shells)
Implement all five decorator layers as forwarding shells (call `inner.execute` without policy). This establishes the composition; real policy is grafted in the next step.

### 9.5 Migrate `SmartFilter` and `ContextRule` into layers
Move `SmartFilter` logic to `src/tools/middleware/permission.rs`; `ContextRule` to `src/tools/middleware/context_rule.rs`. `ApprovalGate` stays at `src/agent_loop/exec_approval/` — `PermissionLayer` holds `Arc<ApprovalGate>` to reuse it without relocation. Behavior parity tests.

### 9.6 `ToolServiceConfig` + AppContext assembly
Add `ToolServiceConfig { default_timeout, per_tool: HashMap<String, Duration> }`. Extend `src/bin/aleph-server/commands/start/builder/` to construct the full chain and inject `Arc<dyn ToolService>` into AppContext.

### 9.7 Agent_loop migration
Change `agent_loop/tool_pipeline.rs` + `tool_orchestrator.rs` to consume `Arc<dyn ToolService>`. Remove direct imports of `McpClient`, `BuiltinToolRegistry`, `ExtensionTool`. Add helper `invoke_with_session_trace(...)` that emits `SessionEvent::ToolCallRequested` → calls `tool_svc.execute` → emits `ToolResult` / `ToolError` / `ToolCallDenied` per result. One migration site per commit.

### 9.8 Documentation + CHANGELOG
Update `docs/reference/TOOL_SYSTEM.md` to describe the façade. Update `docs/reference/GLOSSARY.md` — flip the "Tools" entry from future-tense to present. Add CHANGELOG entry.

### 9.9 Exit gate
- `grep -rn 'McpClient\|BuiltinToolRegistry\|ExtensionTool' src/agent_loop/` → zero hits
- `grep -rn 'SmartFilter\|ContextRule' src/agent_loop/` → zero hits (both moved to `src/tools/middleware/`)
- All existing tool-related tests green; `just test-all` matches baseline (8982 passed / 2 pre-existing failures)
- MCP hot-reload integration test green

## 10. Testing Strategy

### Unit tests (by module)

| Module | Cases |
|--------|-------|
| `ToolRegistry` | register/unregister/snapshot; concurrent registers; duplicate-name rejection |
| `CoreDispatch` | correct routing; `NotFound` error; `list()` snapshot |
| `BuiltinHandler` | wraps `AlephToolDyn` correctly |
| `McpHandler` | forwards to `tools/call`; maps MCP `isError` → `Execution`; connection loss → `Transport` |
| `ExtensionHandler` | same for extension runtime (mocked) |
| `TimeoutLayer` | timeout triggers `ToolError::Timeout`; per-tool override works |
| `ContextRuleLayer` | rule rewrites input; rule denies → `PermissionDenied` |
| `PermissionLayer` | always_allow bypasses approval; never_allow → `PermissionDenied`; require_confirmation → approval gate called; approval denial → `PermissionDenied` |
| `ExecAuditLayer` | latency_ms stamped on success; error path also stamped |
| Layer composition | denied at Permission never reaches Core; denied at ContextRule doesn't hit Timeout |

### Integration tests

1. **Full dispatch end-to-end**: construct complete chain with 3 tools (builtin + mock MCP + mock extension); execute each; verify
2. **Hot-reload**: `list()` before connect → N tools; connect mock MCP server → `list()` → N+k tools; disconnect → `list()` → N
3. **SessionEvent emission via helper**: `invoke_with_session_trace` wires ToolService + SessionService; inspect `session_events` table for expected sequence
4. **SmartFilter config loading**: load real `aleph.toml` filter section; verify runtime behavior matches config
5. **Timeout + approval cooperation**: approval gate takes 30s to respond, tool timeout is 10s; verify approval wait is not counted against tool timeout

### Regression

- All agent_loop tool-call integration tests remain green
- Gateway `tools.*` RPC behavior unchanged
- All existing builtin / MCP / extension tool tests pass

### Performance baselines (non-blocking)

- Single `execute("noop", {})` through 5-layer chain: < 200µs overhead
- `list()` with 1000 tools: < 5ms
- Concurrent `snapshot()` × 1000 with interleaved register/unregister: deadlock-free, eventually consistent

## 11. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| ContextRule behavior change when relocated from `agent_loop/` | Medium | High | Parity tests pre/post relocation; keep logic byte-identical; reviewer spot-check |
| MCP hot-reload race between `tools/list` fetch and `ArcSwap::store` | Medium | Medium | Integration test registers + executes concurrently; single-writer discipline through `McpClientManager` |
| `AlephTool` adapter boundary introduces subtle input-type mismatches | Low | High | `BuiltinHandler` uses same `serde_json::Value` pipeline existing tools already accept |
| Decorator chain overhead is measurable on hot loops | Low | Low | Baseline test; each layer is one async function + one `await`; if measured overhead matters, inline in `release` profile |
| `ToolServiceConfig` per-tool timeouts bloat user config | Low | Low | Default-driven; only opt-in per-tool; document convention |
| `PermissionLayer` and agent_loop both emit denial messages, double-logging | Medium | Low | Single emission point: agent_loop helper maps `Err(PermissionDenied)` → `ToolCallDenied`; Permission layer does not emit itself |
| Naming convention collision between MCP `{server_id}__{name}` and extension `ext__{plugin_id}__{name}` | Low | Medium | Prefixes are non-overlapping; duplicate-check at register enforces the invariant |

## 12. Open Questions (deferred to implementation)

- Exact concurrent-registration ordering semantics — does agent_loop ever call `list()` mid-register, and is it OK to see a partial set? Likely yes, but verify with the integration test from §10.
- Where to house `ToolServiceConfig` — alongside `AcpConfig` under `src/config/types/`, or in its own file? Decide during §9.6.
- Whether `BuiltinHandler` needs a compile-time-typed variant for perf-critical tools; probably not in Phase 2 (dyn dispatch overhead is sub-microsecond).
- Whether `ExecAuditLayer` should also fire OpenTelemetry spans or just `tracing`; stick with `tracing` for v1 unless downstream consumers exist.

## 13. Success Metrics

- `agent_loop` has zero direct imports of `McpClient`, `BuiltinToolRegistry`, `ExtensionTool` after Phase 2
- `SmartFilter` and `ContextRule` relocated to `src/tools/middleware/`; zero hits in `src/agent_loop/`
- `MCP hot-reload` integration test green
- Five middleware layers individually unit-testable
- Zero regression on baseline `cargo test` (8982 passed / 2 pre-existing failures)
- Daily release cadence uninterrupted throughout Phase 2

## 14. Next Action

1. User reviews this spec
2. On approval → invoke `writing-plans` skill to produce a task-level implementation plan
3. Implementation executes via subagent-driven-development, matching the Phase 0 / Phase 1 pattern
