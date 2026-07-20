# Memory Extension Lifecycle Wiring (X1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `MemoryExtension` lifecycle real end-to-end — bind the MCP caller (Task 11), fire `on_delegation` + `on_pre_compress`, and keep the unfireable `on_session_switch` as an honest API-only hook.

**Architecture:** Three components. C1 binds `UnboundMcpCaller` → a real `McpManagerHandle`-backed caller via `ArcSwap` interior mutability, with the server id resolved at registration. C2/C3 add two `dispatch_*` call sites into already-threaded paths (`subagent_tool` completion, `compress_to_notes`). `on_session_switch` keeps its trait/dispatch/MCP-adapter surface but gets no producer.

**Tech Stack:** Rust, `arc-swap` (already a dep), `tokio`, `async-trait`, MCP actor (`McpManagerHandle`).

---

## ⚠️ PROJECT PROTOCOL — READ FIRST

- **Worktree only.** All code changes happen in worktree branch `fix/memory-extension-lifecycle-x1` off `main`. Never edit `main` directly (this plan doc is the only thing already on main).
- **Do NOT run `cargo check` / `cargo test` / `cargo build` at any point.** This is the user's mandatory "资源并发治理" constraint. Commit directly after each task.
- **Grep caller-verification guards substitute for the compiler.** Each task lists exact `grep` commands that must return the expected matches before commit. They are how we prove no caller/impl was missed without compiling.
- **Append-only git.** `git add <explicit paths>` then `git commit`. Never `git add -A`, never `reset`/`amend`/`rebase` (main is shared with concurrent sessions).
- **Non-destructive / entropy-reducing.** Default behavior must be byte-identical when no MCP `[memory]` plugin is installed and no extension is registered.

> **Refinement vs spec §3-C1.3:** `replace_caller(name, caller)` is dropped as YAGNI. `bind_memory_callers` iterates a registry snapshot and calls `McpMemoryExtension::rebind` directly, so a name-keyed replace is unnecessary. The registry instead exposes `mcp_bindings_snapshot()`. Server id is resolved at registration (where loader access exists) and stored on the extension.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `src/memory/extensions/mcp_adapter.rs` | `McpMemoryExtension` interior mutability (`ArcSwap` caller + `server_id`), `rebind`, `new_unbound`, real `ManagerBackedMcpCaller` | T1 |
| `src/memory/extensions/registry.rs` | typed `mcp_bindings` side-table, `register_mcp`, `mcp_bindings_snapshot`; session_switch TODO comment | T2 |
| `src/extension/loader.rs` | resolve server id + register via `register_mcp` | T3 |
| `src/extension/mod.rs` | `bind_memory_callers()` (iterate snapshot, build real caller, rebind) | T4 |
| `src/bin/aleph-server/commands/start/mod.rs` + `src/extension/plugin_ops.rs` | cold-boot bind + hot-load bind | T5 |
| `src/agents/subagent_tool/spawn.rs` | `on_delegation` dispatch on child completion | T6 |
| `src/memory/notes/ingest/ingestor.rs` + mocks in `src/memory/compression/service.rs` | `ingest_batch` gains `extra_context: Option<&str>` | T7 |
| `src/memory/compression/service.rs` + `src/bin/aleph-server/commands/start/builder/handlers/memory.rs` | `with_extension_registry` + `on_pre_compress` dispatch in `compress_to_notes` + boot wiring | T8 |

---

## Task 1: C1 — `McpMemoryExtension` interior mutability + `ManagerBackedMcpCaller`

**Files:**
- Modify: `src/memory/extensions/mcp_adapter.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/memory/extensions/mcp_adapter.rs`:

```rust
    #[tokio::test]
    async fn rebind_swaps_caller_visible_to_hooks() {
        // Starts unbound → on_capture errors. After rebind to a canned caller
        // that allows, on_capture returns Allow. Proves ArcSwap visibility.
        let ext = McpMemoryExtension::new_unbound("p".to_string(), Some("plugin:p/srv".to_string()));
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let mut r = raw();
        // Unbound: errors.
        assert!(ext.on_capture(&ctx, &mut r).await.is_err());
        // Rebind to a caller that returns empty (Allow).
        let caller = Arc::new(CannedCaller::new(vec![("memory.on_capture", json!({}))]));
        ext.rebind(caller);
        let d = ext.on_capture(&ctx, &mut r).await.unwrap();
        assert!(matches!(d, CaptureDecision::Allow));
        assert_eq!(ext.server_id(), Some("plugin:p/srv"));
    }

    #[tokio::test]
    async fn manager_backed_caller_maps_success_content() {
        // ManagerBackedMcpCaller maps McpToolResult{success,content} → content Value.
        // Uses a stub that bypasses the real handle by testing the mapping helper.
        let ok = crate::mcp::types::McpToolResult::success(json!({"text": "hi"}));
        let mapped = ManagerBackedMcpCaller::map_result(ok).unwrap();
        assert_eq!(mapped, json!({"text": "hi"}));
        let err = crate::mcp::types::McpToolResult::error("boom");
        assert!(ManagerBackedMcpCaller::map_result(err).is_err());
    }
```

- [ ] **Step 2: Implement — convert `caller` to `ArcSwap`, add `server_id`, `rebind`, `new_unbound`**

Replace the imports + struct + impl at the top of `src/memory/extensions/mcp_adapter.rs` (currently lines 11-35):

```rust
use crate::memory::store::raw_memory::RawMemory;
use crate::sync_primitives::Arc;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Minimal trait the adapter needs to talk to a plugin. Tests use an
/// in-memory implementation; production wires it to the real MCP client.
#[async_trait]
pub trait McpCaller: Send + Sync {
    async fn call(&self, method: &str, args: Value) -> Result<Value, AlephError>;
}

pub struct McpMemoryExtension {
    name: String,
    /// `Some` when created unbound by the plugin loader (drives the boot-time
    /// rebind to a real `ManagerBackedMcpCaller`). `None` for test-constructed
    /// extensions that are handed a concrete caller up front.
    server_id: Option<String>,
    /// Swappable so the boot-time bind can replace `UnboundMcpCaller` with the
    /// real MCP-backed caller without re-registering. Dispatch reads via `.load()`.
    caller: ArcSwap<dyn McpCaller>,
}

impl McpMemoryExtension {
    /// Construct with a concrete caller (already bound). `server_id` is `None`,
    /// so the boot-time bind pass skips it.
    pub fn new(name: impl Into<String>, caller: Arc<dyn McpCaller>) -> Self {
        Self {
            name: name.into(),
            server_id: None,
            caller: ArcSwap::from(caller),
        }
    }

    /// Construct unbound: backed by `UnboundMcpCaller` until `rebind` replaces
    /// it. `server_id` (when `Some`) is the MCP server that
    /// `bind_memory_callers` will route this plugin's hook calls to.
    pub fn new_unbound(name: String, server_id: Option<String>) -> Self {
        let caller: Arc<dyn McpCaller> = Arc::new(UnboundMcpCaller::new(name.clone()));
        Self {
            name,
            server_id,
            caller: ArcSwap::from(caller),
        }
    }

    /// Swap the underlying caller. Visible to every subsequent hook dispatch.
    pub fn rebind(&self, caller: Arc<dyn McpCaller>) {
        self.caller.store(caller);
    }

    /// The MCP server id this extension's hooks route to, if resolved at
    /// registration. `None` means "leave bound to whatever caller it has".
    pub fn server_id(&self) -> Option<&str> {
        self.server_id.as_deref()
    }
}
```

- [ ] **Step 3: Implement — every hook reads `self.caller.load()`**

In the `#[async_trait] impl MemoryExtension for McpMemoryExtension` block, every `self.caller.call(...)` becomes `self.caller.load().call(...)`. There are 6 call sites (on_retrieve, on_capture, produce, on_session_switch, on_pre_compress, on_delegation). Example for `on_retrieve` (line ~54):

```rust
        let resp = self.caller.load().call("memory.on_retrieve", args).await?;
```

Apply the same `.load()` insertion to all 6 (`memory.on_capture`, `memory.produce`, `memory.on_session_switch`, `memory.on_pre_compress`, `memory.on_delegation`).

- [ ] **Step 4: Implement — `ManagerBackedMcpCaller`**

Add after the `UnboundMcpCaller` impl block (after current line 196):

```rust
/// Real `McpCaller` backed by the live MCP manager. Routes each hook method
/// call to the plugin's MCP server via `McpManagerHandle::get_client` →
/// `McpClient::call_tool`. Constructed at boot by `bind_memory_callers` once
/// the manager handle is available.
pub struct ManagerBackedMcpCaller {
    handle: crate::mcp::McpManagerHandle,
    server_id: String,
}

impl ManagerBackedMcpCaller {
    pub fn new(handle: crate::mcp::McpManagerHandle, server_id: impl Into<String>) -> Self {
        Self {
            handle,
            server_id: server_id.into(),
        }
    }

    /// Map an `McpToolResult` to the inner JSON the hook adapters expect.
    /// Success → the `content` Value; failure → an error. Extracted so the
    /// mapping is unit-testable without a live handle.
    pub(crate) fn map_result(
        res: crate::mcp::types::McpToolResult,
    ) -> Result<Value, AlephError> {
        if res.success {
            Ok(res.content)
        } else {
            Err(AlephError::other(
                res.error.unwrap_or_else(|| "mcp memory tool call failed".to_string()),
            ))
        }
    }
}

#[async_trait]
impl McpCaller for ManagerBackedMcpCaller {
    async fn call(&self, method: &str, args: Value) -> Result<Value, AlephError> {
        let client = self
            .handle
            .get_client(&self.server_id)
            .await?
            .ok_or_else(|| {
                AlephError::other(format!(
                    "memory MCP server '{}' not running (method={method})",
                    self.server_id
                ))
            })?;
        let res = client
            .call_tool(method, args)
            .await
            .map_err(|e| AlephError::other(format!("mcp call_tool '{method}' failed: {e}")))?;
        Self::map_result(res)
    }
}
```

- [ ] **Step 5: Grep guards (compiler substitute)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
# All 6 hook calls go through .load() — must be 6, zero bare self.caller.call(
grep -c "self.caller.load().call(" src/memory/extensions/mcp_adapter.rs   # expect 6
grep -c "self.caller.call(" src/memory/extensions/mcp_adapter.rs           # expect 0
# New symbols present
grep -n "struct ManagerBackedMcpCaller\|fn new_unbound\|fn rebind\|fn map_result\|ArcSwap" src/memory/extensions/mcp_adapter.rs
# new(name, caller) signature unchanged → existing tests' McpMemoryExtension::new calls still valid
grep -c "McpMemoryExtension::new(" src/memory/extensions/mcp_adapter.rs    # expect >=8 (unchanged callers)
```

Expected: first `6`, second `0`, third shows all four new symbols, fourth `>=8`.

- [ ] **Step 6: Export `ManagerBackedMcpCaller`**

In `src/memory/extensions/mod.rs` line 18, extend the re-export:

```rust
pub use mcp_adapter::{ManagerBackedMcpCaller, McpCaller, McpMemoryExtension, UnboundMcpCaller};
```

Guard:
```bash
grep -n "ManagerBackedMcpCaller" src/memory/extensions/mod.rs   # expect 1
```

- [ ] **Step 7: Commit**

```bash
git add src/memory/extensions/mcp_adapter.rs src/memory/extensions/mod.rs
git commit -m "memory: ArcSwap caller + ManagerBackedMcpCaller for extension binding (C1)"
```

---

## Task 2: C1 — registry typed side-table + `register_mcp` + snapshot

**Files:**
- Modify: `src/memory/extensions/registry.rs`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/memory/extensions/registry.rs`:

```rust
    #[tokio::test]
    async fn register_mcp_appears_in_both_dispatch_and_snapshot() {
        use crate::memory::extensions::mcp_adapter::McpMemoryExtension;
        let reg = MemoryExtensionRegistry::new();
        let ext = Arc::new(McpMemoryExtension::new_unbound(
            "p".to_string(),
            Some("plugin:p/srv".to_string()),
        ));
        reg.register_mcp(ext);
        // Visible to dispatch (main list).
        assert_eq!(reg.len(), 1);
        // Visible to the typed side-table for binding.
        let snap = reg.mcp_bindings_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].server_id(), Some("plugin:p/srv"));
    }
```

- [ ] **Step 2: Implement — add the side-table field**

In `src/memory/extensions/registry.rs`, extend the struct (currently lines 35-39) and its `Default`/`new`/`Clone`:

```rust
#[derive(Default)]
pub struct MemoryExtensionRegistry {
    /// Extensions in registration order (for on_capture this is the chain order).
    extensions: RwLock<Vec<Arc<dyn MemoryExtension>>>,
    /// Typed side-table of MCP-backed extensions, retained at their concrete
    /// type so the boot-time bind pass can call `rebind`. Each entry is the
    /// SAME `Arc` as the corresponding `dyn MemoryExtension` in `extensions`,
    /// so a rebind is immediately visible to dispatch.
    mcp_bindings: RwLock<Vec<Arc<crate::memory::extensions::mcp_adapter::McpMemoryExtension>>>,
}
```

Update `new()` (lines 55-59):

```rust
    pub fn new() -> Self {
        Self {
            extensions: RwLock::new(Vec::new()),
            mcp_bindings: RwLock::new(Vec::new()),
        }
    }
```

Update `Clone` (lines 41-52) to also snapshot the side-table:

```rust
impl Clone for MemoryExtensionRegistry {
    fn clone(&self) -> Self {
        let snapshot = self
            .extensions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mcp = self
            .mcp_bindings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Self {
            extensions: RwLock::new(snapshot),
            mcp_bindings: RwLock::new(mcp),
        }
    }
}
```

- [ ] **Step 3: Implement — `register_mcp` + `mcp_bindings_snapshot`**

Add these methods inside `impl MemoryExtensionRegistry` (after `register`, ~line 67):

```rust
    /// Register an MCP-backed extension. It lands in BOTH the dispatch list
    /// (as `dyn MemoryExtension`) and the typed side-table (as the concrete
    /// `McpMemoryExtension`), sharing one `Arc` so a later `rebind` on the
    /// side-table entry is visible to dispatch.
    pub fn register_mcp(
        &self,
        ext: Arc<crate::memory::extensions::mcp_adapter::McpMemoryExtension>,
    ) {
        self.extensions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(ext.clone() as Arc<dyn MemoryExtension>);
        self.mcp_bindings
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(ext);
    }

    /// Snapshot the MCP-backed extensions for the boot-time bind pass. The
    /// lock is released before the caller does any async work.
    pub fn mcp_bindings_snapshot(
        &self,
    ) -> Vec<Arc<crate::memory::extensions::mcp_adapter::McpMemoryExtension>> {
        self.mcp_bindings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
```

- [ ] **Step 4: Update the `on_session_switch` wire-point TODO comment (honest)**

Replace the doc comment above `dispatch_on_session_switch` (currently lines 166-171) with:

```rust
    /// on_session_switch: sequential broadcast. Failures and timeouts are
    /// logged and skipped — they never block a session rotation.
    ///
    /// **No Aleph-side producer (by design, X1).** Aleph sessions are created
    /// fresh, compacted in place, or deleted — none rotate a session id
    /// mid-process, so there is no event matching this hook's contract. The
    /// hook stays part of the extension API surface (third-party MCP `[memory]`
    /// plugins may implement `memory.on_session_switch`); wire an Aleph
    /// producer here only if a real session-rotation event is introduced.
```

- [ ] **Step 5: Grep guards**

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n "fn register_mcp\|fn mcp_bindings_snapshot\|mcp_bindings:" src/memory/extensions/registry.rs   # expect 3 defs + field
grep -c "mcp_bindings: RwLock::new" src/memory/extensions/registry.rs   # expect 2 (new + Clone)
grep -n "No Aleph-side producer" src/memory/extensions/registry.rs       # expect 1
```

- [ ] **Step 6: Commit**

```bash
git add src/memory/extensions/registry.rs
git commit -m "memory: registry mcp_bindings side-table + register_mcp/snapshot (C1)"
```

---

## Task 3: C1 — loader resolves server id and registers via `register_mcp`

**Files:**
- Modify: `src/extension/loader.rs`

- [ ] **Step 1: Update `register_memory_extension_if_declared` to take a server id and use `register_mcp`**

Replace the function (currently lines 401-414):

```rust
pub(crate) fn register_memory_extension_if_declared(
    manifest: &PluginManifest,
    server_id: Option<String>,
    registry: &Arc<MemoryExtensionRegistry>,
) {
    if manifest.memory_manifest.is_some() {
        let ext = McpMemoryExtension::new_unbound(manifest.name.clone(), server_id);
        registry.register_mcp(Arc::new(ext));
        info!(
            plugin = %manifest.name,
            "registered McpMemoryExtension (unbound) for plugin with [memory] section"
        );
    }
}
```

- [ ] **Step 2: Update `load_plugin_with_memory` to resolve the server id from the loader's MCP configs**

Replace the method body (currently lines 190-199):

```rust
    pub fn load_plugin_with_memory(
        &mut self,
        manifest: &PluginManifest,
        registry: &mut PluginRegistry,
        memory_registry: &Arc<MemoryExtensionRegistry>,
    ) -> ExtensionResult<()> {
        self.load_plugin(manifest, registry)?;
        // Resolve the plugin's MCP server id (memory hooks route there). A
        // memory plugin is expected to declare exactly one server; if it
        // declares several, use the first and warn.
        let server_id = self.mcp_configs.get(&manifest.id).and_then(|servers| {
            let mut keys = servers.keys();
            let first = keys.next().cloned();
            if keys.next().is_some() {
                warn!(
                    plugin = %manifest.id,
                    "plugin declares >1 MCP server; routing [memory] hooks to the first"
                );
            }
            first
        });
        register_memory_extension_if_declared(manifest, server_id, memory_registry);
        Ok(())
    }
```

- [ ] **Step 3: Update the in-file test caller**

The test at line ~534 calls `register_memory_extension_if_declared`. Find it and add the `server_id` arg. The call currently looks like `register_memory_extension_if_declared(&m, &registry)` (or via `load_plugin_with_memory`). If it calls the free function directly, change to:

```rust
        register_memory_extension_if_declared(&m, Some("plugin:test/srv".to_string()), &registry);
```

- [ ] **Step 4: Grep guards (every caller updated to 3 args)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
# Free-function definition takes server_id
grep -n "fn register_memory_extension_if_declared" src/extension/loader.rs
# No caller still uses the old 2-arg form (manifest, registry) — every call has the server_id middle arg.
grep -rn "register_memory_extension_if_declared(" src --include="*.rs"
# new_unbound is what the loader constructs now
grep -n "McpMemoryExtension::new_unbound" src/extension/loader.rs   # expect 1
grep -c "register_mcp" src/extension/loader.rs                      # expect >=1
```

Manually confirm every `register_memory_extension_if_declared(` call in the grep output passes three arguments.

- [ ] **Step 5: Commit**

```bash
git add src/extension/loader.rs
git commit -m "extension: resolve server id + register memory ext via register_mcp (C1)"
```

---

## Task 4: C1 — `ExtensionManager::bind_memory_callers`

**Files:**
- Modify: `src/extension/mod.rs`

- [ ] **Step 1: Implement `bind_memory_callers`**

Add a method on `impl ExtensionManager` (near `set_mcp_handle`, after ~line 285):

```rust
    /// Bind every registered MCP-backed memory extension to the live MCP
    /// manager. Idempotent: re-binding an already-bound extension just re-stores
    /// the caller. No-op unless BOTH the MCP handle and the memory registry are
    /// present (CLI/test paths leave them unset). Call once at boot after
    /// `set_mcp_handle` + `set_memory_registry` + plugin load, and again after
    /// hot-loading a plugin.
    pub async fn bind_memory_callers(&self) {
        use crate::memory::extensions::ManagerBackedMcpCaller;
        let handle = {
            let g = self.mcp_handle.read().unwrap_or_else(|e| e.into_inner());
            match g.as_ref() {
                Some(h) => h.clone(),
                None => return,
            }
        };
        let registry = {
            let g = self.memory_registry.read().unwrap_or_else(|e| e.into_inner());
            match g.as_ref() {
                Some(r) => r.clone(),
                None => return,
            }
        };
        for ext in registry.mcp_bindings_snapshot() {
            if let Some(server_id) = ext.server_id() {
                let caller = crate::sync_primitives::Arc::new(ManagerBackedMcpCaller::new(
                    handle.clone(),
                    server_id.to_string(),
                ));
                ext.rebind(caller);
                tracing::info!(server_id = %server_id, "bound memory MCP caller");
            }
        }
    }
```

- [ ] **Step 2: Grep guards**

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n "fn bind_memory_callers" src/extension/mod.rs            # expect 1
grep -n "mcp_bindings_snapshot\|ManagerBackedMcpCaller\|\.rebind(" src/extension/mod.rs   # expect each present
```

- [ ] **Step 3: Commit**

```bash
git add src/extension/mod.rs
git commit -m "extension: bind_memory_callers binds MCP memory extensions to live handle (C1)"
```

---

## Task 5: C1 — boot wiring (cold) + hot-load binding

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs:858-859`
- Modify: `src/extension/plugin_ops.rs:58`, `:125`

- [ ] **Step 1: Cold-boot bind after the MCP handle is set**

In `src/bin/aleph-server/commands/start/mod.rs`, right after lines 858-859:

```rust
                em.set_mcp_handle(handle);
                let n = em.sync_mcp_plugin_servers().await;
```

append:

```rust
                // X1: now that both the MCP handle and (from agent_init) the
                // memory registry are set and plugins are loaded, bind every
                // MCP-backed memory extension's caller to the live manager.
                em.bind_memory_callers().await;
```

- [ ] **Step 2: Hot-load bind in `ensure_plugin_loaded`**

In `src/extension/plugin_ops.rs`, after the `load_plugin_with_memory` call at line ~58 (inside the block that holds the loader lock — add AFTER the lock guard is dropped). Locate:

```rust
            loader.load_plugin_with_memory(&manifest, &mut registry, mem_reg)?;
```

After the surrounding lock scope closes (so we don't hold the loader write lock across the await), add:

```rust
        // X1: bind the just-loaded plugin's memory caller if the MCP handle is
        // already live (idempotent for already-bound extensions).
        self.bind_memory_callers().await;
```

- [ ] **Step 3: Hot-load bind in `load_runtime_plugin`**

In `src/extension/plugin_ops.rs`, the `load_runtime_plugin` method calls `load_plugin_with_memory` at line ~125. After that method's loader-lock scope closes, add the same call:

```rust
        // X1: bind memory caller for the hot-loaded plugin (idempotent).
        self.bind_memory_callers().await;
```

- [ ] **Step 4: Grep guards**

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n "bind_memory_callers().await" src/bin/aleph-server/commands/start/mod.rs   # expect 1
grep -c "bind_memory_callers().await" src/extension/plugin_ops.rs                  # expect 2
```

Manually confirm in `plugin_ops.rs` that each `bind_memory_callers().await` is OUTSIDE any `loader`/`registry` write-lock scope (no lock held across the await).

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/commands/start/mod.rs src/extension/plugin_ops.rs
git commit -m "extension: wire bind_memory_callers at cold boot + hot load (C1)"
```

---

## Task 6: C2 — `on_delegation` dispatch on subagent completion

**Files:**
- Modify: `src/agents/subagent_tool/spawn.rs:85-107`

- [ ] **Step 1: Clone the dispatch inputs before the spawn**

In `src/agents/subagent_tool/spawn.rs`, just before `let tracker = self.background_tracker.clone();` (line ~83), add:

```rust
        // X1 C2: capture on_delegation inputs before the task/registry are
        // moved into the spawned future.
        let deleg_registry = self.capture_registry.clone();
        let deleg_parent_agent_id = self.parent_agent_id.clone();
        let deleg_parent_session_id = self.parent_session_id.clone();
        let deleg_task = task.clone();
```

- [ ] **Step 2: Dispatch inside the spawned task, before `mark_completed`**

In the same file, the spawned future builds `outcome` then calls `tracker.mark_completed(&rid, outcome);` (line ~106). Insert the dispatch between them:

```rust
            // X1 C2: notify memory extensions that a delegated child finished.
            // Fire-and-forget; dispatch has its own per-hook timeout + warn.
            if let Some(reg) = deleg_registry {
                let result_summary = match &outcome {
                    CompletedOutcome::Ok { final_text, .. } => final_text.clone(),
                    CompletedOutcome::Err(e) => format!("(error) {e}"),
                };
                let ctx = crate::memory::extensions::types::DelegationCtx {
                    agent_id: deleg_parent_agent_id,
                    namespace: crate::memory::namespace::NamespaceScope::Owner,
                    parent_session_id: deleg_parent_session_id.unwrap_or_default(),
                    child_session_id: rid.clone(),
                    task: deleg_task,
                    result_summary,
                };
                reg.dispatch_on_delegation(&ctx).await;
            }
            tracker.mark_completed(&rid, outcome);
```

(Replace the existing bare `tracker.mark_completed(&rid, outcome);` line with the block above.)

- [ ] **Step 3: Grep guards**

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n "dispatch_on_delegation\|DelegationCtx\|deleg_registry" src/agents/subagent_tool/spawn.rs   # expect present
# Inputs cloned before spawn (must appear before the tokio::spawn line)
grep -n "let deleg_task = task.clone();\|tokio::spawn" src/agents/subagent_tool/spawn.rs
# outcome read before move into mark_completed
grep -n "match &outcome\|mark_completed(&rid, outcome)" src/agents/subagent_tool/spawn.rs
```

Manually confirm the four `deleg_*` clones appear at a line number BEFORE the `tokio::spawn` line, and `match &outcome` appears before `mark_completed(&rid, outcome)`.

- [ ] **Step 4: Commit**

```bash
git add src/agents/subagent_tool/spawn.rs
git commit -m "agents: fire on_delegation when a subagent run completes (C2)"
```

---

## Task 7: C3 — `ingest_batch` gains `extra_context: Option<&str>`

**Files:**
- Modify: `src/memory/notes/ingest/ingestor.rs` (trait + `DefaultCompoundIngestor` impl + in-file mock)
- Modify: `src/memory/compression/service.rs` (3 test mock impls at lines ~682, ~755, ~791)

- [ ] **Step 1: Add the parameter to the trait method**

In `src/memory/notes/ingest/ingestor.rs`, find the `trait CompoundIngestor` definition and update `ingest_batch`'s signature to add a trailing `extra_context: Option<&str>` parameter. The trait method becomes:

```rust
    async fn ingest_batch(
        &self,
        agent_id: &str,
        raws: Vec<crate::memory::store::raw_memory::RawMemory>,
        extra_context: Option<&str>,
    ) -> Result<ApplyReport, AlephError>;
```

- [ ] **Step 2: Thread `extra_context` through the real impl into `plan()`**

The LLM prompt is NOT built inline in `ingest_batch` — it is built inside the concrete method `DefaultCompoundIngestor::plan()` (ingestor.rs:58). So `ingest_batch` must pass `extra_context` down to `plan()`, and `plan()` prepends it to the user prompt.

First, update the real `impl ... CompoundIngestor for DefaultCompoundIngestor<S>` signature (line ~296) to add the `extra_context: Option<&str>` param, then thread it into BOTH `self.plan(...)` calls inside `ingest_batch` (the primary at line ~346 and the HashConflict re-plan at line ~382):

```rust
        let mut plan = self.plan(agent_id, &raws, &related, &source, extra_context).await?;
```
```rust
                let mut plan2 = self.plan(agent_id, &augmented, &related, &source, extra_context).await?;
```

Then update the `plan()` method signature (ingestor.rs:58-64) to accept the param and prepend it to the user prompt (after line 75 `let user = build_user_prompt(...)`):

```rust
    pub async fn plan(
        &self,
        _agent_id: &str,
        raws: &[crate::memory::store::raw_memory::RawMemory],
        related: &[RelatedPage],
        source: &RawMemorySource,
        extra_context: Option<&str>,
    ) -> Result<IngestPlan, AlephError> {
```

and immediately after `let user = build_user_prompt(raws, related, &observation_date);`:

```rust
        // X1 C3: fold extension-contributed pre-compress context into the
        // planning prompt so extracted insights survive compression.
        let user = match extra_context {
            Some(extra) if !extra.trim().is_empty() => {
                format!("Extension context (preserve relevant facts):\n{extra}\n\n{user}")
            }
            _ => user,
        };
```

> The `plan()` unit tests (ingestor.rs:910+) call `plan(...)` directly with 4 args — add a trailing `None` to each so they still compile. Grep `\.plan(` to find them all.

- [ ] **Step 3: Update the in-file mock impl**

`src/memory/notes/ingest/ingestor.rs` has a mock impl at line ~870 (`async fn ingest_batch`). Add the `extra_context: Option<&str>` parameter (the body can ignore it: prefix `_extra_context`).

- [ ] **Step 4: Update the 3 mock impls in `service.rs`**

`src/memory/compression/service.rs` has three `ingest_batch` mock impls (lines ~682, ~755, ~791). Add `_extra_context: Option<&str>` to each signature.

- [ ] **Step 5: Grep guard — EVERY `ingest_batch` impl/def has the new param**

```bash
cd /Volumes/TBU4/Workspace/Aleph
# Every fn ingest_batch site (trait def + real impl + ingestor mock + 3 service mocks)
grep -rn "fn ingest_batch" src --include="*.rs"
# Every plan() call must pass the new 5th arg (real impl x2 + plan() unit tests)
grep -rn "\.plan(" src/memory/notes/ingest/ingestor.rs
# plan() definition carries extra_context
grep -n "extra_context: Option<&str>" src/memory/notes/ingest/ingestor.rs   # expect >=2 (ingest_batch + plan)
```

Manually open each `fn ingest_batch` site and confirm it carries `extra_context: Option<&str>` (or `_extra_context`); confirm both `self.plan(...)` calls in `ingest_batch` and every `.plan(` in tests pass the trailing arg. This is the critical compiler-substitute check for the two signature changes.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/ingest/ingestor.rs src/memory/compression/service.rs
git commit -m "memory: ingest_batch accepts extra_context for pre-compress contribution (C3)"
```

---

## Task 8: C3 — `CompressionService` fires `on_pre_compress`

**Files:**
- Modify: `src/memory/compression/service.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/memory.rs:251`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/memory/compression/service.rs` a test that a registered `on_pre_compress` extension's contribution reaches `ingest_batch` as `extra_context`. Use a recording mock ingestor:

```rust
    #[tokio::test]
    async fn pre_compress_contribution_reaches_ingest_extra_context() {
        use crate::memory::extensions::types::PreCompressCtx;
        use crate::memory::extensions::{MemoryExtension, MemoryExtensionRegistry};
        use crate::memory::namespace::NamespaceScope;
        use async_trait::async_trait;

        // Extension that contributes fixed text on pre-compress.
        struct ContribExt;
        #[async_trait]
        impl MemoryExtension for ContribExt {
            fn name(&self) -> &str {
                "test.contrib"
            }
            async fn on_pre_compress(
                &self,
                _ctx: &PreCompressCtx,
            ) -> Result<String, AlephError> {
                Ok("CONTRIB".to_string())
            }
        }

        // Recording ingestor that captures the extra_context it receives.
        let seen = Arc::new(crate::sync_primitives::Mutex::new(None::<String>));
        struct RecordIngestor {
            seen: Arc<crate::sync_primitives::Mutex<Option<String>>>,
        }
        #[async_trait]
        impl crate::memory::notes::ingest::CompoundIngestor for RecordIngestor {
            async fn ingest_batch(
                &self,
                _agent_id: &str,
                _raws: Vec<crate::memory::store::raw_memory::RawMemory>,
                extra_context: Option<&str>,
            ) -> Result<crate::memory::notes::ingest::ApplyReport, AlephError> {
                *self.seen.lock().unwrap_or_else(|e| e.into_inner()) =
                    extra_context.map(|s| s.to_string());
                Ok(crate::memory::notes::ingest::ApplyReport::default())
            }
        }

        let reg = Arc::new(MemoryExtensionRegistry::new());
        reg.register(Arc::new(ContribExt));
        // Build a service with one transcript raw queued + the recording ingestor +
        // the registry, run compress_to_notes, assert seen == Some("CONTRIB").
        // (Use the file's existing service-construction test helper; pass the
        // recording ingestor via with_compound_ingestor and reg via
        // with_extension_registry. Seed one raw memory for workspace "default".)
        // ... assemble per the existing test harness in this module ...
        let _ = (reg, seen, NamespaceScope::Owner); // wire per local helpers
    }
```

> Note for implementer: this module already has compound-ingest tests (`compress_to_notes_*`) that construct a `CompressionService` with a mock ingestor and seed raw memories. Mirror that exact setup; the only additions are `.with_extension_registry(reg)` and asserting the recorded `extra_context`.

- [ ] **Step 2: Add the registry field + builder**

In `src/memory/compression/service.rs`, add a field to the `CompressionService` struct:

```rust
    /// Optional memory-extension registry. When set, `compress_to_notes` fires
    /// `on_pre_compress` and folds the contribution into the ingest prompt.
    extension_registry: Option<Arc<crate::memory::extensions::MemoryExtensionRegistry>>,
```

Initialize it to `None` in `new_with_backend` (in the `Self { ... }` literal, ~line 108):

```rust
            extension_registry: None,
```

Add the builder after `with_profile_synthesizer` (~line 165):

```rust
    /// Attach a memory-extension registry so `compress_to_notes` fires
    /// `on_pre_compress` before ingest.
    pub fn with_extension_registry(
        mut self,
        registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.extension_registry = Some(registry);
        self
    }
```

- [ ] **Step 3: Fire `on_pre_compress` and pass the result to `ingest_batch`**

In `compress_to_notes`, before the `ing.ingest_batch(workspace_id, ingest_rows)` call (line ~276), compute the contribution and pass it through:

```rust
                // X1 C3: let extensions contribute context before ingest.
                let extra_context: Option<String> = if let Some(reg) = &self.extension_registry {
                    let ctx = crate::memory::extensions::types::PreCompressCtx {
                        agent_id: workspace_id.to_string(),
                        namespace: crate::memory::namespace::NamespaceScope::Owner,
                        session_id: None,
                        messages_count: ingest_rows.len(),
                        oldest_at: None,
                        newest_at: None,
                    };
                    let text = reg.dispatch_on_pre_compress(&ctx).await;
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                } else {
                    None
                };
                let ingest_outcome = ing
                    .ingest_batch(workspace_id, ingest_rows, extra_context.as_deref())
                    .await;
```

(Replace the existing `let ingest_outcome = ing.ingest_batch(workspace_id, ingest_rows).await;` line.)

- [ ] **Step 4: Boot wiring — thread the registry into `init_compression_service`**

`init_compression_service` (handlers/memory.rs:232) does NOT currently receive the registry. Add a trailing parameter to its signature (after `profile_synthesizer`, line ~246):

```rust
    profile_synthesizer: Option<
        std::sync::Arc<dyn alephcore::memory::notes::profile::synthesizer::ProfileSynthesizer>,
    >,
    extension_registry: Option<
        std::sync::Arc<alephcore::memory::extensions::MemoryExtensionRegistry>,
    >,
) -> std::sync::Arc<alephcore::memory::compression::CompressionService> {
```

and add the wiring after the `profile_synthesizer` block (after line ~261):

```rust
    if let Some(reg) = extension_registry {
        service = service.with_extension_registry(reg);
    }
```

Then update the single call site at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:896` (`super::init_compression_service(...)`) to pass `Some(memory_ext_registry.clone())` as the new trailing argument. (`memory_ext_registry` is in scope there — it is constructed at agent_init line ~371.)

- [ ] **Step 5: Grep guards**

```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n "with_extension_registry\|extension_registry:" src/memory/compression/service.rs   # field + builder + init
grep -c "extension_registry: None" src/memory/compression/service.rs                       # expect 1 (new_with_backend)
# ingest_batch call now passes 3 args incl extra_context
grep -n "ingest_batch(workspace_id, ingest_rows" src/memory/compression/service.rs
grep -n "dispatch_on_pre_compress" src/memory/compression/service.rs                        # expect 1
grep -n "with_extension_registry\|extension_registry:" src/bin/aleph-server/commands/start/builder/handlers/memory.rs  # param + wiring
# call site passes the new trailing arg
grep -n "init_compression_service" src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
grep -n "memory_ext_registry.clone()" src/bin/aleph-server/commands/start/builder/agent_init/mod.rs  # >=1
```

Manually confirm the `init_compression_service(...)` call at agent_init:896 passes `Some(memory_ext_registry.clone())` as its final argument.

- [ ] **Step 6: Commit**

```bash
git add src/memory/compression/service.rs src/bin/aleph-server/commands/start/builder/handlers/memory.rs
git commit -m "memory: fire on_pre_compress and fold contribution into ingest (C3)"
```

---

## Final Verification (grep-only, NO cargo)

```bash
cd /Volumes/TBU4/Workspace/Aleph
# C1: all hook calls swapped; binding path complete
grep -c "self.caller.load().call(" src/memory/extensions/mcp_adapter.rs        # 6
grep -rn "register_mcp\|mcp_bindings_snapshot\|bind_memory_callers\|new_unbound\|ManagerBackedMcpCaller" src --include="*.rs" | grep -v test
# C1 boot: 1 cold + 2 hot
grep -rn "bind_memory_callers().await" src --include="*.rs"                     # 3 total
# C2: delegation wired
grep -n "dispatch_on_delegation" src/agents/subagent_tool/spawn.rs             # 1
# C3: every ingest_batch carries extra_context
grep -rn "fn ingest_batch" src --include="*.rs"   # each must carry extra_context — verify by eye
grep -n "dispatch_on_pre_compress" src/memory/compression/service.rs           # 1
# on_session_switch: NO producer added (only the honest comment)
grep -rn "dispatch_on_session_switch" src --include="*.rs" | grep -v "registry.rs" | grep -v test   # expect ONLY mcp_adapter trait impl, zero new callers
```

Expected final state: `dispatch_on_session_switch` has **no** new production caller (only its definition in `registry.rs` and the trait impl in `mcp_adapter.rs`); `dispatch_on_delegation` and `dispatch_on_pre_compress` each have exactly one new production caller; the MCP binding path is complete (register → snapshot → bind → rebind).

## Self-Review notes (coverage vs spec)

- **C1** (binding): T1 (ArcSwap + ManagerBackedMcpCaller) + T2 (side-table/register_mcp/snapshot) + T3 (server-id resolution at registration) + T4 (bind_memory_callers) + T5 (cold+hot wiring). ✅
- **C2** (on_delegation): T6. ✅
- **C3** (on_pre_compress): T7 (ingest_batch param) + T8 (dispatch + builder + boot wiring). ✅
- **on_session_switch**: kept; T2 Step 4 replaces the TODO with an honest "no producer" comment; final guard asserts no new caller. ✅
- **Spec refinement**: `replace_caller` dropped (YAGNI) in favor of `mcp_bindings_snapshot` + direct `rebind`; server id stored on the extension and resolved at registration rather than at bind time. Documented at top of plan.
