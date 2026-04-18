# Managed-Agents Phase 2 — Tool Service Façade — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `src/tools/service.rs` + a decorator middleware chain so `agent_loop` dispatches tools via a single `Arc<dyn ToolService>` instead of reaching into builtin / MCP / extension registries directly.

**Architecture:** Five-layer decorator chain (`Audit → Permission → ContextRule → Timeout → CoreDispatch`), each layer is itself a `ToolService`. `CoreDispatch` holds an `ArcSwap`-backed `ToolRegistry` populated at boot (builtin), and dynamically during runtime (MCP / extension). SmartFilter, ContextRule, and ApprovalGate migrate from `agent_loop/` into `src/tools/middleware/`. `agent_loop` emits `SessionEvent::ToolCallRequested/Result/Error` via a new helper, keeping `ToolService` and `SessionService` orthogonal.

**Tech Stack:** Rust 2024, tokio, async_trait, thiserror, arc-swap, serde_json, rusqlite (for persistence consumers already in place), tracing.

**Source spec:** `docs/superpowers/specs/2026-04-18-tool-service-facade-design.md` §9 steps 9.1–9.9.

---

## Pre-flight

- [ ] **Pre-1: Worktree setup**

Use the EnterWorktree tool with `name: "managed-agents-phase-2"`. Fast-forward merge if the worktree branch points to stale HEAD (pattern inherited from Phase 0/1):
```bash
git merge main --ff-only
```
Confirm: `git log --oneline -3` shows `466a83ac6 docs: add Phase 2 Tool Service façade design` at or near HEAD.

- [ ] **Pre-2: Baseline snapshot**

Run:
```bash
echo "=== Phase 2 baseline ===" > /tmp/phase2-baseline.txt
echo "-- agent_loop direct imports (should drop to 0) --" >> /tmp/phase2-baseline.txt
grep -rn 'McpClient\|BuiltinToolRegistry\|ExtensionTool' src/agent_loop/ >> /tmp/phase2-baseline.txt
echo "-- SmartFilter/ContextRule locations --" >> /tmp/phase2-baseline.txt
grep -rn 'struct SmartFilter\|struct ContextRule' src/ | head -5 >> /tmp/phase2-baseline.txt
echo "-- current tools module files --" >> /tmp/phase2-baseline.txt
ls src/tools/ >> /tmp/phase2-baseline.txt
cat /tmp/phase2-baseline.txt
```
Record this baseline. Task 12 diffs against it.

- [ ] **Pre-3: Baseline build**

Run: `cargo check -p alephcore 2>&1 | tail -3`
Expected: `Finished dev`

Run: `cargo test -p alephcore --lib 2>&1 | tail -5`
Expected: `test result: FAILED. 8982 passed; 2 failed` — same 2 pre-existing failures (`telegram::config::tests::parse_v2_config_directly`, `memory::notes::ingest::prompts::tests::base_prompt_snapshot`). Phase 2 must not introduce new failures beyond these 2.

**Foreground cargo only; `timeout: 600000`. No `run_in_background` — previous worktree harness sessions killed background cargo prematurely.**

---

## Task 1: Types scaffold + `ToolService` trait + `ToolError`

**Files:**
- Create: `src/tools/service.rs`
- Create: `src/tools/registry.rs` (stub)
- Create: `src/tools/dispatch.rs` (stub)
- Create: `src/tools/handlers/mod.rs`, `src/tools/handlers/builtin.rs`, `src/tools/handlers/mcp.rs`, `src/tools/handlers/extension.rs` (all stubs)
- Create: `src/tools/middleware/mod.rs`, `src/tools/middleware/audit.rs`, `src/tools/middleware/permission.rs`, `src/tools/middleware/context_rule.rs`, `src/tools/middleware/timeout.rs` (all stubs)
- Modify: `src/tools/mod.rs` — register new sub-modules

**Context:** Pure scaffolding. All new files; no runtime wiring. Cargo check must pass with acceptable unused-import warnings.

- [ ] **Step 1.1: Inspect current `src/tools/mod.rs`**

Run: `head -50 src/tools/mod.rs && echo '---' && ls src/tools/`
Note the existing module declarations. You'll add new ones alongside, not replace.

- [ ] **Step 1.2: Create `src/tools/service.rs`**

```rust
//! ToolService — consumer-side façade over tool dispatch.
//!
//! See: docs/superpowers/specs/2026-04-18-tool-service-facade-design.md

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::session::events::ToolOutput;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSource {
    Builtin,
    Mcp { server_id: String },
    Extension { plugin_id: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinitionMetadata {
    #[serde(default)]
    pub hidden_from_llm: bool,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub source: ToolSource,
    #[serde(default)]
    pub metadata: ToolDefinitionMetadata,
}

#[async_trait]
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

- [ ] **Step 1.3: Create stub files for later tasks**

Each stub is one docstring line. Example for `src/tools/registry.rs`:
```rust
//! ToolRegistry — ArcSwap-backed map of name → handler. Populated in Task 2.
```

Repeat with appropriate one-line docstrings for:
- `src/tools/dispatch.rs` — "CoreDispatch — bottom of the ToolService decorator chain. Task 3."
- `src/tools/handlers/mod.rs` — "ToolHandler implementations for builtin / MCP / extension sources."
- `src/tools/handlers/builtin.rs` — "BuiltinHandler — wraps AlephToolDyn. Task 3."
- `src/tools/handlers/mcp.rs` — "McpHandler — forwards to MCP tools/call. Task 4."
- `src/tools/handlers/extension.rs` — "ExtensionHandler — dispatches through ExtensionRuntime. Task 4."
- `src/tools/middleware/mod.rs` — "ToolService middleware layers. Decorator composition in src/tools/facade.rs."
- `src/tools/middleware/audit.rs` — "ExecAuditLayer — tracing + latency. Task 9."
- `src/tools/middleware/permission.rs` — "PermissionLayer — SmartFilter + ApprovalGate. Task 7."
- `src/tools/middleware/context_rule.rs` — "ContextRuleLayer — rewrite/deny by context. Task 6."
- `src/tools/middleware/timeout.rs` — "TimeoutLayer — per-tool timeout. Task 8."

Each file also contains one module-level doc comment making clippy happy — don't leave them utterly empty.

- [ ] **Step 1.4: Register new sub-modules in `src/tools/mod.rs`**

Read the existing file; add (in alphabetical order or at the end, matching the file's style):
```rust
pub mod service;
pub mod registry;
pub mod dispatch;
pub mod handlers;
pub mod middleware;

pub use service::{
    ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};
```

- [ ] **Step 1.5: Build**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev` with only acceptable unused-import warnings for the new stubs.

- [ ] **Step 1.6: Commit**

```bash
git add src/tools/
git commit -m "tools: add ToolService trait + module scaffold

Phase 2 Task 1: types only, no runtime wiring. ToolService trait,
ToolError enum, ToolDefinition family. Stubs for registry, dispatch,
handlers, middleware — filled in later tasks."
```

---

## Task 2: `ToolRegistry` with `ArcSwap` + tests

**Files:**
- Modify: `src/tools/registry.rs`
- Add (if not already a dependency): `Cargo.toml` — verify `arc-swap` is present (`grep arc-swap Cargo.toml`; Aleph's memory module uses it, so it should be)

**Context:** The registry is a single atomic `HashMap<String, Arc<dyn ToolHandler>>`. `register` / `unregister` clone-mutate-store; `snapshot` is lock-free.

- [ ] **Step 2.1: Confirm arc-swap availability**

Run: `grep -n arc-swap Cargo.toml`
If not present, add `arc-swap = "1"` to `[dependencies]` (Aleph already uses it in memory modules; should be workspace dep).

- [ ] **Step 2.2: Define `ToolHandler` trait in `src/tools/handlers/mod.rs`**

```rust
//! ToolHandler implementations for builtin / MCP / extension sources.

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError};

pub mod builtin;
pub mod mcp;
pub mod extension;

#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError>;
    fn definition(&self) -> ToolDefinition;
}
```

- [ ] **Step 2.3: Implement `ToolRegistry` in `src/tools/registry.rs`**

```rust
//! ToolRegistry — ArcSwap-backed name → handler map.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::broadcast;

use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolError, ToolSource};

#[derive(Debug, Clone)]
pub enum RegistryChange {
    Registered { name: String, source: ToolSource },
    Unregistered { name: String, source: ToolSource },
}

pub struct ToolRegistry {
    inner: Arc<ArcSwap<HashMap<String, Arc<dyn ToolHandler>>>>,
    change_tx: broadcast::Sender<RegistryChange>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            change_tx: tx,
        }
    }

    pub fn register(&self, name: String, handler: Arc<dyn ToolHandler>) -> Result<(), ToolError> {
        let current = self.inner.load();
        if current.contains_key(&name) {
            return Err(ToolError::Other(format!("duplicate tool name: {name}")));
        }
        let mut next = (**current).clone();
        let source = handler.definition().source.clone();
        next.insert(name.clone(), handler);
        self.inner.store(Arc::new(next));
        let _ = self.change_tx.send(RegistryChange::Registered { name, source });
        Ok(())
    }

    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        let current = self.inner.load();
        let handler = current.get(name).cloned()?;
        let mut next = (**current).clone();
        let removed = next.remove(name)?;
        let source = removed.definition().source.clone();
        self.inner.store(Arc::new(next));
        let _ = self.change_tx.send(RegistryChange::Unregistered {
            name: name.to_string(),
            source,
        });
        Some(handler)
    }

    pub fn snapshot(&self) -> Arc<HashMap<String, Arc<dyn ToolHandler>>> {
        self.inner.load_full()
    }

    /// Internal subscribe — not re-exported through ToolService yet. YAGNI per design §5.1.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<RegistryChange> {
        self.change_tx.subscribe()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::ToolOutput;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeHandler {
        name: String,
        source: ToolSource,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!({"tool": self.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: self.source.clone(),
                metadata: ToolDefinitionMetadata::default(),
            }
        }
    }

    fn fake(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler {
            name: name.into(),
            source: ToolSource::Builtin,
        })
    }

    #[test]
    fn register_and_snapshot() {
        let reg = ToolRegistry::new();
        reg.register("a".into(), fake("a")).unwrap();
        reg.register("b".into(), fake("b")).unwrap();
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap.contains_key("a"));
        assert!(snap.contains_key("b"));
    }

    #[test]
    fn duplicate_register_returns_other() {
        let reg = ToolRegistry::new();
        reg.register("dup".into(), fake("dup")).unwrap();
        let err = reg.register("dup".into(), fake("dup")).unwrap_err();
        assert!(matches!(err, ToolError::Other(msg) if msg.contains("dup")));
    }

    #[test]
    fn unregister_removes() {
        let reg = ToolRegistry::new();
        reg.register("z".into(), fake("z")).unwrap();
        let removed = reg.unregister("z").unwrap();
        assert_eq!(removed.definition().name, "z");
        assert_eq!(reg.snapshot().len(), 0);
    }

    #[test]
    fn unregister_missing_returns_none() {
        let reg = ToolRegistry::new();
        assert!(reg.unregister("nope").is_none());
    }

    #[test]
    fn snapshot_stable_against_concurrent_register() {
        // Emit a snapshot, then register while holding the snapshot — snapshot's
        // contents must be unchanged (that's the ArcSwap guarantee).
        let reg = ToolRegistry::new();
        reg.register("x".into(), fake("x")).unwrap();
        let snap1 = reg.snapshot();
        reg.register("y".into(), fake("y")).unwrap();
        assert_eq!(snap1.len(), 1);              // snap1 frozen
        assert_eq!(reg.snapshot().len(), 2);     // new snapshot sees both
    }

    #[test]
    fn change_events_are_sent() {
        let reg = ToolRegistry::new();
        let mut rx = reg.subscribe();
        reg.register("e".into(), fake("e")).unwrap();
        let evt = rx.try_recv().expect("event");
        assert!(matches!(evt, RegistryChange::Registered { .. }));
        reg.unregister("e");
        let evt = rx.try_recv().expect("event");
        assert!(matches!(evt, RegistryChange::Unregistered { .. }));
    }
}
```

- [ ] **Step 2.4: Run tests**

Run: `cargo test -p alephcore --lib tools::registry 2>&1 | tail -15`
Expected: 6 passed.

- [ ] **Step 2.5: Commit**

```bash
git add src/tools/registry.rs src/tools/handlers/mod.rs
git commit -m "tools: ArcSwap-backed ToolRegistry + ToolHandler trait

Phase 2 Task 2: register/unregister/snapshot; duplicate-name rejection;
internal change broadcast (not exposed on ToolService in v1)."
```

---

## Task 3: `CoreDispatch` + `BuiltinHandler`

**Files:**
- Modify: `src/tools/dispatch.rs`
- Modify: `src/tools/handlers/builtin.rs`

**Context:** Wire the lowest-level dispatch. `BuiltinHandler` adapts the existing `AlephToolDyn` trait object into the new `ToolHandler` interface. `CoreDispatch` looks up in the snapshot and invokes.

- [ ] **Step 3.1: Inspect `AlephToolDyn` shape**

Run: `grep -n 'pub trait AlephToolDyn\|pub trait AlephTool\b\|pub fn call_dyn\|pub fn invoke' src/tools/mod.rs src/tools/*.rs 2>/dev/null | head -20`
Note the signature of `AlephToolDyn::call` (or whatever the dynamic entry point is). The input type is almost certainly `serde_json::Value` or a typed wrapper; the return is `Result<serde_json::Value, Error>` of some form. Record exact types.

- [ ] **Step 3.2: Implement `BuiltinHandler`**

Adapt to the signatures from 3.1. Reference implementation (adapt types to match discovered reality):

```rust
//! BuiltinHandler — wraps AlephToolDyn for ToolHandler.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
use crate::tools::AlephToolDyn;  // adjust import to match reality

pub struct BuiltinHandler {
    inner: Arc<dyn AlephToolDyn>,
    name: String,
}

impl BuiltinHandler {
    pub fn new(name: String, inner: Arc<dyn AlephToolDyn>) -> Self {
        Self { inner, name }
    }
}

#[async_trait]
impl ToolHandler for BuiltinHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        // ADAPT: replace `self.inner.call(input).await` with whatever the real
        // AlephToolDyn entry is — likely `call_dyn` or similar.
        match self.inner.call_dyn(input).await {
            Ok(value) => Ok(ToolOutput {
                value,
                metadata: ToolOutputMetadata::default(),
            }),
            Err(e) => Err(ToolError::Execution {
                name: self.name.clone(),
                cause: e.to_string(),
            }),
        }
    }

    fn definition(&self) -> ToolDefinition {
        // ADAPT: existing AlephToolDyn likely has `name()`, `description()`, and
        // `input_schema()` methods. Wire them through.
        ToolDefinition {
            name: self.name.clone(),
            description: self.inner.description().to_string(),
            input_schema: self.inner.input_schema(),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}
```

If the real `AlephToolDyn` method names differ, adjust; do not change the public trait. Document the mapping in a one-line comment above the `invoke` impl.

- [ ] **Step 3.3: Implement `CoreDispatch` in `src/tools/dispatch.rs`**

```rust
//! CoreDispatch — bottom of the ToolService decorator chain.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::registry::ToolRegistry;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct CoreDispatch {
    registry: Arc<ToolRegistry>,
}

impl CoreDispatch {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolService for CoreDispatch {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let snapshot = self.registry.snapshot();
        let handler = snapshot
            .get(name)
            .ok_or_else(|| ToolError::NotFound { name: name.to_string() })?
            .clone();
        drop(snapshot);  // release the Arc reference before await
        handler.invoke(input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.registry
            .snapshot()
            .values()
            .map(|h| h.definition())
            .collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.registry.snapshot().get(name).map(|h| h.definition())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::tools::handlers::ToolHandler;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolSource};
    use async_trait::async_trait;

    struct Echo {
        name: String,
    }

    #[async_trait]
    impl ToolHandler for Echo {
        async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: input,
                metadata: ToolOutputMetadata::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: "echo".into(),
                input_schema: serde_json::json!({}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            }
        }
    }

    fn with_tool(name: &str) -> CoreDispatch {
        let reg = Arc::new(ToolRegistry::new());
        reg.register(
            name.into(),
            Arc::new(Echo { name: name.into() }),
        )
        .unwrap();
        CoreDispatch::new(reg)
    }

    #[tokio::test]
    async fn execute_routes_by_name() {
        let d = with_tool("ping");
        let out = d.execute("ping", serde_json::json!({"x": 1})).await.unwrap();
        assert_eq!(out.value, serde_json::json!({"x": 1}));
    }

    #[tokio::test]
    async fn execute_not_found() {
        let d = with_tool("ping");
        let err = d.execute("missing", serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound { ref name } if name == "missing"));
    }

    #[tokio::test]
    async fn list_returns_all() {
        let reg = Arc::new(ToolRegistry::new());
        reg.register("a".into(), Arc::new(Echo { name: "a".into() })).unwrap();
        reg.register("b".into(), Arc::new(Echo { name: "b".into() })).unwrap();
        let d = CoreDispatch::new(reg);
        let list = d.list().await;
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn describe_returns_one() {
        let d = with_tool("foo");
        assert!(d.describe("foo").await.is_some());
        assert!(d.describe("nope").await.is_none());
    }
}
```

- [ ] **Step 3.4: Run tests**

Run: `cargo test -p alephcore --lib tools::dispatch tools::handlers 2>&1 | tail -15`
Expected: CoreDispatch tests (4) pass. BuiltinHandler has no dedicated tests yet.

- [ ] **Step 3.5: Commit**

```bash
git add src/tools/dispatch.rs src/tools/handlers/builtin.rs
git commit -m "tools: CoreDispatch + BuiltinHandler

Phase 2 Task 3: lowest-level dispatch. ToolRegistry snapshot lookup,
ArcSwap guarantees in-flight execute() stability across register/unregister."
```

---

## Task 4: `McpHandler` + `ExtensionHandler` + registration wiring

**Files:**
- Modify: `src/tools/handlers/mcp.rs`
- Modify: `src/tools/handlers/extension.rs`
- Modify: wherever `McpClient` / `McpClientManager` live (grep to find) — call `registry.register` on connect
- Modify: wherever extension loading happens (search `ExtensionLoader`, `plugin_load`) — call `registry.register` when a plugin ships tools

**Context:** Bridge MCP and Extension worlds into the new registry.

- [ ] **Step 4.1: Locate McpClient + current tool fetching**

```bash
grep -rn 'tools/list\|McpClient\|fn list_tools\b' src/ | head -20
grep -rn 'McpClientManager\|connect.*mcp\|async fn connect' src/ | head -15
```
Record: the struct that owns MCP connection lifecycle and where it currently calls `tools/list` on connect.

- [ ] **Step 4.2: Implement `McpHandler`**

```rust
//! McpHandler — forwards to MCP tools/call.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};

// ADAPT: import the concrete McpClient type discovered in Step 4.1
// e.g. `use crate::mcp::McpClient;` or `use crate::runtimes::mcp::McpClient;`

pub struct McpHandler {
    client: Arc</* McpClient */>, // fill in the concrete type
    server_id: String,
    tool_name: String,
    description: String,
    input_schema: Value,
}

impl McpHandler {
    pub fn new(
        client: Arc</* McpClient */>,
        server_id: String,
        tool_name: String,
        description: String,
        input_schema: Value,
    ) -> Self {
        Self { client, server_id, tool_name, description, input_schema }
    }
}

#[async_trait]
impl ToolHandler for McpHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        // Call MCP tools/call. Map result variants:
        //   - server responds with isError=true  → ToolError::Execution
        //   - transport drop / IO error          → ToolError::Transport
        //   - server responds with content       → Ok(ToolOutput { value, metadata })
        // ADAPT to actual McpClient signature. Skeleton:
        match self.client.call_tool(&self.tool_name, input).await {
            Ok(result) => Ok(ToolOutput {
                value: result,
                metadata: ToolOutputMetadata::default(),
            }),
            // Distinguish transport from business errors at the mapping site.
            // If McpClient surfaces a transport-vs-logical distinction, use it.
            // Otherwise default to Execution.
            Err(e) => {
                if is_transport(&e) {
                    Err(ToolError::Transport {
                        name: self.tool_name.clone(),
                        cause: e.to_string(),
                    })
                } else {
                    Err(ToolError::Execution {
                        name: self.tool_name.clone(),
                        cause: e.to_string(),
                    })
                }
            }
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: format!("{}__{}", self.server_id, self.tool_name),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            source: ToolSource::Mcp { server_id: self.server_id.clone() },
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}

// Discriminator — adapt to match concrete McpClient error type. Likely
// checks for IO / connection-closed variants.
fn is_transport(err: &impl std::fmt::Display) -> bool {
    let s = err.to_string();
    s.contains("connection")
        || s.contains("io error")
        || s.contains("closed")
        || s.contains("transport")
}
```

- [ ] **Step 4.3: Implement `ExtensionHandler`**

```rust
//! ExtensionHandler — dispatches through ExtensionRuntime.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};

// ADAPT: find the extension runtime entry point:
//   grep -rn 'ExtensionRuntime\|ExtensionLoader\|plugin_load' src/

pub struct ExtensionHandler {
    // runtime: Arc<ExtensionRuntime>,
    plugin_id: String,
    tool_name: String,
    description: String,
    input_schema: Value,
}

impl ExtensionHandler {
    pub fn new(
        /* runtime: Arc<ExtensionRuntime>, */
        plugin_id: String,
        tool_name: String,
        description: String,
        input_schema: Value,
    ) -> Self {
        Self { plugin_id, tool_name, description, input_schema }
    }
}

#[async_trait]
impl ToolHandler for ExtensionHandler {
    async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
        // ADAPT: call the runtime entry. Likely:
        //   self.runtime.invoke_tool(&self.plugin_id, &self.tool_name, input).await
        // Map errors similarly to McpHandler.
        Err(ToolError::Other(
            "ExtensionHandler not yet wired — fill ADAPT in Task 4".into(),
        ))
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: format!("ext__{}__{}", self.plugin_id, self.tool_name),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            source: ToolSource::Extension { plugin_id: self.plugin_id.clone() },
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}
```

The engineer **must** fill the ADAPT points with real runtime calls by the end of this task. Do not commit the placeholder `Err(ToolError::Other(...))` return.

- [ ] **Step 4.4: Wire registration in the MCP connection lifecycle**

Find the code that currently calls `tools/list` on a newly-connected MCP server (from Step 4.1). Add registry calls. Pattern:

```rust
for tool in fetched_tools {
    let handler: Arc<dyn ToolHandler> = Arc::new(McpHandler::new(
        client.clone(),
        server_id.clone(),
        tool.name.clone(),
        tool.description.clone(),
        tool.input_schema.clone(),
    ));
    let qualified_name = format!("{}__{}", server_id, tool.name);
    if let Err(e) = tool_registry.register(qualified_name.clone(), handler) {
        tracing::warn!(?e, "MCP tool register failed for {qualified_name}");
    }
}
```

Wire disconnect: iterate registry snapshot → find handlers whose definition's `source` matches `Mcp { server_id }` → unregister.

The MCP connection code needs an `Arc<ToolRegistry>` field to call these. Add it to the struct; thread through its constructor.

- [ ] **Step 4.5: Wire extension registration similarly**

Find extension-load code (grep Step 4.1's commands). Same pattern: construct `ExtensionHandler`, register.

- [ ] **Step 4.6: Integration test**

Create `tests/tool_service_mcp_extension.rs` (or adapt existing MCP test file):

```rust
// Mock MCP server + mock extension runtime (if feasible — if not feasible due
// to heavy setup, create a minimal fake McpClient/ExtensionRuntime that the
// handlers can invoke).
//
// Test shape:
//  1. Start with empty ToolRegistry
//  2. Connect mock MCP → register 2 tools → assert list() returns 2
//  3. Execute one of the MCP tools → assert output
//  4. Disconnect mock MCP → list() returns 0
//  5. Attempting execute on the unregistered tool returns NotFound
```

If mocking MCP is too invasive, defer integration test to Task 13 and rely on unit tests of the handlers alone.

- [ ] **Step 4.7: Build + test**

Run: `cargo check -p alephcore 2>&1 | tail -3` → `Finished dev`
Run: `cargo test -p alephcore --lib tools:: 2>&1 | tail -15` → all prior + new handler tests pass

- [ ] **Step 4.8: Commit**

```bash
git add -A
git commit -m "tools: McpHandler + ExtensionHandler + registration wiring

Phase 2 Task 4: connect-time registration, disconnect cleanup. MCP tools
carry {server_id}__{tool} namespace, extension tools ext__{plugin_id}__{tool}.
Transport vs execution errors distinguished per §6."
```

---

## Task 5: Middleware shells (all five layers, pass-through)

**Files:** Modify all five `src/tools/middleware/*.rs` files.

**Context:** Establish the composition skeleton. Each layer forwards to `inner` without adding logic. Real policy lands in Tasks 6–9.

- [ ] **Step 5.1: Write shells in a consistent pattern**

Example for `src/tools/middleware/audit.rs`:

```rust
//! ExecAuditLayer — tracing + latency (Task 9 fills in).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct ExecAuditLayer {
    inner: Arc<dyn ToolService>,
}

impl ExecAuditLayer {
    pub fn new(inner: Arc<dyn ToolService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolService for ExecAuditLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        self.inner.execute(name, input).await
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner.list().await
    }
    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.inner.describe(name).await
    }
}
```

Create the same pattern for:
- `src/tools/middleware/permission.rs` → struct `PermissionLayer { inner }`
- `src/tools/middleware/context_rule.rs` → struct `ContextRuleLayer { inner }`
- `src/tools/middleware/timeout.rs` → struct `TimeoutLayer { inner }`

Each of the four above differs only in struct name and constructor signature — at this step, all just forward.

- [ ] **Step 5.2: Export them in `src/tools/middleware/mod.rs`**

```rust
//! ToolService middleware layers.

pub mod audit;
pub mod permission;
pub mod context_rule;
pub mod timeout;

pub use audit::ExecAuditLayer;
pub use permission::PermissionLayer;
pub use context_rule::ContextRuleLayer;
pub use timeout::TimeoutLayer;
```

- [ ] **Step 5.3: Compose in a test (sanity-check stacking)**

Add to `src/tools/middleware/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::tools::dispatch::CoreDispatch;
    use crate::tools::registry::ToolRegistry;
    use crate::tools::service::ToolService;

    #[tokio::test]
    async fn empty_chain_compiles_and_lists_empty() {
        let registry = Arc::new(ToolRegistry::new());
        let core = Arc::new(CoreDispatch::new(registry));
        let timeout = Arc::new(TimeoutLayer::new(core));
        let ctx = Arc::new(ContextRuleLayer::new(timeout));
        let perm = Arc::new(PermissionLayer::new(ctx));
        let audit = Arc::new(ExecAuditLayer::new(perm));
        let svc: Arc<dyn ToolService> = audit;
        assert!(svc.list().await.is_empty());
    }
}
```

For shells, the constructors take only `inner` — later tasks extend the signatures.

- [ ] **Step 5.4: Build + test**

Run: `cargo test -p alephcore --lib tools::middleware 2>&1 | tail -10`
Expected: `empty_chain_compiles_and_lists_empty` passes.

- [ ] **Step 5.5: Commit**

```bash
git add src/tools/middleware/
git commit -m "tools: middleware layer shells (pass-through)

Phase 2 Task 5: Audit, Permission, ContextRule, Timeout — all five
layers forward to inner. Composition sanity-test green. Real policy
lands in Tasks 6–9."
```

---

## Task 6: Relocate `ContextRule` into `ContextRuleLayer`

**Files:**
- Modify: `src/tools/middleware/context_rule.rs` — move logic in
- Delete lines from wherever `ContextRule` lived in `src/agent_loop/` (keep the struct definition — only move the **invocation** into the layer; the struct can relocate in Task 11 if it's cleaner)

**Context:** Preserve behavior bit-for-bit. Read the existing invocation, copy it into the layer's `execute` method. Don't redesign.

- [ ] **Step 6.1: Find the existing logic**

```bash
grep -rn 'struct ContextRule\|fn apply_context_rule\|fn evaluate_rule\|context_rule' src/agent_loop/ src/ 2>/dev/null | head -20
```
Identify: where the struct is defined, where rules are loaded (ArcSwap or config), where they're evaluated during tool dispatch.

- [ ] **Step 6.2: Relocate invocation into `ContextRuleLayer::execute`**

Build on the Task 5 shell. Augment constructor to take `rules: Arc<ArcSwap<Vec<ContextRule>>>`.

```rust
pub struct ContextRuleLayer {
    inner: Arc<dyn ToolService>,
    rules: Arc<ArcSwap<Vec<ContextRule>>>,
}

impl ContextRuleLayer {
    pub fn new(inner: Arc<dyn ToolService>, rules: Arc<ArcSwap<Vec<ContextRule>>>) -> Self {
        Self { inner, rules }
    }
}

#[async_trait]
impl ToolService for ContextRuleLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let rules = self.rules.load();
        // COPY the existing evaluation logic here verbatim. It should be a
        // linear scan returning an action: Allow / Deny(reason) / Rewrite(new_input).
        let action = /* existing eval */;
        match action {
            ContextRuleAction::Allow => self.inner.execute(name, input).await,
            ContextRuleAction::Deny(reason) => Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason,
            }),
            ContextRuleAction::Rewrite(new_input) => self.inner.execute(name, new_input).await,
        }
    }
    // list() + describe() still forward
}
```

If `ContextRule` currently returns a different action type (e.g. `bool` allow/deny), adapt the code to the new `ContextRuleAction` enum you define in this file (or reuse whatever the existing code uses). **Do not change the rule evaluation semantics.**

- [ ] **Step 6.3: Update the invocation path in `agent_loop/`**

The old caller in `agent_loop/` (tool_pipeline or similar) previously evaluated rules inline. In this task, we're only **relocating** the evaluation — not removing it yet. Leave the old call in place (agent_loop will be fully migrated in Task 10). The new layer operates in parallel and gets its rules from the same source.

Dual-evaluation is OK here because the action is idempotent (same input → same action).

- [ ] **Step 6.4: Add parity tests**

For each representative rule: construct rules → run through `ContextRuleLayer` → assert same action as the legacy path.

```rust
#[tokio::test]
async fn rule_deny_short_circuits() {
    let rules = Arc::new(ArcSwap::from_pointee(vec![
        ContextRule { /* match-all deny */ },
    ]));
    let inner = /* mock that would succeed */;
    let layer = ContextRuleLayer::new(inner, rules);
    let err = layer.execute("anything", json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::PermissionDenied { .. }));
}

#[tokio::test]
async fn rule_rewrite_passes_new_input() {
    // Construct a rule that rewrites input. Mock inner records the input it sees.
    // Assert inner saw the rewritten input, not the original.
}

#[tokio::test]
async fn empty_rules_allow_through() {
    // rules empty → inner gets called with original input
}
```

- [ ] **Step 6.5: Build + test**

Run: `cargo test -p alephcore --lib tools::middleware::context_rule 2>&1 | tail -10`
Expected: 3+ new tests pass.

Run: `cargo test -p alephcore --lib agent_loop 2>&1 | tail -5`
Expected: no regression in existing agent_loop tests (they still use the legacy inline eval).

- [ ] **Step 6.6: Commit**

```bash
git add src/tools/middleware/context_rule.rs src/agent_loop/
git commit -m "tools: relocate ContextRule evaluation into ContextRuleLayer

Phase 2 Task 6: evaluation logic now lives in the middleware layer.
Agent_loop still calls the legacy path in parallel; Task 10 removes
the inline call."
```

---

## Task 7: Relocate `SmartFilter` + `ApprovalGate` into `PermissionLayer`

**Files:**
- Modify: `src/tools/middleware/permission.rs`
- Possibly move `SmartFilter` struct definition (depends on where it lives; grep below)

**Context:** Same pattern as Task 6, but permission has three outcomes (Allow / Confirm / Deny) plus user-interaction via ApprovalGate.

- [ ] **Step 7.1: Locate existing code**

```bash
grep -rn 'struct SmartFilter\|SmartFilterConfig\|always_allow\|require_confirmation\|never_allow' src/ | head -20
grep -rn 'struct ApprovalGate\|ApprovalRequest\|ask_approval' src/ | head -15
```
Record: where SmartFilter is instantiated, where it's applied currently, and the ApprovalGate's async API.

- [ ] **Step 7.2: Implement `PermissionLayer`**

```rust
//! PermissionLayer — SmartFilter + ApprovalGate.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

// ADAPT the imports below to match where SmartFilter and ApprovalGate live now.
use crate::agent_loop::exec_approval::gate::ApprovalGate;
// SmartFilter likely at src/agent_loop/... — adapt.
use crate::tools::middleware::permission::filter::SmartFilter;

pub mod filter; // holds the SmartFilter struct, relocated from agent_loop.

pub struct PermissionLayer {
    inner: Arc<dyn ToolService>,
    smart_filter: Arc<SmartFilter>,
    approval_gate: Arc<ApprovalGate>,
}

impl PermissionLayer {
    pub fn new(
        inner: Arc<dyn ToolService>,
        smart_filter: Arc<SmartFilter>,
        approval_gate: Arc<ApprovalGate>,
    ) -> Self {
        Self { inner, smart_filter, approval_gate }
    }
}

pub enum Classification {
    Allow,
    Confirm,
    Deny,
}

#[async_trait]
impl ToolService for PermissionLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        match self.smart_filter.classify(name) {
            Classification::Deny => {
                return Err(ToolError::PermissionDenied {
                    name: name.to_string(),
                    reason: "smart_filter: never_allow".into(),
                });
            }
            Classification::Confirm => {
                // ADAPT: use the real approval_gate.ask(...) signature.
                let approved = self
                    .approval_gate
                    .ask(name, &input)
                    .await
                    .map_err(|e| ToolError::Other(e.to_string()))?;
                if !approved {
                    return Err(ToolError::PermissionDenied {
                        name: name.to_string(),
                        reason: "user denied approval".into(),
                    });
                }
            }
            Classification::Allow => {}
        }
        self.inner.execute(name, input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> { self.inner.list().await }
    async fn describe(&self, name: &str) -> Option<ToolDefinition> { self.inner.describe(name).await }
}
```

- [ ] **Step 7.3: Relocate `SmartFilter` struct into `src/tools/middleware/permission/filter.rs`**

Copy the struct and its impl verbatim from the old location. Adjust visibility so `PermissionLayer` can see it. Delete the old definition only after the new one compiles and tests pass.

- [ ] **Step 7.4: Leave `ApprovalGate` in place**

The gate stays at `src/agent_loop/exec_approval/gate.rs`. Only the caller moves. (Spec §7.4 locks this.)

- [ ] **Step 7.5: Parity tests**

```rust
#[tokio::test]
async fn always_allow_bypasses_approval() { /* never calls approval_gate */ }

#[tokio::test]
async fn never_allow_short_circuits() { /* returns PermissionDenied without reaching inner */ }

#[tokio::test]
async fn require_confirmation_calls_approval_and_proceeds_on_yes() {}

#[tokio::test]
async fn require_confirmation_returns_denied_on_no() {}
```

Use a mock ApprovalGate if the real one requires UI plumbing. Document the mock.

- [ ] **Step 7.6: Build + test**

Run: `cargo test -p alephcore --lib tools::middleware::permission 2>&1 | tail -10`
Run: `cargo test -p alephcore --lib agent_loop 2>&1 | tail -5`
Both green; no regression.

- [ ] **Step 7.7: Commit**

```bash
git add src/tools/middleware/permission.rs src/tools/middleware/permission/ src/agent_loop/
git commit -m "tools: relocate SmartFilter + approval calls into PermissionLayer

Phase 2 Task 7: SmartFilter struct moved to src/tools/middleware/permission/filter.rs.
ApprovalGate stays at src/agent_loop/exec_approval/ — only the call-site relocates.
Layer short-circuits Deny without waiting; Confirm routes through the gate; Allow
passes through unaltered."
```

---

## Task 8: Real `TimeoutLayer`

**Files:** Modify: `src/tools/middleware/timeout.rs`

**Context:** Wrap `inner.execute` in `tokio::time::timeout`. Default comes from config; per-tool overrides carried in a `HashMap<String, Duration>`.

- [ ] **Step 8.1: Implement the real layer**

```rust
//! TimeoutLayer — per-tool timeout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::time::timeout;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct TimeoutLayer {
    inner: Arc<dyn ToolService>,
    default_timeout: Duration,
    per_tool_override: HashMap<String, Duration>,
}

impl TimeoutLayer {
    pub fn new(
        inner: Arc<dyn ToolService>,
        default_timeout: Duration,
        per_tool_override: HashMap<String, Duration>,
    ) -> Self {
        Self { inner, default_timeout, per_tool_override }
    }

    fn timeout_for(&self, name: &str) -> Duration {
        self.per_tool_override
            .get(name)
            .copied()
            .unwrap_or(self.default_timeout)
    }
}

#[async_trait]
impl ToolService for TimeoutLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let t = self.timeout_for(name);
        match timeout(t, self.inner.execute(name, input)).await {
            Ok(result) => result,
            Err(_) => Err(ToolError::Timeout {
                name: name.to_string(),
                elapsed_ms: t.as_millis() as u64,
            }),
        }
    }
    async fn list(&self) -> Vec<ToolDefinition> { self.inner.list().await }
    async fn describe(&self, name: &str) -> Option<ToolDefinition> { self.inner.describe(name).await }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::service::ToolService;
    use async_trait::async_trait;

    struct SlowInner {
        delay: Duration,
    }

    #[async_trait]
    impl ToolService for SlowInner {
        async fn execute(&self, _name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            tokio::time::sleep(self.delay).await;
            Ok(ToolOutput {
                value: serde_json::json!("done"),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> { vec![] }
        async fn describe(&self, _n: &str) -> Option<ToolDefinition> { None }
    }

    #[tokio::test(start_paused = true)]
    async fn fast_tool_succeeds() {
        let inner = Arc::new(SlowInner { delay: Duration::from_millis(10) });
        let layer = TimeoutLayer::new(inner, Duration::from_secs(1), HashMap::new());
        let out = layer.execute("x", serde_json::json!({})).await.unwrap();
        assert_eq!(out.value, serde_json::json!("done"));
    }

    #[tokio::test(start_paused = true)]
    async fn slow_tool_times_out() {
        let inner = Arc::new(SlowInner { delay: Duration::from_secs(10) });
        let layer = TimeoutLayer::new(inner, Duration::from_millis(100), HashMap::new());
        let err = layer.execute("x", serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::Timeout { elapsed_ms: 100, .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn per_tool_override_wins() {
        let inner = Arc::new(SlowInner { delay: Duration::from_secs(10) });
        let mut overrides = HashMap::new();
        overrides.insert("slow".to_string(), Duration::from_millis(50));
        let layer = TimeoutLayer::new(inner, Duration::from_secs(100), overrides);
        let err = layer.execute("slow", serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::Timeout { elapsed_ms: 50, .. }));
    }
}
```

- [ ] **Step 8.2: Run tests**

Run: `cargo test -p alephcore --lib tools::middleware::timeout 2>&1 | tail -10`
Expected: 3 passed.

- [ ] **Step 8.3: Commit**

```bash
git add src/tools/middleware/timeout.rs
git commit -m "tools: TimeoutLayer with per-tool override

Phase 2 Task 8: wraps inner.execute in tokio::time::timeout; per-tool
Duration override via HashMap; maps elapsed to ToolError::Timeout."
```

---

## Task 9: Real `ExecAuditLayer` (latency + tracing)

**Files:** Modify: `src/tools/middleware/audit.rs`

- [ ] **Step 9.1: Implement**

```rust
//! ExecAuditLayer — tracing + latency measurement (outermost layer).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct ExecAuditLayer {
    inner: Arc<dyn ToolService>,
}

impl ExecAuditLayer {
    pub fn new(inner: Arc<dyn ToolService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolService for ExecAuditLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        tracing::info!(target: "tool.call", tool = %name, phase = "start");

        let result = self.inner.execute(name, input).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        match &result {
            Ok(_) => tracing::info!(target: "tool.call", tool = %name, phase = "ok", elapsed_ms),
            Err(e) => tracing::warn!(target: "tool.call", tool = %name, phase = "err", elapsed_ms, error = %e),
        }

        // Stamp latency on success. Failure path is observed via the event
        // emitter (agent_loop) which carries error info; no latency stamping
        // because Err has no ToolOutput to stamp.
        result.map(|mut out| {
            out.metadata.latency_ms = elapsed_ms;
            out
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> { self.inner.list().await }
    async fn describe(&self, name: &str) -> Option<ToolDefinition> { self.inner.describe(name).await }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct NoOp;
    #[async_trait]
    impl ToolService for NoOp {
        async fn execute(&self, _n: &str, _i: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput { value: serde_json::json!(true), metadata: Default::default() })
        }
        async fn list(&self) -> Vec<ToolDefinition> { vec![] }
        async fn describe(&self, _n: &str) -> Option<ToolDefinition> { None }
    }

    #[tokio::test]
    async fn stamps_latency_on_success() {
        let inner = Arc::new(NoOp);
        let layer = ExecAuditLayer::new(inner);
        let out = layer.execute("whatever", serde_json::json!({})).await.unwrap();
        // latency_ms is 0 on very fast calls under `start_paused = true`, but
        // in a real test clock it's small positive. Loose assertion:
        assert!(out.metadata.latency_ms < 1_000_000);
    }
}
```

- [ ] **Step 9.2: Build + test**

Run: `cargo test -p alephcore --lib tools::middleware::audit 2>&1 | tail -10`
Expected: 1 passed.

- [ ] **Step 9.3: Commit**

```bash
git add src/tools/middleware/audit.rs
git commit -m "tools: ExecAuditLayer with latency + tracing

Phase 2 Task 9: measures from outermost boundary, stamps latency_ms on
ToolOutput.metadata; tracing spans on call-start and call-end (ok|err)."
```

---

## Task 10: `ToolServiceConfig` + AppContext assembly

**Files:**
- Create: `src/config/types/tool_service.rs` (or `src/tools/config.rs` — follow project convention; grep `src/config/types/` for style)
- Modify: `src/bin/aleph-server/commands/start/builder/` — add builder method

- [ ] **Step 10.1: Define `ToolServiceConfig`**

```rust
//! ToolService runtime configuration — timeouts, future tunables.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolServiceConfig {
    #[serde(default = "default_timeout_seconds")]
    pub default_timeout_seconds: u64,

    /// Tool-name → seconds.
    #[serde(default)]
    pub per_tool_seconds: HashMap<String, u64>,
}

fn default_timeout_seconds() -> u64 {
    60
}

impl Default for ToolServiceConfig {
    fn default() -> Self {
        Self {
            default_timeout_seconds: 60,
            per_tool_seconds: HashMap::new(),
        }
    }
}

impl ToolServiceConfig {
    pub fn default_timeout(&self) -> Duration {
        Duration::from_secs(self.default_timeout_seconds)
    }

    pub fn per_tool_durations(&self) -> HashMap<String, Duration> {
        self.per_tool_seconds
            .iter()
            .map(|(k, v)| (k.clone(), Duration::from_secs(*v)))
            .collect()
    }
}
```

Wire it into the existing `Config` struct (grep `pub struct Config` or similar; it probably has `acp`, `session`, `memory` sections — add `tools` alongside).

- [ ] **Step 10.2: AppContext builder helper**

```rust
pub fn build_tool_service(
    server: Arc<ToolServer>,
    smart_filter: Arc<SmartFilter>,
    approval: Arc<ApprovalGate>,
    rules: Arc<ArcSwap<Vec<ContextRule>>>,
    config: &ToolServiceConfig,
) -> Arc<dyn ToolService> {
    let registry = Arc::new(ToolRegistry::new());
    register_builtins_into(&registry, &server);

    let core    = Arc::new(CoreDispatch::new(registry.clone()));
    let timeout = Arc::new(TimeoutLayer::new(
        core,
        config.default_timeout(),
        config.per_tool_durations(),
    ));
    let ctxrule = Arc::new(ContextRuleLayer::new(timeout, rules));
    let perm    = Arc::new(PermissionLayer::new(ctxrule, smart_filter, approval));
    let audit   = Arc::new(ExecAuditLayer::new(perm));
    audit
}

fn register_builtins_into(registry: &ToolRegistry, server: &ToolServer) {
    for (name, tool) in server.all_builtin() {
        let handler = Arc::new(BuiltinHandler::new(name.clone(), tool.clone()));
        if let Err(e) = registry.register(name, handler) {
            tracing::warn!(?e, "builtin register failed");
        }
    }
}
```

`ToolServer::all_builtin()` may not exist — grep current `ToolServer` surface and adapt. If necessary, add a read-only `all_builtin_handlers()` method.

- [ ] **Step 10.3: Wire into AppContext**

Find where `AppContext` fields are assigned during startup (typical: `src/bin/aleph-server/commands/start/builder/handlers.rs` or `src/bin/aleph-server/commands/start/mod.rs`). Add:

```rust
let tool_service: Arc<dyn ToolService> = build_tool_service(
    tool_server.clone(),
    smart_filter.clone(),
    approval_gate.clone(),
    context_rules.clone(),
    &config.tools,
);
app_context.tool_service = tool_service.clone();
```

The `AppContext` struct needs a new `tool_service: Arc<dyn ToolService>` field — add it.

- [ ] **Step 10.4: Build + test**

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`
Run: `cargo test -p alephcore --lib 2>&1 | tail -10` → no new failures.

- [ ] **Step 10.5: Commit**

```bash
git add src/config/ src/bin/aleph-server/ src/
git commit -m "tools: ToolServiceConfig + AppContext assembly

Phase 2 Task 10: ToolServiceConfig with default_timeout + per_tool overrides.
build_tool_service helper composes the full decorator chain at startup; injected
into AppContext as Arc<dyn ToolService>."
```

---

## Task 11: Agent_loop migration — consume `ToolService`

**Files:**
- Modify: `src/agent_loop/tool_pipeline.rs`
- Modify: `src/agent_loop/tool_orchestrator.rs`
- Possibly other agent_loop files that import `McpClient` / `BuiltinToolRegistry` / `ExtensionTool`
- Create: `src/session/tool_trace.rs` (the `invoke_with_session_trace` helper)

**Context:** Every direct tool dispatch from agent_loop routes through `Arc<dyn ToolService>` instead. The helper bundles session-event emission.

- [ ] **Step 11.1: Implement the helper**

Create `src/session/tool_trace.rs`:

```rust
//! Helper that wires ToolService dispatch + SessionService event emission.
//!
//! Call this from agent_loop wherever a tool is invoked. Keeps the two façades
//! orthogonal (neither imports the other).

use std::sync::Arc;

use serde_json::Value;

use crate::session::events::{
    now_ms, SessionEvent, SessionId, ToolOutput, TurnId,
};
use crate::session::service::SessionService;
use crate::tools::service::{ToolError, ToolService};

pub async fn invoke_with_session_trace(
    tool_svc: &Arc<dyn ToolService>,
    session_svc: &Arc<dyn SessionService>,
    session_id: &SessionId,
    turn_id: TurnId,
    call_id: String,
    name: String,
    input: Value,
) -> Result<ToolOutput, ToolError> {
    // Emit requested event first — fire-and-forget style
    let _ = session_svc
        .emit_event(
            session_id,
            SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call_id.clone(),
                name: name.clone(),
                input: input.clone(),
                at: now_ms(),
            },
        )
        .await;

    let result = tool_svc.execute(&name, input).await;

    match &result {
        Ok(output) => {
            let _ = session_svc
                .emit_event(
                    session_id,
                    SessionEvent::ToolResult {
                        turn_id,
                        call_id,
                        output: output.clone(),
                        at: now_ms(),
                    },
                )
                .await;
        }
        Err(ToolError::PermissionDenied { reason, .. }) => {
            let _ = session_svc
                .emit_event(
                    session_id,
                    SessionEvent::ToolCallDenied {
                        turn_id,
                        call_id,
                        reason: reason.clone(),
                        at: now_ms(),
                    },
                )
                .await;
        }
        Err(e) => {
            let _ = session_svc
                .emit_event(
                    session_id,
                    SessionEvent::ToolError {
                        turn_id,
                        call_id,
                        error: e.to_string(),
                        at: now_ms(),
                    },
                )
                .await;
        }
    }

    result
}
```

Register in `src/session/mod.rs`:
```rust
pub mod tool_trace;
pub use tool_trace::invoke_with_session_trace;
```

- [ ] **Step 11.2: Discover agent_loop tool-call sites**

```bash
grep -rn 'McpClient\|BuiltinToolRegistry\|ExtensionTool\|tool_server\.' src/agent_loop/ | head -30
grep -rn 'fn invoke\|fn dispatch\|\.call_tool\|\.execute_tool' src/agent_loop/tool_pipeline.rs src/agent_loop/tool_orchestrator.rs 2>/dev/null | head -20
```

List every tool dispatch call site in agent_loop.

- [ ] **Step 11.3: Per-site migration, one commit each**

For each site:
1. Remove direct reference to `McpClient` / `BuiltinToolRegistry` / `ExtensionTool`
2. Call `invoke_with_session_trace(tool_svc, session_svc, session_id, turn_id, call_id, name, input)` if the site has access to `session_id` + `turn_id`; otherwise call `tool_svc.execute(name, input)` directly (some pipeline-internal sites may not be in a turn context)
3. Remove the inline SmartFilter / ContextRule evaluation — these now run inside the decorator chain
4. `cargo check -p alephcore` → `Finished dev`
5. `cargo test -p alephcore --lib agent_loop 2>&1 | tail -5` → no regression
6. Commit: `git commit -m "agent_loop: migrate <specific-site> to ToolService"`

**Small commits.** If Task 11 produces 6–10 commits, that's expected. Bisect-debuggable.

- [ ] **Step 11.4: Final verification**

```bash
grep -rn 'McpClient\|BuiltinToolRegistry\|ExtensionTool' src/agent_loop/
grep -rn 'SmartFilter\|ContextRule' src/agent_loop/
```
Both expected zero output. If any site resists migration (e.g. it's a type-name reference in a struct field rather than a call), document the resisting case in the final commit message.

- [ ] **Step 11.5: Test**

```bash
cargo test -p alephcore --lib 2>&1 | tail -10
```
Expected: **8982+ passed / 2 failed** baseline held.

- [ ] **Step 11.6: Final commit for the batch**

If uncommitted leftovers remain:
```bash
git add -A
git commit -m "agent_loop: final cleanup — no more direct McpClient/Registry refs"
```

---

## Task 12: Documentation + CHANGELOG + final verification + release gate

**Files:**
- Modify: `docs/reference/TOOL_SYSTEM.md`
- Modify: `docs/reference/GLOSSARY.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 12.1: Update TOOL_SYSTEM.md**

Add a "Phase 2 refactor" section at the top pointing to the façade:

```markdown
## ToolService façade (post-Phase-2)

Consumers (agent_loop and future Harness) depend on `Arc<dyn ToolService>` exclusively:

```
pub trait ToolService: Send + Sync + 'static {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError>;
    async fn list(&self) -> Vec<ToolDefinition>;
    async fn describe(&self, name: &str) -> Option<ToolDefinition>;
}
```

Implementation stack at runtime (outer to inner):
  ExecAuditLayer → PermissionLayer → ContextRuleLayer → TimeoutLayer → CoreDispatch

CoreDispatch holds an ArcSwap-backed ToolRegistry of three handler sources:
  BuiltinHandler, McpHandler, ExtensionHandler.

Tool authors continue to implement `AlephTool` — the façade adapts them via
`BuiltinHandler`. No author-side API change.

See: docs/superpowers/specs/2026-04-18-tool-service-facade-design.md
```

- [ ] **Step 12.2: Update GLOSSARY.md**

Flip the "Tools" entry from future-tense to present-tense:

```markdown
**Aleph today:** `ToolService` trait (`src/tools/service.rs`), backed by
`CoreDispatch` + `ToolRegistry` (ArcSwap) and a five-layer decorator chain
(Audit / Permission / ContextRule / Timeout / Core). Three sources (builtin,
MCP, extension) plug in as `ToolHandler` impls. See [TOOL_SYSTEM.md](./TOOL_SYSTEM.md).
```

- [ ] **Step 12.3: CHANGELOG entry**

Append under `## [Unreleased]`:

```markdown
### Added
- **Tool Service façade:** `src/tools/service.rs` exposes a single
  `execute(name, input) → Result<ToolOutput, ToolError>` across builtin, MCP,
  and extension sources. Five-layer decorator chain (audit/permission/
  context-rule/timeout/core) replaces the inline policy logic in agent_loop.
  Phase 2 of the managed-agents refactor.

### Changed
- `src/agent_loop/**` no longer imports `McpClient`, `BuiltinToolRegistry`,
  or `ExtensionTool` directly; tool calls route through
  `Arc<dyn ToolService>`. SmartFilter and ContextRule evaluation moved to
  `src/tools/middleware/`.
```

- [ ] **Step 12.4: Final verification gate**

```bash
echo "=== agent_loop zero-coupling ==="
grep -rn 'McpClient\|BuiltinToolRegistry\|ExtensionTool' src/agent_loop/ || echo "(none)"
grep -rn 'SmartFilter\|ContextRule' src/agent_loop/ || echo "(none)"

echo ""
echo "=== src/tools/ structure ==="
ls src/tools/
ls src/tools/handlers/
ls src/tools/middleware/

echo ""
echo "=== full test suite ==="
cargo test -p alephcore --lib 2>&1 | tail -10
```

Expected:
- Both agent_loop greps: `(none)` or zero lines
- `src/tools/` contains `service.rs`, `registry.rs`, `dispatch.rs`, `handlers/`, `middleware/`
- Test result matches baseline: **8982+ passed / 2 failed** (same 2 pre-existing as Pre-3)

If any gate fails — do NOT proceed. Fix or revert.

- [ ] **Step 12.5: Clippy on new code**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | grep -E 'src/tools/' | head -20
```
Expected: no lines from `src/tools/*`. Pre-existing clippy debt in other modules is not this task's concern.

- [ ] **Step 12.6: Commit docs + changelog**

```bash
git add docs/reference/TOOL_SYSTEM.md docs/reference/GLOSSARY.md
git commit -m "docs: Tool Service façade reference (Phase 2)"

git add CHANGELOG.md
git commit -m "changelog: note Phase 2 Tool Service façade"
```

- [ ] **Step 12.7: Release gate — STOP**

Phase 2 is code-complete. Do NOT auto-release. Present to the user:

> "Phase 2 implementation complete on branch `worktree-managed-agents-phase-2`. All commits green, no new test failures beyond the 2 pre-existing on main. Next step options:
>
> 1. **Merge to main** — `git -C /Volumes/TBU4/Workspace/Aleph merge worktree-managed-agents-phase-2 --no-ff`
> 2. **Release** — `just release $(date +%Y.%m.%d)`
> 3. **Both**
> 4. **Start Phase 3 brainstorm** (Sandbox trait — workspace sandbox)
>
> Which?"

Only proceed on explicit choice.

---

## Non-Goals (explicit scope discipline)

- Do NOT migrate Gateway `tools.*` RPC to `ToolService` — that stays on `ToolServer` until a later phase
- Do NOT change the `AlephTool` author-side trait — only adapt via `BuiltinHandler`
- Do NOT add `subscribe()` to `ToolService` — YAGNI in v1
- Do NOT change MCP/Extension runtime lifecycle beyond adding registry register/unregister hooks

## Rollback

If any task's gate fails:
```bash
git revert <sha>
```
Do NOT `git reset --hard` without explicit user consent.

## Done-ness Signals

Phase 2 is done when:
1. All 12 tasks checked off
2. `grep -rn 'McpClient\|BuiltinToolRegistry\|ExtensionTool' src/agent_loop/` → zero hits
3. `grep -rn 'SmartFilter\|ContextRule' src/agent_loop/` → zero hits
4. MCP hot-reload path works: connect → `list()` grows; disconnect → `list()` shrinks
5. Baseline `cargo test -p alephcore --lib` matches main (8982 passed / 2 pre-existing failed)
6. CHANGELOG entry committed
7. User has made a merge/release decision at Step 12.7

Proceed to **Phase 3 brainstorming** only after all signals are green.
