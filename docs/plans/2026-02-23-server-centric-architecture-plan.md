# Server-Centric Architecture Reframing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reframe the project from Server-Client to Server-centric architecture by renaming directories, deleting distributed execution code, and updating all documentation.

**Architecture:** Two-phase approach. Phase 1 renames directories and updates documentation (semantic refactoring). Phase 2 deletes all distributed execution infrastructure (ExecutionPolicy, ClientManifest, ReverseRpc, ToolRouter, RoutedExecutor). Each phase ends with `cargo build` + `cargo test` verification.

**Tech Stack:** Rust workspace, Cargo.toml, TOML config, Markdown docs

**Reference:** Design doc at `docs/plans/2026-02-23-server-centric-architecture-design.md`

---

## Phase 1: Semantic Refactoring

### Task 1: Rename `clients/` to `apps/`

**Files:**
- Rename: `clients/` → `apps/` (entire directory)
- Modify: `Cargo.toml:12-15` (workspace members)
- Modify: `core/Cargo.toml:222` (aleph-sdk path)
- Modify: `build-macos.sh:9` (MACOS_DIR path)

**Step 1: Rename the directory**

```bash
mv clients apps
```

**Step 2: Update workspace Cargo.toml**

In `/Volumes/TBU4/Workspace/Aleph/Cargo.toml`, replace lines 12-15:

```toml
# Before:
    "clients/cli",
    "clients/shared",
    "clients/desktop/src-tauri",
    "clients/dashboard",

# After:
    "apps/cli",
    "apps/shared",
    "apps/desktop/src-tauri",
    "apps/dashboard",
```

**Step 3: Update core Cargo.toml SDK path**

In `/Volumes/TBU4/Workspace/Aleph/core/Cargo.toml`, find and replace:

```toml
# Before:
aleph-sdk = { path = "../clients/shared" }

# After:
aleph-sdk = { path = "../apps/shared" }
```

**Step 4: Update build-macos.sh**

In `/Volumes/TBU4/Workspace/Aleph/build-macos.sh`, line 9:

```bash
# Before:
MACOS_DIR="$ROOT_DIR/clients/macos"

# After:
MACOS_DIR="$ROOT_DIR/apps/macos"
```

**Step 5: Search for any other `clients/` path references in code**

```bash
rg "clients/" --type toml --type rust --type sh -l
```

Fix any remaining references found.

**Step 6: Verify build**

```bash
cargo check --workspace
```

**Step 7: Commit**

```bash
git add -A && git commit -m "refactor: rename clients/ to apps/"
```

---

### Task 2: Rename `channels/` to `interfaces/`

**Files:**
- Rename: `core/src/gateway/channels/` → `core/src/gateway/interfaces/`
- Modify: `core/src/gateway/mod.rs:77` (mod declaration)
- Modify: All files that import from `channels`

**Step 1: Rename the directory**

```bash
mv core/src/gateway/channels core/src/gateway/interfaces
```

**Step 2: Update module declaration in gateway/mod.rs**

In `core/src/gateway/mod.rs`, line 77:

```rust
// Before:
pub mod channels;

// After:
pub mod interfaces;
```

**Step 3: Search and replace all `channels` imports**

```bash
rg "gateway::channels" --type rust -l
rg "super::channels" --type rust -l
rg "crate::gateway::channels" --type rust -l
```

Replace all occurrences:
- `gateway::channels` → `gateway::interfaces`
- `super::channels` → `super::interfaces`
- `crate::gateway::channels` → `crate::gateway::interfaces`

**Step 4: Update re-exports in lib.rs if any reference channels**

```bash
rg "channels" core/src/lib.rs
```

Fix any references found.

**Step 5: Verify build**

```bash
cargo check --workspace
```

**Step 6: Commit**

```bash
git add -A && git commit -m "refactor: rename gateway/channels/ to gateway/interfaces/"
```

---

### Task 3: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Replace the architecture diagram**

Find the old CLIENT LAYER / GATEWAY / AGENT / EXECUTION / STORAGE diagram and replace with the new Server-centric version from the design doc.

**Step 2: Remove the entire "Server-Client 模式" section**

Delete the section starting with `### 🌐 Server-Client 模式` through to the `详见：[Server-Client 架构设计]` line. This includes:
- The brain/hands diagram
- The component table (ExecutionPolicy, ClientManifest, ToolRouter, ReverseRpcManager, RoutedExecutor)
- The routing decision matrix
- The "关键架构原则：Server-Side Execution" box

**Step 3: Add new architecture principle**

Add a concise section:

```markdown
### 🏛️ Server-Centric 架构

Aleph 是一个**自包含的 AI Server**。所有感知、思考、执行都在 Server 侧完成。

外部 Interface（App、Bot、CLI）仅负责消息的输入和展示，不承担任何业务逻辑。
它们如同社交软件的对话窗口，只是输入出口。
```

**Step 4: Update project structure section**

Replace all `clients/` → `apps/`, `channels/` → `interfaces/` references in the project structure tree.

**Step 5: Update documentation index**

Remove references to server-client design docs, add reference to new server-centric design doc.

**Step 6: Commit**

```bash
git add CLAUDE.md && git commit -m "docs: update CLAUDE.md for server-centric architecture"
```

---

### Task 4: Update other documentation

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/GATEWAY.md`
- Modify: `docs/TOOL_SYSTEM.md`
- Modify: `README.md`
- Modify: `docs/plans/2026-02-06-server-client-architecture-design.md` (add SUPERSEDED header)
- Modify: `docs/plans/2026-02-06-server-client-implementation.md` (add SUPERSEDED header)

**Step 1: Update ARCHITECTURE.md**

- Replace architecture diagram with server-centric version
- Remove Server-Client routing section
- Update all `clients/` → `apps/`, `channels/` → `interfaces/` path references
- Remove ExecutionPolicy, ClientManifest, ToolRouter descriptions

**Step 2: Update GATEWAY.md**

- Remove ReverseRpc documentation
- Remove ClientManifest capability negotiation docs
- Update terminology: client → interface where referring to connections
- Update `channels/` → `interfaces/` references

**Step 3: Update TOOL_SYSTEM.md**

- Remove ExecutionPolicy documentation
- Remove ToolRouter/RoutedExecutor descriptions
- Simplify execution path: tools always execute server-side

**Step 4: Update README.md**

- Replace architecture overview diagram
- Update `clients/` → `apps/` in any structure references
- Update terminology

**Step 5: Mark old design docs as superseded**

Add to the top of each file:

```markdown
> **SUPERSEDED** by `docs/plans/2026-02-23-server-centric-architecture-design.md`
> This document describes a deprecated Server-Client architecture that has been replaced.
```

Files:
- `docs/plans/2026-02-06-server-client-architecture-design.md`
- `docs/plans/2026-02-06-server-client-implementation.md`

**Step 6: Search for remaining "Server-Client" or "client" references in docs**

```bash
rg -i "server.client" docs/ -l
rg "ClientManifest|ExecutionPolicy|ReverseRpc|ToolRouter|RoutedExecutor" docs/ -l
```

Fix any remaining references.

**Step 7: Commit**

```bash
git add docs/ README.md && git commit -m "docs: update all documentation for server-centric architecture"
```

---

## Phase 2: Code Deletion

### Task 5: Delete ExecutionPolicy

**Files:**
- Delete: `core/src/dispatcher/types/execution_policy.rs`
- Modify: `core/src/dispatcher/types/mod.rs:22,62` (remove mod + re-export)

**Step 1: Delete the file**

```bash
rm core/src/dispatcher/types/execution_policy.rs
```

**Step 2: Remove module declaration from mod.rs**

In `core/src/dispatcher/types/mod.rs`:

Remove line 22:
```rust
mod execution_policy;
```

Remove lines 61-62:
```rust
// Execution Policy (for Server-Client routing)
pub use execution_policy::ExecutionPolicy;
```

**Step 3: Find all imports of ExecutionPolicy**

```bash
rg "ExecutionPolicy" --type rust -l
```

Remove all `use` statements and usages. Key files expected:
- `core/src/gateway/config.rs:13` — `use crate::dispatcher::ExecutionPolicy;`
- `core/src/dispatcher/types/unified.rs` — field on UnifiedTool
- `core/src/executor/router.rs` — used in routing logic (will be deleted in Task 8)

**Step 4: DO NOT verify build yet** — other deletions depend on this. Continue to Task 6.

---

### Task 6: Delete ToolRoutingConfig and PolicyOverride from gateway config

**Files:**
- Modify: `core/src/gateway/config.rs:13,42-44,63,314-396,642-686` (remove routing config)

**Step 1: Remove ExecutionPolicy import**

In `core/src/gateway/config.rs`, line 13, remove:
```rust
use crate::dispatcher::ExecutionPolicy;
```

**Step 2: Remove tool_routing field from GatewayConfig**

Remove lines 42-44:
```rust
    /// Tool routing configuration for Server-Client architecture
    #[serde(default)]
    pub tool_routing: ToolRoutingConfig,
```

Remove from Default impl (line 63):
```rust
            tool_routing: ToolRoutingConfig::default(),
```

**Step 3: Delete ToolRoutingConfig struct, PolicyOverride enum, and impls**

Delete lines 314-396 (the entire `ToolRoutingConfig` struct, `PolicyOverride` enum, `From<PolicyOverride>` impl, and `ToolRoutingConfig` methods).

**Step 4: Delete related tests**

Delete the `test_tool_routing_config` test (lines 642-667) and `test_tool_routing_config_apply_to_router` test (lines 669-685).

**Step 5: Update re-exports in gateway/mod.rs**

In `core/src/gateway/mod.rs`, line 134, remove `ToolRoutingConfig` and `PolicyOverride`:

```rust
// Before:
pub use config::{GatewayConfig, ToolRoutingConfig, PolicyOverride};

// After:
pub use config::GatewayConfig;
```

**Step 6: Search for remaining ToolRoutingConfig references**

```bash
rg "ToolRoutingConfig|PolicyOverride|tool_routing" --type rust -l
```

Fix any remaining references.

---

### Task 7: Delete ClientManifest and ReverseRpcManager

**Files:**
- Delete: `core/src/gateway/client_manifest.rs`
- Delete: `core/src/gateway/reverse_rpc.rs`
- Modify: `core/src/gateway/mod.rs:107-109,202-205` (remove mod + re-exports)

**Step 1: Delete the files**

```bash
rm core/src/gateway/client_manifest.rs
rm core/src/gateway/reverse_rpc.rs
```

**Step 2: Remove module declarations from gateway/mod.rs**

Remove lines 106-109:
```rust
#[cfg(feature = "gateway")]
mod client_manifest;
#[cfg(feature = "gateway")]
mod reverse_rpc;
```

Remove lines 202-205:
```rust
#[cfg(feature = "gateway")]
pub use client_manifest::{ClientManifest, ClientCapabilities, ClientEnvironment, ExecutionConstraints};
#[cfg(feature = "gateway")]
pub use reverse_rpc::{ReverseRpcManager, ReverseRpcError, PendingRequest};
```

**Step 3: Search for all imports**

```bash
rg "ClientManifest|ClientCapabilities|ClientEnvironment|ExecutionConstraints" --type rust -l
rg "ReverseRpcManager|ReverseRpcError|PendingRequest" --type rust -l
```

Remove all found `use` statements and usages.

---

### Task 8: Delete ToolRouter and RoutedExecutor

**Files:**
- Delete: `core/src/executor/router.rs`
- Delete: `core/src/executor/routed_executor.rs`
- Modify: `core/src/executor/mod.rs:52-54,66-68` (remove mod + re-exports)

**Step 1: Delete the files**

```bash
rm core/src/executor/router.rs
rm core/src/executor/routed_executor.rs
```

**Step 2: Update executor/mod.rs**

Remove line 52:
```rust
mod router;
```

Remove lines 53-54:
```rust
#[cfg(feature = "gateway")]
mod routed_executor;
```

Remove line 66:
```rust
pub use router::{RoutingDecision, ToolRouter};
```

Remove lines 67-68:
```rust
#[cfg(feature = "gateway")]
pub use routed_executor::{RoutedExecutionError, RoutedExecutionResult, RoutedExecutor};
```

**Step 3: Update the module doc comment**

In `core/src/executor/mod.rs`, remove lines 13 and 31-47 referencing RoutedExecutor and Server-Client routing.

**Step 4: Search for remaining references**

```bash
rg "ToolRouter|RoutedExecutor|RoutingDecision|RoutedExecution" --type rust -l
```

Remove all found references.

---

### Task 9: Clean up ConnectionState and ExecutionEngine

**Files:**
- Modify: `core/src/gateway/server.rs:17,23,42-47,52-70,79-80+`
- Modify: `core/src/gateway/execution_engine.rs:22,26,49-76,138`

**Step 1: Clean up server.rs — remove imports**

Remove line 17:
```rust
use super::ClientManifest;
```

Remove line 23:
```rust
use super::reverse_rpc::ReverseRpcManager;
```

**Step 2: Clean up server.rs — remove ConnectionState fields**

Remove these fields from the `ConnectionState` struct (lines 42-47):
```rust
    /// Client capability manifest (set during connect if provided)
    pub manifest: Option<ClientManifest>,
    /// Reverse RPC manager for Server-Client tool routing
    pub reverse_rpc: Option<Arc<ReverseRpcManager>>,
    /// Channel sender for sending requests to the client
    pub client_sender: Option<tokio::sync::mpsc::Sender<JsonRpcRequest>>,
```

**Step 3: Clean up server.rs — remove with_routing constructor**

Delete the `with_routing()` method (lines 53-70). Find the `new()` or default constructor and ensure it doesn't reference deleted fields.

**Step 4: Clean up server.rs — remove set_manifest and related methods**

Search for and remove:
- `set_manifest()` method
- `supports_tool()` method
- `has_manifest()` method
- Any other method that references `manifest`, `reverse_rpc`, or `client_sender`

**Step 5: Clean up execution_engine.rs — remove ClientContext**

In `core/src/gateway/execution_engine.rs`:

Remove import line 22:
```rust
use crate::executor::{RoutedExecutor, SingleStepExecutor, ToolRegistry, ToolRouter};
```
Replace with:
```rust
use crate::executor::{SingleStepExecutor, ToolRegistry};
```

Remove import line 26:
```rust
use super::{ClientManifest, JsonRpcRequest, ReverseRpcManager};
```
Replace with (keep only what's still needed):
```rust
use super::JsonRpcRequest;
```

Delete the entire `ClientContext` struct and impl (lines 49-76).

**Step 6: Search for ClientContext usage in execution_engine.rs and fix**

```bash
rg "ClientContext|client_context|with_client|RoutedExecutor|ToolRouter" core/src/gateway/execution_engine.rs
```

Remove all references. Simplify execution to use `SingleStepExecutor` directly instead of `RoutedExecutor`.

**Step 7: Update re-export in gateway/mod.rs**

Line 138, remove `ClientContext` from the export:
```rust
// Before:
pub use execution_engine::{ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine, ClientContext};

// After:
pub use execution_engine::{ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine};
```

---

### Task 10: Clean up CLI and shared client code

**Files:**
- Delete: `apps/cli/src/executor.rs` (LocalExecutor)
- Delete: `apps/shared/src/executor.rs` (LocalExecutor trait)
- Modify: `apps/cli/src/main.rs` or `apps/cli/src/lib.rs` (remove executor module)
- Modify: `apps/cli/src/client.rs` (remove tool.call handler)
- Modify: `apps/shared/src/lib.rs` (remove executor module)

**Step 1: Delete executor files**

```bash
rm apps/cli/src/executor.rs
rm apps/shared/src/executor.rs
```

**Step 2: Remove module declarations**

Search for `mod executor` in CLI and shared crates:
```bash
rg "mod executor" apps/ --type rust
```

Remove all `mod executor` and `pub mod executor` declarations.

**Step 3: Clean up CLI client.rs**

In `apps/cli/src/client.rs`, search for `tool.call` or reverse RPC handler code:
```bash
rg "tool.call|reverse_rpc|LocalExecutor|executor" apps/cli/src/client.rs
```

Remove the reverse RPC handling logic.

**Step 4: Search for any remaining references**

```bash
rg "LocalExecutor|executor" apps/ --type rust -l
```

Fix any remaining references.

---

### Task 11: Clean up shared protocol crate

**Files:**
- Delete: `shared/protocol/src/policy.rs` (if it exists)
- Delete: `shared/protocol/src/manifest.rs` (if it exists)
- Modify: `shared/protocol/src/lib.rs` (remove module declarations)

**Step 1: Check if these files exist**

```bash
ls shared/protocol/src/policy.rs shared/protocol/src/manifest.rs 2>/dev/null
```

**Step 2: If they exist, delete them**

```bash
rm -f shared/protocol/src/policy.rs shared/protocol/src/manifest.rs
```

**Step 3: Update lib.rs**

```bash
rg "mod policy|mod manifest|ExecutionPolicy|ClientManifest" shared/protocol/src/
```

Remove all module declarations and re-exports.

---

### Task 12: Final verification and commit

**Step 1: Full workspace build**

```bash
cargo build --workspace 2>&1
```

Fix any compilation errors.

**Step 2: Run all tests**

```bash
cargo test --workspace 2>&1
```

Fix any test failures.

**Step 3: Search for leftover references**

```bash
rg "ExecutionPolicy|ClientManifest|ReverseRpc|ToolRouter|RoutedExecutor|LocalExecutor|tool_routing|PolicyOverride|ClientContext|client_manifest|reverse_rpc" --type rust
```

Clean up any remaining references.

**Step 4: Search for stale doc references**

```bash
rg "Server-Client|server.client" docs/ CLAUDE.md README.md --ignore-case
```

Ensure no misleading references remain (except in SUPERSEDED docs).

**Step 5: Commit all Phase 2 changes**

```bash
git add -A && git commit -m "refactor: remove distributed execution infrastructure (ExecutionPolicy, ClientManifest, ReverseRpc, ToolRouter, RoutedExecutor)"
```

---

## Verification Checklist

After all tasks are complete, verify:

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] No `clients/` directory exists (renamed to `apps/`)
- [ ] No `core/src/gateway/channels/` exists (renamed to `interfaces/`)
- [ ] No files named `execution_policy.rs`, `client_manifest.rs`, `reverse_rpc.rs`, `router.rs` (in executor), `routed_executor.rs` exist
- [ ] `rg "ExecutionPolicy|ClientManifest|ReverseRpc|ToolRouter|RoutedExecutor" --type rust` returns nothing
- [ ] CLAUDE.md has updated architecture diagram and no Server-Client section
- [ ] Old design docs are marked SUPERSEDED
