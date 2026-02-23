# Server Purification Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove all desktop control components (perception, vision, browser, canvas) and simplify the execution architecture to Server-only mode.

**Architecture:** Delete ~15 modules/files related to desktop perception, browser CDP automation, and client-side tool routing. Simplify ExecutionEngine to always use SingleStepExecutor. Browser automation delegated to Playwright MCP Server via config.

**Tech Stack:** Rust, Cargo feature flags, MCP protocol

**Design doc:** `docs/plans/2026-02-23-server-purification-design.md`

---

### Task 1: Remove Cargo.toml dependencies and feature flags

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Remove browser feature flag**

In `core/Cargo.toml`, delete lines 28-29:
```toml
# Browser control via CDP
browser = ["dep:chromiumoxide", "gateway"]
```

**Step 2: Remove chromiumoxide dependency**

Delete lines 199-200:
```toml
# Browser control dependencies (optional)
chromiumoxide = { version = "0.7", optional = true, default-features = false, features = ["tokio-runtime"] }
```

**Step 3: Remove macOS-only desktop dependencies**

Delete lines 225-228:
```toml
[target.'cfg(target_os = "macos")'.dependencies]
accessibility-sys = "0.1"
core-foundation = "0.9"
core-graphics = "0.23"
```

**Step 4: Build to verify dependency removal**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -50`
Expected: Compilation errors about missing modules (we'll fix those next)

**Step 5: Commit**

```bash
git add core/Cargo.toml
git commit -m "deps: remove chromiumoxide, accessibility-sys, core-foundation, core-graphics"
```

---

### Task 2: Delete perception, vision, and browser directories

**Files:**
- Delete: `core/src/perception/` (entire directory)
- Delete: `core/src/vision/` (entire directory)
- Delete: `core/src/browser/` (entire directory)

**Step 1: Delete the three directories**

```bash
rm -rf core/src/perception/
rm -rf core/src/vision/
rm -rf core/src/browser/
```

**Step 2: Remove module declarations from lib.rs**

In `core/src/lib.rs`, delete line 70:
```rust
pub mod perception;
```

Delete line 87:
```rust
pub mod vision;
```

Delete lines 109-110:
```rust
#[cfg(feature = "browser")]
pub mod browser;
```

**Step 3: Remove vision re-exports from lib.rs**

In `core/src/lib.rs`, delete lines 293:
```rust
pub use crate::vision::{VisionConfig, VisionRequest, VisionResult, VisionService};
```

**Step 4: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -80`
Expected: Errors about `FocusHint`, `PerceptionRef`, handler modules, etc. (fixed in subsequent tasks)

**Step 5: Commit**

```bash
git add -A
git commit -m "remove: delete perception, vision, browser modules"
```

---

### Task 3: Clean up core/types.rs (remove PerceptionRef)

**Files:**
- Modify: `core/src/core/types.rs`

**Step 1: Remove FocusHint import and PerceptionRef**

In `core/src/core/types.rs`:

Delete line 11:
```rust
use crate::perception::FocusHint;
```

Remove `perception` field from `CapturedContext` (line 37):
```rust
    pub perception: Option<PerceptionRef>,         // Optional perception snapshot reference
```

Delete the entire `PerceptionRef` struct (lines 40-45):
```rust
/// Reference to a perception snapshot.
#[derive(Debug, Clone)]
pub struct PerceptionRef {
    pub snapshot_id: String,
    pub focus_hint: Option<FocusHint>,
}
```

**Step 2: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -50`

**Step 3: Commit**

```bash
git add core/src/core/types.rs
git commit -m "remove: PerceptionRef and FocusHint from core types"
```

---

### Task 4: Delete builtin tools (snapshot_capture, canvas)

**Files:**
- Delete: `core/src/builtin_tools/snapshot_capture.rs`
- Delete: `core/src/builtin_tools/canvas/` (entire directory)
- Modify: `core/src/builtin_tools/mod.rs`

**Step 1: Delete files**

```bash
rm core/src/builtin_tools/snapshot_capture.rs
rm -rf core/src/builtin_tools/canvas/
```

**Step 2: Remove module declarations from builtin_tools/mod.rs**

In `core/src/builtin_tools/mod.rs`:

Delete line 39:
```rust
pub mod canvas;
```

Delete line 54:
```rust
pub mod snapshot_capture;
```

Delete line 80:
```rust
pub use snapshot_capture::SnapshotCaptureTool;
```

Delete the canvas re-export block (lines 93-100):
```rust
// Canvas tool re-exports
pub use canvas::{
    create_router as create_canvas_router, parse_jsonl, parse_message, validate_jsonl,
    A2uiMessage, A2uiParseError, BeginRendering, CanvasAction, CanvasBackend, CanvasController,
    CanvasHostConfig, CanvasHostState, CanvasState, CanvasTool, CanvasToolArgs, CanvasToolOutput,
    Component, ComponentType, DataModelUpdate, DataUpdate, EventHandler, NoOpBackend,
    SnapshotFormat, Surface, SurfaceManager, SurfaceUpdate, UserAction, WindowPlacement,
};
```

Also update the module doc comment at the top to remove `CanvasTool` reference (line 16).

**Step 3: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -50`

**Step 4: Commit**

```bash
git add -A
git commit -m "remove: snapshot_capture and canvas builtin tools"
```

---

### Task 5: Delete gateway handlers (browser, state_bus, ocr)

**Files:**
- Delete: `core/src/gateway/handlers/browser.rs`
- Delete: `core/src/gateway/handlers/state_bus.rs`
- Delete: `core/src/gateway/handlers/ocr.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Delete handler files**

```bash
rm core/src/gateway/handlers/browser.rs
rm core/src/gateway/handlers/state_bus.rs
rm core/src/gateway/handlers/ocr.rs
```

**Step 2: Remove module declarations from handlers/mod.rs**

In `core/src/gateway/handlers/mod.rs`:

Delete line 62:
```rust
pub mod ocr;
```

Delete line 91:
```rust
pub mod state_bus;
```

Delete lines 100-101:
```rust
#[cfg(feature = "browser")]
pub mod browser;
```

Also remove "ocr", "state_bus", "browser" from the doc comment table at the top.

**Step 3: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -50`

**Step 4: Commit**

```bash
git add -A
git commit -m "remove: browser, state_bus, ocr gateway handlers"
```

---

### Task 6: Delete client routing infrastructure (ReverseRpc, ClientManifest, ToolRouter, RoutedExecutor, ExecutionPolicy)

**Files:**
- Delete: `core/src/gateway/reverse_rpc.rs`
- Delete: `core/src/gateway/client_manifest.rs`
- Delete: `core/src/executor/router.rs`
- Delete: `core/src/executor/routed_executor.rs`
- Delete: `core/src/dispatcher/types/execution_policy.rs`
- Modify: `core/src/gateway/mod.rs`
- Modify: `core/src/executor/mod.rs`
- Modify: `core/src/dispatcher/types/mod.rs`
- Modify: `core/src/dispatcher/mod.rs`

**Step 1: Delete files**

```bash
rm core/src/gateway/reverse_rpc.rs
rm core/src/gateway/client_manifest.rs
rm core/src/executor/router.rs
rm core/src/executor/routed_executor.rs
rm core/src/dispatcher/types/execution_policy.rs
```

**Step 2: Remove from gateway/mod.rs**

In `core/src/gateway/mod.rs`, delete lines 107-109:
```rust
#[cfg(feature = "gateway")]
mod client_manifest;
#[cfg(feature = "gateway")]
mod reverse_rpc;
```

Delete lines 202-205:
```rust
#[cfg(feature = "gateway")]
pub use client_manifest::{ClientManifest, ClientCapabilities, ClientEnvironment, ExecutionConstraints};
#[cfg(feature = "gateway")]
pub use reverse_rpc::{ReverseRpcManager, ReverseRpcError, PendingRequest};
```

**Step 3: Remove from executor/mod.rs**

In `core/src/executor/mod.rs`:

Delete line 52:
```rust
mod router;
```

Delete lines 53-54:
```rust
#[cfg(feature = "gateway")]
mod routed_executor;
```

Delete line 66:
```rust
pub use router::{RoutingDecision, ToolRouter};
```

Delete lines 67-68:
```rust
#[cfg(feature = "gateway")]
pub use routed_executor::{RoutedExecutionError, RoutedExecutionResult, RoutedExecutor};
```

Also update the module doc comment to remove RoutedExecutor references and the Server-Client routing section.

**Step 4: Remove from dispatcher/types/mod.rs**

In `core/src/dispatcher/types/mod.rs`, delete line 22:
```rust
mod execution_policy;
```

Delete lines 61-62:
```rust
// Execution Policy (for Server-Client routing)
pub use execution_policy::ExecutionPolicy;
```

**Step 5: Remove from dispatcher/mod.rs**

In `core/src/dispatcher/mod.rs`, remove `ExecutionPolicy` from the re-export at line 106:
```rust
// Before:
    ConflictInfo, ConflictResolution, ExecutionPolicy, RoutingLayer, ...
// After:
    ConflictInfo, ConflictResolution, RoutingLayer, ...
```

**Step 6: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -80`

**Step 7: Commit**

```bash
git add -A
git commit -m "remove: client routing infrastructure (ReverseRpc, ClientManifest, ToolRouter, RoutedExecutor, ExecutionPolicy)"
```

---

### Task 7: Simplify ExecutionEngine (remove client routing)

**Files:**
- Modify: `core/src/gateway/execution_engine.rs`

**Step 1: Remove ClientContext and routing imports**

In `core/src/gateway/execution_engine.rs`:

Delete line 22 references to RoutedExecutor, ToolRouter:
```rust
// Before:
use crate::executor::{RoutedExecutor, SingleStepExecutor, ToolRegistry, ToolRouter};
// After:
use crate::executor::{SingleStepExecutor, ToolRegistry};
```

Delete line 26:
```rust
use super::{ClientManifest, JsonRpcRequest, ReverseRpcManager};
```

**Step 2: Remove ClientContext struct**

Delete the entire `ClientContext` struct and its impl (lines 49-76):
```rust
/// Client context for Server-Client routing.
#[derive(Clone)]
pub struct ClientContext { ... }
impl ClientContext { ... }
```

**Step 3: Remove client_context parameter from execute()**

In `ExecutionEngine::execute()` (line 217), remove the `client_context` parameter:
```rust
// Before:
    pub async fn execute<E: EventEmitter + Send + Sync + 'static>(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        client_context: Option<ClientContext>,
    ) -> Result<(), ExecutionError> {
// After:
    pub async fn execute<E: EventEmitter + Send + Sync + 'static>(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<(), ExecutionError> {
```

Update the `run_agent_loop` call (~line 298) to remove `client_context`:
```rust
// Before:
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
                client_context,
            ) => result,
// After:
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
            ) => result,
```

**Step 4: Simplify run_agent_loop()**

Remove `client_context` parameter from `run_agent_loop()` (~line 458):
```rust
// Before:
    async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        client_context: Option<ClientContext>,
    ) -> Result<String, ExecutionError> {
// After:
    async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
```

Replace the routing branch (lines 535-577) with direct executor usage:
```rust
// Before: if let Some(ctx) = client_context { ... } else { ... }
// After: always use local_executor
        let agent_loop = AgentLoop::new(thinker, local_executor, compressor, loop_config);
        let mut run_context = RunContext::new(
            request.input.clone(),
            context,
            allowed_tools,
            identity.clone(),
        )
        .with_abort_signal(abort_rx);
        if let Some(history) = initial_history {
            run_context = run_context.with_initial_history(history);
        }
        let result = agent_loop
            .run(run_context, callback.as_ref())
            .await;
```

Update the doc comments to remove Server-Client routing references.

**Step 5: Update ExecutionAdapter impl**

In the `ExecutionAdapter for ExecutionEngine` impl (~line 895):
```rust
// Before:
        ExecutionEngine::execute(self, request, agent, wrapper, None).await
// After:
        ExecutionEngine::execute(self, request, agent, wrapper).await
```

Also update the `ClientContext` export in `gateway/mod.rs` line 138 - remove `ClientContext`:
```rust
// Before:
pub use execution_engine::{ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine, ClientContext};
// After:
pub use execution_engine::{ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine};
```

**Step 6: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -80`

**Step 7: Commit**

```bash
git add core/src/gateway/execution_engine.rs core/src/gateway/mod.rs
git commit -m "simplify: ExecutionEngine to always use SingleStepExecutor, remove client routing"
```

---

### Task 8: Simplify gateway/server.rs (remove routing from ConnectionState)

**Files:**
- Modify: `core/src/gateway/server.rs`

**Step 1: Remove ClientManifest and ReverseRpcManager imports**

Delete line 17:
```rust
use super::ClientManifest;
```

Delete line 23:
```rust
use super::reverse_rpc::ReverseRpcManager;
```

**Step 2: Simplify ConnectionState**

Remove from the struct (~lines 43-47):
```rust
    pub manifest: Option<ClientManifest>,
    pub reverse_rpc: Option<Arc<ReverseRpcManager>>,
    pub client_sender: Option<tokio::sync::mpsc::Sender<JsonRpcRequest>>,
```

Remove `with_routing` constructor (~lines 54-70). Replace with a simple `new()`:
```rust
impl ConnectionState {
    fn new() -> Self {
        Self {
            authenticated: false,
            first_message: true,
            subscriptions: vec![],
            metadata: HashMap::new(),
            device_id: None,
            permissions: vec![],
            guest_session_id: None,
        }
    }
```

Remove methods: `set_manifest`, `supports_tool`, `has_scope`, `has_manifest`, `supports_routing`, `client_context` (~lines 80-123).

**Step 3: Simplify handle_connection()**

In `handle_connection()` (~line 374):

Remove the reverse_rpc creation (~line 389):
```rust
    let reverse_rpc = Arc::new(ReverseRpcManager::new());
```

Remove the client channel creation (~line 392-393):
```rust
    let (client_tx, mut client_rx) = tokio::sync::mpsc::channel::<JsonRpcRequest>(32);
```

Replace connection state initialization (~lines 396-402) with:
```rust
    {
        let mut conns = ctx.connections.write().await;
        conns.insert(conn_id.clone(), ConnectionState::new());
    }
```

Remove the reverse RPC response handling block (~lines 412-418):
```rust
                        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&text) {
                            if reverse_rpc.handle_response(response) {
                                debug!("Handled reverse RPC response from {}", conn_id);
                                continue;
                            }
                        }
```

Remove the `debug.tool_call` handling branch (~lines 547-554):
```rust
                                    } else if req.method == "debug.tool_call" {
                                        let resp = handle_debug_tool_call(
                                            req.clone(),
                                            &mut write,
                                            reverse_rpc.clone(),
                                        ).await;
                                        serde_json::to_string(&resp).unwrap_or_default()
```

Remove the entire client_rx select branch (~lines 711-734):
```rust
            request = client_rx.recv() => {
                ...
            }
```

Remove the `handle_debug_tool_call` function entirely (~lines 824-923).

**Step 4: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -80`

**Step 5: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "simplify: ConnectionState, remove reverse RPC from server"
```

---

### Task 9: Clean up auth handler (remove ClientManifest)

**Files:**
- Modify: `core/src/gateway/handlers/auth.rs`

**Step 1: Remove ClientManifest import**

Delete line 13:
```rust
use crate::gateway::ClientManifest;
```

**Step 2: Remove manifest from ConnectParams**

Remove from `ConnectParams` struct (~line 43):
```rust
    pub manifest: Option<ClientManifest>,
```

**Step 3: Build to verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | head -50`

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/auth.rs
git commit -m "simplify: remove ClientManifest from auth handler"
```

---

### Task 10: Fix remaining compilation errors

**Files:**
- Various files that may still reference deleted types

**Step 1: Build and identify all remaining errors**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1`

Scan all errors carefully. Common patterns to fix:
- `use crate::perception::...` → delete the import line
- `use crate::vision::...` → delete the import line
- `use crate::browser::...` → delete the import line
- `ClientManifest` references → remove
- `ReverseRpcManager` references → remove
- `RoutedExecutor` references → remove or replace with `SingleStepExecutor`
- `ExecutionPolicy` references → remove
- `PerceptionRef` references → remove
- `SnapshotCaptureTool` references → remove from tool registration
- `CanvasTool` references → remove from tool registration
- `client_context` parameters → remove
- Feature-gated `#[cfg(feature = "browser")]` blocks → delete entirely

Pay special attention to:
- `core/src/daemon/` — may reference perception/vision
- `core/src/executor/builtin_registry.rs` — may register SnapshotCaptureTool/CanvasTool
- `core/src/bin/aleph_server/` — may wire browser handlers
- `core/src/gateway/handlers/debug.rs` — references reverse_rpc types
- `shared/protocol/` — may define ExecutionPolicy

**Step 2: Fix each error iteratively**

For each error, make the minimal fix (delete import, remove field, remove registration).

**Step 3: Build to verify clean compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1`
Expected: Clean compilation (0 errors)

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test 2>&1 | tail -30`
Expected: All tests pass (some tests related to deleted modules will be gone, that's fine)

**Step 5: Commit**

```bash
git add -A
git commit -m "fix: resolve all compilation errors from module deletion"
```

---

### Task 11: Update documentation

**Files:**
- Modify: `CLAUDE.md` — update architecture diagram, remove perception/vision/browser references, update Server-Client section
- Modify: `docs/ARCHITECTURE.md` — if it references deleted modules
- Modify: `docs/TOOL_SYSTEM.md` — remove SnapshotCaptureTool and CanvasTool

**Step 1: Update CLAUDE.md**

Key changes:
- Remove `browser` from feature flags table
- Remove `ExecutionPolicy`, `ClientManifest`, `ToolRouter`, `ReverseRpcManager`, `RoutedExecutor` from Server-Client section
- Simplify the routing decision matrix — all tools execute on Server
- Remove `perception` from architecture diagrams
- Remove Canvas/Vision/Browser from subsystem table
- Remove `SnapshotCaptureTool` / `CanvasTool` from tool list

**Step 2: Add Playwright MCP config example to CLAUDE.md**

Add a section showing how to configure Playwright:
```toml
[[mcp.servers]]
name = "playwright"
command = "npx"
args = ["-y", "@anthropic/playwright-mcp", "--headless"]
```

**Step 3: Commit**

```bash
git add CLAUDE.md docs/
git commit -m "docs: update for server purification - remove desktop/browser references, add Playwright MCP"
```

---

### Task 12: Final verification

**Step 1: Full build (debug)**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo build 2>&1 | tail -10`
Expected: Successful build

**Step 2: Full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test 2>&1 | tail -30`
Expected: All remaining tests pass

**Step 3: Check for dead code warnings**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check 2>&1 | grep "warning.*dead_code\|warning.*unused" | head -20`
Expected: No new warnings related to our changes

**Step 4: Verify binary size reduction**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo build --release --bin aleph-server 2>&1 | tail -5 && ls -lh target/release/aleph-server`
Expected: Smaller binary than before (removed chromiumoxide, accessibility-sys, core-graphics)

**Step 5: Final commit if any fixes needed**

```bash
git add -A
git commit -m "chore: final cleanup after server purification"
```
