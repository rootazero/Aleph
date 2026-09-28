# Plan P1 — temporal composability core (effects + lifecycle)

Base: `3ddc1f2e7` (+ spec commit `35e5f8bca`). Every `path:line` below is at `3ddc1f2e7`.
Spec: `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md` §3.1, §3.2, §3.5 G1–G3, §4, §5, §6.
Repo (worktree): `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/plugin-scope-round`.

## Facts verified in code before planning (things the brief assumed differently)

| Brief / contract said | What the code is | Consequence |
|---|---|---|
| `PluginId` = whatever `registry/types.rs` uses | Plugin ids are bare `String` everywhere (`PluginRecord.id: String`, `get_plugin(&str)`) | `type PluginId = String` alias in `effects/scope.rs`; every primitive takes `&str` |
| `PluginRegistry::unregister_plugin(id)` is new | It **exists**: `src/extension/registry/plugin_registry/mod.rs:432-455` (removes the record + every capability row keyed by that plugin) | Nothing to add on the registry; the `registry_row` disposer calls it |
| `runtime/wasm/loader.rs` | Does not exist. The loader is `src/extension/loader.rs` (`PluginLoader`, 731 lines) behind `Arc<tokio::sync::RwLock<PluginLoader>>`; WASM instantiation is `runtime/wasm/mod.rs::WasmRuntime::load_plugin_with` | The `wasm_module` producer lives in `src/extension/loader.rs` |
| Manager lock type for the registry | `plugin_registry: Arc<tokio::sync::RwLock<PluginRegistry>>` (async). Handles injected after construction use `crate::sync_primitives::RwLock<Option<_>>` (std) with `unwrap_or_else(\|e\| e.into_inner())`. `load_guard: tokio::sync::Mutex<()>` serialises `load_all`/`reload` | `scopes` uses `crate::sync_primitives::Mutex<HashMap<String, EffectScope>>` (std; never held across an await — the scope is *taken out* under the lock and disposed after). Transitions serialise on the existing `load_guard` |
| Async style | Everything on `ExtensionManager` is `async fn` on `&self`; WASM guest calls run on `spawn_blocking` with owned guards (`clone().write_owned().await`) | Producers are `async fn` taking `Arc` handles, so a `Disposer` can own what it needs |
| `MemoryExtensionRegistry` keyed by plugin id | Keyed by `MemoryExtension::name()` — for plugins that is `manifest.name` (display name), set at `loader.rs:492 McpMemoryExtension::new_unbound(manifest.name.clone(), …)` | `unregister(name)` keyed by the same name; the disposer captures the exact name it registered — no plugin-id side table |
| ToolCatalog reachable from `ExtensionManager` | Not reachable: `Arc<ToolCatalog>` is built inside `init_tool_catalog` (`tool_catalog_init.rs:45`) **after** the first `ensure_loaded()` (`agent_init/mod.rs:600`) and threaded by parameter; there is no global | `ExtensionManager::set_tool_catalog(Arc<ToolCatalog>)` (same injection pattern as `set_mcp_handle`/`set_memory_registry`), and the catalog is constructed *before* the first `ensure_loaded` (boot reorder, Task P1.8) — smaller than a new `CapabilitySlot` (which would also need `ALL_SLOTS`, a pin test and a doctor row) |
| Boot order | `load_all` runs at `agent_init/mod.rs:600` **before** `set_memory_registry` (`tool_catalog_init.rs:518`) and `set_mcp_handle` (`start/mod.rs:1445`, in a background task that then runs `sync_mcp_plugin_servers` + `sync_plugin_services`) | All three handles are installed before the first `load_all` so one `mount` can register all six effects (P1.8). The background catch-up task is deleted (P1.11) |
| MCP `add_transient_server` | Awaits the child's `initialize` handshake (`actor.rs:737-800`, up to 60 s per server); today it runs in a background task "because starting MCP server subprocesses must never block boot" | New `McpManagerHandle::add_transient_server_detached` sends the command and returns the `oneshot` receiver; ordering against a later `remove_transient_server` is guaranteed by the actor's single mpsc queue (P1.4). The producer hands the receivers back to `lifecycle.rs`, whose `watch_server_starts` awaits them on one task per plugin and whose `activation_settled()` completes when every such task has finished (R1.1; P3 turns the watcher into the `Pending` writer) |
| `plugins.load` / `plugins.unload` RPC | Their only backing bodies are `load_runtime_plugin` / `unload_runtime_plugin`, which the primitives replace; zero clients (`rg` in Panel/TUI/CLI/qa: only `method_census.rs` and their own tests) | Deleted in P1.11 (see Open questions: P5's CUT list shrinks by these two) |
| `get_all_commands` (`skill_ops.rs:23`) | Exactly one caller, `tool_catalog_init.rs:216` — the boot-only slash registration this phase replaces | Deleted in P1.11 (P4's `/cmd` body work needs a per-command lookup, not this list — Open question) |

## Design decisions fixed for every task below

1. **`Disposer` resolves to `Result<(), String>`** (contract says `()`). Reason: `unload_plugin`, `remove_transient_server` and `stop_plugin_services` all report failure, and the spec's §4 "dispose 单条失败 → 记日志（带 step 标签）继续" needs the failure *and* the label in one place — `EffectScope::dispose` logs `step = label, error = e` once, instead of every closure knowing its own label. Recorded in Contract deltas.
2. **Mount admits with today's gates**, in this order: row present (`NotFound`) → not already mounted (`AlreadyMounted`) → owner trust (`Blocked`, writes the Blocked row) → operator preference `plugins.toml` (`Disabled`) → manifest parse (`Parse`, writes the Error row) → six effects. A step failure disposes the partial scope and writes `PluginStatus::Error("<step>: <reason>")` at the single site `lifecycle.rs::write_failed_row` (G-2: `PluginStatus` keeps today's names — `Loaded / Disabled / Blocked(String) / Error(String)`; P3 adds `Pending { waiting_on }` and removes `Overridden`; no rename).
3. **A missing handle is a recorded skip, not a mount failure.** CLI paths (`aleph-server plugins list`, `commands/plugins.rs:22`) construct a manager with no MCP handle / memory registry / catalog; today "plugin MCP registration simply no-ops there" (`mod.rs:213-219` field doc). `EffectScope::skip(step, why)` records it, `mount` adds a `Warn` diagnostic, status stays `Loaded`. P3 turns `scope.skipped()` into `Pending { waiting_on }`. The daemon never skips because P1.8 installs every handle first; `qa/plugins/run.sh scope` greps the server log for `not attached` and fails on it.
4. **Registry row is an effect; the terminal status is a separate write.** `unmount` clones the record, disposes (the `registry_row` disposer removes the row and every capability row), then re-inserts the clone with `status = Disabled, error = None` so the plugin stays listable and re-enablable — exactly what `re_enabling_needs_no_reload` (`mod.rs:1603`) asserts today. `reload()` disposes without re-inserting because discovery rebuilds the rows.
5. **Only mounted plugins have capability rows.** Today `load_all` registers capabilities for disabled plugins too (`mod.rs:664-680`), relying on `is_active()` filters downstream. After P1 a `Disabled` row has counts (from the adapter output) but no tool/hook/service/skill/agent rows; every downstream `is_active()` filter stays (harmless, and P2's `visible_to` needs the rows anyway).
6. **WASM loads at mount, not at first call.** `ensure_plugin_loaded` (lazy load on first `call_plugin_tool`) is deleted; a call against an unmounted runtime is an error. Reason: the `service` step needs the module loaded before `start_handler` runs, and a lazily created effect would sit outside the registration order the scope disposes in.
7. **`PluginLoader` stops mirroring `.mcp.json`.** The `mcp_server` producer reads `.mcp.json` itself (`mcp_config::read_mcp_json`) and hands configs to the manager; `PluginLoader.mcp_configs` and its six accessors lose every caller and are deleted (P1.11).
8. **`after_transition()` is the only caller** of `republish_plugin_projections()` and `sync_hooks_from_registry()`; it also rebuilds the hook executor and re-layers `sync_user_hooks()`. It runs **once** per public primitive (`mount`, `unmount`, `reload_plugin`, `reload`, `load_all`), not once per plugin inside `load_all`. P6 appends its one `notify_tools_list_changed()` line inside this function and nowhere else.
9. `src/harness/`: 0-line diff. No task touches it.

---

### Task P1.1: `Disposer`, `EffectScope`, `DisposeReport`

**Files:**
- Create: `src/extension/effects/mod.rs`
- Create: `src/extension/effects/disposer.rs`
- Create: `src/extension/effects/scope.rs`
- Modify: `src/extension/mod.rs:26-52` (module list) and `:54-77` (re-exports)
- Test: `src/extension/effects/scope.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `futures::future::BoxFuture` (workspace dep `futures = "0.3"`, `Cargo.toml:84`), `tracing`.
- Produces:
  ```rust
  pub type DisposeOutcome = Result<(), String>;
  pub type Disposer = Box<dyn FnOnce() -> BoxFuture<'static, DisposeOutcome> + Send>;
  pub fn sync_disposer(f: impl FnOnce() -> DisposeOutcome + Send + 'static) -> Disposer;
  pub fn async_disposer<F, Fut>(f: F) -> Disposer
      where F: FnOnce() -> Fut + Send + 'static, Fut: Future<Output = DisposeOutcome> + Send + 'static;
  pub type PluginId = String;
  pub const STEP_LABELS: [&str; 6];
  pub struct EffectScope;  // new(plugin_id) / plugin_id() / effect(step, d) / skip(step, why) / skipped() / steps() / len() / is_empty() / dispose(self)
  pub struct DisposeReport { pub plugin_id: PluginId, pub steps: Vec<(&'static str, DisposeOutcome)> }  // all_ok() / failures() / empty(id)
  ```

- [ ] **Step 1: Write the failing test**

Create `src/extension/effects/scope.rs` with only the test module first (the types do not exist yet, so this fails to compile — that is the RED):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::effects::{async_disposer, sync_disposer};
    use std::sync::{Arc, Mutex};

    fn recorder() -> (Arc<Mutex<Vec<&'static str>>>, impl Fn(&'static str) -> Disposer) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = Arc::clone(&log);
        let mk = move |label: &'static str| {
            let l = Arc::clone(&l2);
            sync_disposer(move || {
                l.lock().unwrap().push(label);
                Ok(())
            })
        };
        (log, mk)
    }

    #[tokio::test]
    async fn disposers_run_in_reverse_registration_order() {
        let (log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        scope.effect("registry_row", mk("registry_row"));
        scope.effect("wasm_module", mk("wasm_module"));
        scope.effect("mcp_server", mk("mcp_server"));
        assert_eq!(scope.len(), 3);
        let report = scope.dispose().await;
        assert!(report.all_ok(), "{report:?}");
        assert_eq!(
            *log.lock().unwrap(),
            vec!["mcp_server", "wasm_module", "registry_row"],
            "reverse of registration order"
        );
        assert_eq!(
            report.steps.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec!["mcp_server", "wasm_module", "registry_row"]
        );
    }

    #[tokio::test]
    async fn async_disposer_is_awaited_before_the_next_one_runs() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut scope = EffectScope::new("p");
        let l = Arc::clone(&log);
        scope.effect(
            "registry_row",
            sync_disposer(move || {
                l.lock().unwrap().push("row");
                Ok(())
            }),
        );
        let l = Arc::clone(&log);
        scope.effect(
            "service",
            async_disposer(move || async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                l.lock().unwrap().push("service-done");
                Ok(())
            }),
        );
        let report = scope.dispose().await;
        assert!(report.all_ok());
        // If the async one were not awaited, "row" would land first.
        assert_eq!(*log.lock().unwrap(), vec!["service-done", "row"]);
    }

    #[tokio::test]
    async fn a_failing_disposer_is_recorded_and_the_rest_still_run() {
        let (log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        scope.effect("registry_row", mk("registry_row"));
        scope.effect(
            "mcp_server",
            sync_disposer(|| Err("remove_transient_server: channel closed".to_string())),
        );
        scope.effect("slash_command", mk("slash_command"));
        let report = scope.dispose().await;
        assert!(!report.all_ok());
        let failures: Vec<_> = report.failures().collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "mcp_server");
        assert!(failures[0].1.contains("channel closed"));
        assert_eq!(*log.lock().unwrap(), vec!["slash_command", "registry_row"]);
    }

    #[tokio::test]
    async fn a_panicking_disposer_is_recorded_and_the_rest_still_run() {
        let (log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        scope.effect("registry_row", mk("registry_row"));
        scope.effect(
            "wasm_module",
            async_disposer(|| async { panic!("guest unload exploded") }),
        );
        scope.effect(
            "service",
            sync_disposer(|| -> DisposeOutcome { panic!("sync boom") }),
        );
        let report = scope.dispose().await;
        let failures: Vec<_> = report.failures().collect();
        assert_eq!(failures.len(), 2, "{report:?}");
        assert!(failures.iter().any(|(s, e)| *s == "wasm_module" && e.contains("guest unload exploded")));
        assert!(failures.iter().any(|(s, e)| *s == "service" && e.contains("sync boom")));
        assert_eq!(*log.lock().unwrap(), vec!["registry_row"]);
    }

    #[tokio::test]
    async fn len_counts_effects_not_skips() {
        let (_log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        assert!(scope.is_empty());
        scope.effect("registry_row", mk("registry_row"));
        scope.skip("mcp_server", "MCP manager not attached");
        assert_eq!(scope.len(), 1);
        assert_eq!(scope.skipped().len(), 1);
        assert_eq!(scope.skipped()[0].0, "mcp_server");
        assert_eq!(scope.steps(), vec!["registry_row"]);
        assert_eq!(scope.plugin_id(), "p");
    }

    #[test]
    fn every_step_label_is_distinct_and_in_mount_order() {
        let mut seen = std::collections::HashSet::new();
        for l in STEP_LABELS {
            assert!(seen.insert(l), "duplicate label {l}");
        }
        assert_eq!(STEP_LABELS[0], "registry_row", "the row is first in, last out");
        assert_eq!(STEP_LABELS[5], "slash_command");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::effects -- --nocapture`
Expected: FAIL to compile — `error[E0432]: unresolved import` / `cannot find type EffectScope` (the module does not exist yet).

- [ ] **Step 3: Write minimal implementation**

`src/extension/effects/mod.rs`:

```rust
//! Effects — the reversible half of a plugin's footprint on the running process.
//!
//! One rule (spec §3.1, U8 "C: 效果归 scope、视图归派生"): every registration
//! that has an inverse returns a [`Disposer`]; the caller (`lifecycle.rs`)
//! owns it inside the plugin's [`EffectScope`]; unmount = run the list in
//! reverse. Anything that has no inverse but can be re-derived from the
//! registry (skill dirs, sub-agents, tool index, hook executor) is a *view*
//! and is recomputed by `ExtensionManager::after_transition`, not disposed.
//!
//! Absorbed from Cordis `fiber.ts:418-561` / dsh AGENTS.md "Registrations are
//! effects" as an ownership rule only: no DI container, no Proxy context, no
//! cascade restart (scan-dsh-cordis.md Top-8 #1; three prior rounds' "不引
//! fiber" rulings stand, narrowed to this).

mod disposer;
mod scope;

pub use disposer::{async_disposer, sync_disposer, DisposeOutcome, Disposer};
pub use scope::{DisposeReport, EffectScope, PluginId, STEP_LABELS};
```

`src/extension/effects/disposer.rs`:

```rust
//! [`Disposer`] — one reversible side effect a plugin made on the running process.

use futures::future::BoxFuture;
use std::future::Future;

/// What a disposer reports. `Err` is a message for the log line
/// `EffectScope::dispose` writes with the step label attached; it never
/// stops the remaining disposers.
pub type DisposeOutcome = Result<(), String>;

/// One reversible side effect. Async because MCP server removal and service
/// stop are; a sync cleanup wraps itself with [`sync_disposer`].
pub type Disposer = Box<dyn FnOnce() -> BoxFuture<'static, DisposeOutcome> + Send>;

/// Wrap a synchronous cleanup as a [`Disposer`].
#[must_use]
pub fn sync_disposer(f: impl FnOnce() -> DisposeOutcome + Send + 'static) -> Disposer {
    Box::new(move || Box::pin(async move { f() }))
}

/// Wrap an async cleanup as a [`Disposer`].
#[must_use]
pub fn async_disposer<F, Fut>(f: F) -> Disposer
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = DisposeOutcome> + Send + 'static,
{
    Box::new(move || Box::pin(f()))
}
```

`src/extension/effects/scope.rs` (above the test module written in Step 1):

```rust
//! [`EffectScope`] — everything one mounted plugin put into the runtime, in
//! registration order, and the one operation that takes it all back out.

use super::disposer::{DisposeOutcome, Disposer};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

/// Plugin identifier as the registry spells it: a bare string
/// (`PluginRecord.id`, `PluginRegistry::get_plugin(&str)`).
pub type PluginId = String;

/// The six effect kinds `lifecycle.rs::mount` registers, in registration
/// order. Dispose runs the reverse: the registry row is first in and last
/// out, so when the views are re-derived it is already gone.
pub const STEP_LABELS: [&str; 6] = [
    "registry_row",
    "wasm_module",
    "mcp_server",
    "service",
    "memory_extension",
    "slash_command",
];

/// Everything one mounted plugin put into the runtime.
pub struct EffectScope {
    plugin_id: PluginId,
    disposers: Vec<(&'static str, Disposer)>,
    /// Steps `mount` could not perform because the runtime handle they need
    /// is not installed in this process (CLI/test paths). Recorded so the
    /// status can say "mounted, waiting on X" instead of silently `Loaded`.
    skipped: Vec<(&'static str, String)>,
}

impl EffectScope {
    #[must_use]
    pub fn new(plugin_id: impl Into<PluginId>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            disposers: Vec::new(),
            skipped: Vec::new(),
        }
    }

    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Record one effect. `step` must be one of [`STEP_LABELS`]; a label
    /// outside that list is a programming error (debug-asserted) because the
    /// activation gate (P3) derives `waiting_on` from these names.
    pub fn effect(&mut self, step: &'static str, d: Disposer) {
        debug_assert!(
            STEP_LABELS.contains(&step),
            "unknown effect step label {step:?}; add it to STEP_LABELS first"
        );
        self.disposers.push((step, d));
    }

    /// Record that `step` was not performed because its handle is absent.
    pub fn skip(&mut self, step: &'static str, why: impl Into<String>) {
        debug_assert!(STEP_LABELS.contains(&step));
        self.skipped.push((step, why.into()));
    }

    #[must_use]
    pub fn skipped(&self) -> &[(&'static str, String)] {
        &self.skipped
    }

    /// Labels of the registered effects, in registration order.
    #[must_use]
    pub fn steps(&self) -> Vec<&'static str> {
        self.disposers.iter().map(|(s, _)| *s).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.disposers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.disposers.is_empty()
    }

    /// Run every disposer in REVERSE registration order. A failing or
    /// panicking disposer is recorded in the report (and logged with its
    /// step label) and does NOT stop the rest (P7). Consumes `self`: a scope
    /// cannot be half-disposed.
    pub async fn dispose(self) -> DisposeReport {
        let mut steps = Vec::with_capacity(self.disposers.len());
        for (step, d) in self.disposers.into_iter().rev() {
            let outcome = run_one(d).await;
            if let Err(e) = &outcome {
                tracing::warn!(plugin_id = %self.plugin_id, step, error = %e, "disposer failed; continuing");
            }
            steps.push((step, outcome));
        }
        DisposeReport {
            plugin_id: self.plugin_id,
            steps,
        }
    }
}

/// Run one disposer, converting a panic on either side of the `await`
/// (building the future, or polling it) into an `Err`.
async fn run_one(d: Disposer) -> DisposeOutcome {
    let fut = match std::panic::catch_unwind(AssertUnwindSafe(d)) {
        Ok(fut) => fut,
        Err(payload) => return Err(format!("disposer panicked: {}", panic_message(&payload))),
    };
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(outcome) => outcome,
        Err(payload) => Err(format!("disposer panicked: {}", panic_message(&payload))),
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// What `dispose` did, step by step, in the order it ran them.
#[derive(Debug)]
pub struct DisposeReport {
    pub plugin_id: PluginId,
    pub steps: Vec<(&'static str, DisposeOutcome)>,
}

impl DisposeReport {
    #[must_use]
    pub fn all_ok(&self) -> bool {
        self.steps.iter().all(|(_, r)| r.is_ok())
    }

    /// The failed steps, in run order.
    pub fn failures(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        self.steps
            .iter()
            .filter_map(|(s, r)| r.as_ref().err().map(|e| (*s, e.as_str())))
    }

    /// A report for a plugin that had nothing to dispose.
    #[must_use]
    pub fn empty(plugin_id: impl Into<PluginId>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            steps: Vec::new(),
        }
    }
}
```

`src/extension/mod.rs` — add the module and re-exports. At `:26-52` the module list currently reads (excerpt):

```rust
pub mod hooks;
mod loader;
pub mod marketplace;
pub mod runtime;
pub mod scope;
pub mod validation;

pub mod capability;
pub mod registrar;
```

Insert `pub mod effects;` after `pub mod capability;`. After `pub use error::*;` (`:54`) add:

```rust
pub use effects::{
    async_disposer, sync_disposer, DisposeOutcome, DisposeReport, Disposer, EffectScope, PluginId,
};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::effects -- --nocapture`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/extension/effects/mod.rs src/extension/effects/disposer.rs src/extension/effects/scope.rs src/extension/mod.rs
git commit -m "extension/effects: Disposer + EffectScope with reverse-order, fault-isolated dispose

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.2: `registry_row` producer — `registrar::register_plugin_row` → `Disposer`

**Files:**
- Modify: `src/extension/registrar/api.rs:1-13` (imports) — append the producer after `impl<'a> CapabilityApi<'a>` (`:28-148`)
- Modify: `src/extension/registrar/mod.rs:1-8` (re-export)
- Test: `src/extension/registrar/api.rs` (`mod tests`, existing at `:154`)

`PluginRegistry::unregister_plugin` already exists (`src/extension/registry/plugin_registry/mod.rs:432-455`) and removes the record plus every tool/hook/service/skill/agent/diagnostic row for that plugin id — it is the inverse this producer returns.

**Interfaces:**
- Consumes: `CapabilityApi::new` / `register_capability` (`api.rs:30-63`), `PluginRegistry::{register_plugin, unregister_plugin, add_diagnostic}`, `Disposer`/`async_disposer` (P1.1).
- Produces:
  ```rust
  pub(crate) async fn register_plugin_row(
      registry: Arc<tokio::sync::RwLock<PluginRegistry>>,
      record: PluginRecord,
      permissions: Vec<PluginPermission>,
      capabilities: Vec<CapabilityDeclaration>,
  ) -> Disposer
  ```

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `src/extension/registrar/api.rs`:

```rust
    #[tokio::test]
    async fn register_plugin_row_writes_the_row_and_its_disposer_removes_everything() {
        use crate::extension::registry::PluginRegistry;
        let registry = std::sync::Arc::new(tokio::sync::RwLock::new(PluginRegistry::new()));
        let record = PluginRecord::new(
            "test-plugin".to_string(),
            "Test Plugin".to_string(),
            crate::extension::types::PluginKind::Static,
            crate::extension::types::PluginOrigin::Global,
        );

        let disposer = register_plugin_row(
            std::sync::Arc::clone(&registry),
            record,
            vec![PluginPermission::Background],
            vec![make_tool(), make_service(), make_skill(), make_agent()],
        )
        .await;

        {
            let reg = registry.read().await;
            assert!(reg.get_plugin("test-plugin").is_some());
            assert_eq!(reg.list_tools_for_plugin("test-plugin").len(), 1);
            assert_eq!(reg.list_services().len(), 1);
            assert_eq!(reg.list_skills().len(), 1);
            assert_eq!(reg.list_agents().len(), 1);
        }

        disposer().await.expect("row disposer cannot fail");

        let reg = registry.read().await;
        assert!(reg.get_plugin("test-plugin").is_none(), "record removed");
        assert!(reg.list_tools_for_plugin("test-plugin").is_empty());
        assert!(reg.list_services().is_empty());
        assert!(reg.list_skills().is_empty());
        assert!(reg.list_agents().is_empty());
    }

    #[tokio::test]
    async fn a_capability_refused_for_permissions_becomes_a_diagnostic_not_a_failure() {
        use crate::extension::registry::{DiagnosticLevel, PluginRegistry};
        let registry = std::sync::Arc::new(tokio::sync::RwLock::new(PluginRegistry::new()));
        let record = PluginRecord::new(
            "test-plugin".to_string(),
            "Test Plugin".to_string(),
            crate::extension::types::PluginKind::Static,
            crate::extension::types::PluginOrigin::Global,
        );
        // No `Background` permission → the service is refused; the tool still lands.
        let disposer = register_plugin_row(
            std::sync::Arc::clone(&registry),
            record,
            vec![],
            vec![make_tool(), make_service()],
        )
        .await;
        {
            let reg = registry.read().await;
            assert_eq!(reg.list_tools_for_plugin("test-plugin").len(), 1);
            assert!(reg.list_services().is_empty());
            let diags: Vec<_> = reg
                .diagnostics()
                .iter()
                .filter(|d| d.plugin_id.as_deref() == Some("test-plugin"))
                .collect();
            assert_eq!(diags.len(), 1, "{diags:?}");
            assert!(matches!(diags[0].level, DiagnosticLevel::Warn));
            assert!(diags[0].message.contains("Background"), "{}", diags[0].message);
        }
        disposer().await.unwrap();
        assert!(registry.read().await.diagnostics().is_empty(), "diagnostics go with the row");
    }
```

`make_agent()` does not exist in the test module today (`make_tool`, `make_service`, `make_skill` do, `:171-222`); add next to them:

```rust
    fn make_agent() -> CapabilityDeclaration {
        CapabilityDeclaration::Agent(AgentRegistration {
            name: "helper".to_string(),
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        })
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::registrar::api -- --nocapture`
Expected: FAIL to compile — `cannot find function register_plugin_row in this scope`.

- [ ] **Step 3: Write minimal implementation**

In `src/extension/registrar/api.rs`, extend the imports at `:7-12`:

```rust
use anyhow::{anyhow, Result};

use crate::extension::capability::{CapabilityDeclaration, Tier};
use crate::extension::effects::{async_disposer, Disposer};
use crate::extension::manifest::PluginPermission;
use crate::extension::registry::{DiagnosticLevel, PluginDiagnostic, PluginRegistry};
use crate::extension::types::PluginRecord;
use crate::sync_primitives::Arc;
```

Append after the `impl<'a> CapabilityApi<'a> { … }` block (after `:148`):

```rust
/// Mount-time registry effect: write the plugin record and every capability
/// it declares under one write lock, and hand back the disposer that removes
/// all of it (`PluginRegistry::unregister_plugin`).
///
/// A capability the plugin lacks permission for is NOT a mount failure — it
/// becomes a `Warn` diagnostic on the row (it used to be a `debug!` line in
/// `load_all`, i.e. invisible on every face). The row itself cannot fail to
/// register, so this returns `Disposer` rather than `Result<Disposer, _>`.
///
/// This is a free function taking the `Arc` handle, not a method on
/// [`CapabilityApi`]: the api borrows `&mut PluginRegistry` inside the lock
/// guard and cannot own what its disposer would need.
pub(crate) async fn register_plugin_row(
    registry: Arc<tokio::sync::RwLock<PluginRegistry>>,
    record: PluginRecord,
    permissions: Vec<PluginPermission>,
    capabilities: Vec<CapabilityDeclaration>,
) -> Disposer {
    let plugin_id = record.id.clone();
    {
        let mut reg = registry.write().await;
        reg.register_plugin(record);
        let mut refused: Vec<String> = Vec::new();
        {
            let mut api = CapabilityApi::new(&mut reg, plugin_id.clone(), permissions);
            for cap in capabilities {
                let kind = cap.kind_name();
                if let Err(e) = api.register_capability(cap) {
                    refused.push(format!("{kind} capability not registered: {e}"));
                }
            }
        }
        for message in refused {
            tracing::warn!(plugin_id = %plugin_id, %message, "capability refused at mount");
            reg.add_diagnostic(PluginDiagnostic {
                level: DiagnosticLevel::Warn,
                message,
                plugin_id: Some(plugin_id.clone()),
                source: Some("mount".to_string()),
            });
        }
    }
    async_disposer(move || async move {
        registry.write().await.unregister_plugin(&plugin_id);
        Ok(())
    })
}
```

`src/extension/registrar/mod.rs` currently:

```rust
//! Registrar — Unified Registration Surface
//!
//! Provides `CapabilityApi` for writing capabilities into `PluginRegistry`.

pub mod api;
pub mod mcp_registrar;

pub use api::CapabilityApi;
```

Add `pub(crate) use api::register_plugin_row;` after the `CapabilityApi` re-export.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::registrar::api -- --nocapture`
Expected: PASS (existing tests + 2 new).

- [ ] **Step 5: Commit**

```bash
git add src/extension/registrar/api.rs src/extension/registrar/mod.rs
git commit -m "extension/registrar: register_plugin_row returns the Disposer that unregisters the row

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.3: `wasm_module` producer — `loader::load_wasm_effect` → `Disposer`

**Files:**
- Modify: `src/extension/loader.rs:22-31` (imports), `:140-155` (`load_plugin` visibility), append the producer after `impl Default for PluginLoader` (`:472-476`)
- Test: `src/extension/loader.rs` (`mod tests`, existing at `:516`)

The loader is behind `Arc<tokio::sync::RwLock<PluginLoader>>` on the manager (`mod.rs:157`). `PluginLoader::load_plugin` (`loader.rs:141-155`) is synchronous and dispatches on `manifest.kind`; the WASM arm compiles the module with extism (`runtime/wasm/mod.rs:142-255`). extism's `PluginBuilder::build` links the module lazily (`Plugin::new` → `linker.instantiate_pre`, `extism-1.21.0/src/plugin.rs:422-423`), so the 8-byte empty module `\0asm\x01\0\0\0` is a valid fixture — instantiation and export lookup happen at first call, not at load.

**Interfaces:**
- Consumes: `PluginLoader::{load_plugin, unload_plugin, is_loaded}`, `Disposer`/`async_disposer`.
- Produces:
  ```rust
  pub(crate) async fn load_wasm_effect(
      loader: Arc<tokio::sync::RwLock<PluginLoader>>,
      manifest: &PluginManifest,
  ) -> ExtensionResult<Disposer>
  /// Test-only fixture bytes shared by every test that needs a loadable module.
  #[cfg(test)] pub(crate) const EMPTY_WASM_MODULE: &[u8] = b"\0asm\x01\x00\x00\x00";
  ```

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `src/extension/loader.rs`:

```rust
    #[tokio::test]
    async fn load_wasm_effect_loads_the_module_and_its_disposer_unloads_it() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("plugin.wasm"), EMPTY_WASM_MODULE).unwrap();
        let mut manifest = PluginManifest::new(
            "qa-wasm".to_string(),
            "QA WASM".to_string(),
            PluginKind::Wasm,
            PathBuf::from("plugin.wasm"),
        );
        manifest.root_dir = tmp.path().to_path_buf();

        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));
        let disposer = load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .expect("an empty module links");
        assert!(loader.read().await.is_loaded("qa-wasm"));
        assert!(loader.read().await.is_wasm_runtime_active());

        disposer().await.expect("unload of a loaded module succeeds");
        assert!(!loader.read().await.is_loaded("qa-wasm"));
    }

    #[tokio::test]
    async fn load_wasm_effect_with_a_missing_file_registers_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut manifest = PluginManifest::new(
            "qa-missing".to_string(),
            "QA Missing".to_string(),
            PluginKind::Wasm,
            PathBuf::from("nope.wasm"),
        );
        manifest.root_dir = tmp.path().to_path_buf();
        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));
        let err = load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .err()
            .expect("missing file is an error, not a silent skip");
        assert!(err.to_string().contains("WASM file not found"), "{err}");
        assert!(!loader.read().await.is_loaded("qa-missing"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::loader::tests::load_wasm_effect -- --nocapture`
Expected: FAIL to compile — `cannot find function load_wasm_effect`, `cannot find value EMPTY_WASM_MODULE`.

- [ ] **Step 3: Write minimal implementation**

In `src/extension/loader.rs`, extend the imports (`:22-31`) with:

```rust
use crate::extension::effects::{async_disposer, Disposer};
```

Change `load_plugin`'s visibility (`:141`) from

```rust
    pub fn load_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
```

to

```rust
    pub(super) fn load_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
```

(Its only callers are inside `src/extension/` — `plugin_ops.rs:92,200` today, and after P1.11 only the producer below. `pub(super)` is what keeps the G1 census (P1.12) honest: crate-visible registration functions in this file must return a `Disposer`.)

Append after `impl Default for PluginLoader { … }` (`:472-476`):

```rust
/// Mount-time runtime effect for a `PluginKind::Wasm` plugin: instantiate the
/// module now and hand back the disposer that unloads it.
///
/// Loading at mount (not at first tool call, as `ensure_plugin_loaded` used
/// to) is what lets the `service` step that follows run the plugin's
/// `start_handler`, and what puts the module inside the scope's dispose order
/// instead of beside it.
pub(crate) async fn load_wasm_effect(
    loader: Arc<tokio::sync::RwLock<PluginLoader>>,
    manifest: &PluginManifest,
) -> ExtensionResult<Disposer> {
    debug_assert_eq!(manifest.kind, PluginKind::Wasm, "only WASM plugins have a module to load");
    loader.write().await.load_plugin(manifest)?;
    let plugin_id = manifest.id.clone();
    Ok(async_disposer(move || async move {
        loader
            .write()
            .await
            .unload_plugin(&plugin_id)
            .map_err(|e| e.to_string())
    }))
}

/// The smallest valid WebAssembly module (magic + version, no sections).
/// extism links it without complaint and fails only when an export is
/// called, which is exactly the shape a lifecycle fixture needs.
#[cfg(test)]
pub(crate) const EMPTY_WASM_MODULE: &[u8] = b"\0asm\x01\x00\x00\x00";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::loader -- --nocapture`
Expected: PASS (existing loader tests + 2 new). If `load_wasm_effect_loads_the_module_and_its_disposer_unloads_it` fails with `Failed to load WASM: …`, the extism version rejects the empty module — stop and report the exact message to the lead (the fallback fixture is a `(module)` compiled with the `wat` crate as a dev-dependency, which is a dependency decision the lead must make; do not add it silently).

- [ ] **Step 5: Commit**

```bash
git add src/extension/loader.rs
git commit -m "extension/loader: load_wasm_effect returns the Disposer that unloads the module

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.4: `mcp_server` producer — `mcp_registrar::register_transient_servers` → `Disposer` (non-blocking add)

**Files:**
- Modify: `src/mcp/manager/handle.rs:100-117` (add `add_transient_server_detached` next to `add_transient_server`)
- Modify: `src/extension/registrar/mcp_registrar.rs:1-12` (header + imports), append the producer before `mod tests` (`:380`)
- Test: `src/extension/registrar/mcp_registrar.rs` (`mod tests`), `src/mcp/manager/handle.rs` (new test)

Why a detached add: `McpManagerHandle::add_transient_server` (`handle.rs:106-117`) awaits the actor's reply, and the actor's `add_transient_server` (`actor.rs:603-617`) awaits `start_server_internal` → the child's `initialize` handshake (`external/connection.rs:335-379`, up to 60 s per server). Boot today runs this in a background task because "starting MCP server subprocesses must never block boot" (`start/mod.rs:1436-1440`). `mount` runs on the boot path, so the producer must return after *enqueuing*. Ordering is still deterministic: the actor drains one mpsc queue (`handle.rs:47`), so an `AddTransientServer` sent before a later `RemoveTransientServer` is always processed first — the disposer can never overtake the add.

**Interfaces:**
- Consumes: `McpManagerHandle` (`tx: mpsc::Sender<McpCommand>`), `McpCommand::AddTransientServer { config, respond_to }` (`types.rs:472-477`), `McpManagerHandle::remove_transient_server` (`handle.rs:124`).
- Produces:
  ```rust
  // src/mcp/manager/handle.rs
  pub async fn add_transient_server_detached(&self, config: McpManagerConfig)
      -> Result<oneshot::Receiver<std::result::Result<(), String>>>;
  // src/extension/registrar/mcp_registrar.rs
  pub type ServerStartReceiver = oneshot::Receiver<std::result::Result<(), String>>;
  pub(crate) async fn register_transient_servers(
      handle: McpManagerHandle,
      configs: HashMap<String, McpManagerConfig>,   // server_id -> config, as read_mcp_json returns
  ) -> std::result::Result<(Disposer, Vec<(String /* server_id */, ServerStartReceiver)>), String>;
  ```
  The receivers are handed back, NOT awaited or watched here (R1.1): `lifecycle.rs::watch_server_starts` (P1.9) owns the one task per plugin that awaits them, and P3 turns that task into the `Pending` writer.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `src/extension/registrar/mcp_registrar.rs` (`:380`):

```rust
    /// Drives a real actor. The server command does not exist, so the add
    /// FAILS inside the actor — that is the point: the producer must return
    /// before the actor answers, the failure must be observable on the
    /// receiver, and the disposer must still send the remove (a no-op for a
    /// server that never started, `actor.rs:624-641`).
    #[tokio::test]
    async fn register_transient_servers_enqueues_without_waiting_and_disposer_removes() {
        use crate::mcp::manager::{McpManagerActor, McpManagerConfig, McpManagerEvent};
        let dir = tempfile::tempdir().unwrap();
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json")))
            .await
            .unwrap();
        tokio::spawn(actor.run());
        let mut events = handle.subscribe();

        let mut configs = std::collections::HashMap::new();
        configs.insert(
            "plugin:qa/never".to_string(),
            McpManagerConfig::stdio("plugin:qa/never", "never (qa)", "qa-nonexistent-mcp-binary-9f3a"),
        );

        let started = std::time::Instant::now();
        let (disposer, receivers) = register_transient_servers(handle.clone(), configs)
            .await
            .expect("enqueue succeeds while the actor is alive");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "producer must not wait for the handshake ({:?})",
            started.elapsed()
        );
        assert_eq!(receivers.len(), 1);
        assert_eq!(receivers[0].0, "plugin:qa/never", "one receiver per server, keyed by id");

        // The disposer is a remove for the same id; it must be accepted even
        // though the add is still in flight / has failed.
        disposer().await.expect("remove_transient_server is a no-op for an unknown id");

        // The actor's verdict arrives on the receiver the caller was handed.
        let (_, rx) = receivers.into_iter().next().unwrap();
        let verdict = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .expect("actor answers")
            .expect("sender not dropped");
        assert!(verdict.is_err(), "a nonexistent binary fails to start: {verdict:?}");

        // No ServerStarted for a binary that does not exist; and nothing panicked.
        let got_started = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match events.recv().await {
                    Ok(McpManagerEvent::ServerStarted { server_id, .. }) if server_id == "plugin:qa/never" => break true,
                    Ok(_) => continue,
                    Err(_) => break false,
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(!got_started, "a nonexistent binary cannot have started");
    }

    #[tokio::test]
    async fn register_transient_servers_fails_when_the_actor_is_gone() {
        use crate::mcp::manager::{McpManagerActor, McpManagerConfig};
        let dir = tempfile::tempdir().unwrap();
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json")))
            .await
            .unwrap();
        drop(actor); // never run: the command channel's receiver is dropped
        let mut configs = std::collections::HashMap::new();
        configs.insert(
            "plugin:qa/x".to_string(),
            McpManagerConfig::stdio("plugin:qa/x", "x", "true"),
        );
        let err = register_transient_servers(handle, configs)
            .await
            .err()
            .expect("a closed channel is a step failure, not a silent skip");
        assert!(err.contains("plugin:qa/x"), "names the server: {err}");
    }
```

Append to `mod tests` in `src/mcp/manager/handle.rs` (create the module if absent — check with `grep -n 'mod tests' src/mcp/manager/handle.rs` first):

```rust
    #[tokio::test]
    async fn add_transient_server_detached_returns_the_actors_answer_on_the_receiver() {
        use super::super::McpManagerActor;
        let dir = tempfile::tempdir().unwrap();
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json")))
            .await
            .unwrap();
        tokio::spawn(actor.run());
        let rx = handle
            .add_transient_server_detached(McpManagerConfig::stdio(
                "plugin:qa/never",
                "never",
                "qa-nonexistent-mcp-binary-9f3a",
            ))
            .await
            .expect("enqueue");
        let answer = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .expect("actor answers within the spawn timeout")
            .expect("sender not dropped");
        assert!(answer.is_err(), "a nonexistent binary fails to start: {answer:?}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib register_transient_servers -- --nocapture && cargo test -p alephcore --lib add_transient_server_detached -- --nocapture`
Expected: FAIL to compile — `cannot find function register_transient_servers`; `no method named add_transient_server_detached`.

- [ ] **Step 3: Write minimal implementation**

`src/mcp/manager/handle.rs` — insert after `add_transient_server` (`:106-117`):

```rust
    /// Enqueue a transient server start and return the actor's eventual
    /// answer instead of awaiting it.
    ///
    /// `add_transient_server` blocks its caller for the child's `initialize`
    /// handshake (`actor.rs:603` → `start_server_internal`); the plugin
    /// lifecycle runs on the boot path and must not. The command is on the
    /// actor's single queue once this returns, so a `remove_transient_server`
    /// sent afterwards is processed after it — the caller may dispose
    /// immediately without racing the start.
    ///
    /// `Err` only when the actor is gone (command channel closed).
    pub async fn add_transient_server_detached(
        &self,
        config: McpManagerConfig,
    ) -> Result<oneshot::Receiver<std::result::Result<(), String>>> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(McpCommand::AddTransientServer { config, respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;
        Ok(rx)
    }
```

`src/extension/registrar/mcp_registrar.rs` — replace the header (`:1-8`):

```rust
//! MCP Registrar — per-agent MCP server scope (P3 Stage I)
//!
//! Historical note: this file used to host a `McpRegistrar` struct for a
//! two-phase `batch_register` write path. That API was superseded by
//! `McpManager::add_transient_server` driven by
//! `ExtensionManager::sync_mcp_plugin_servers`, so the struct has been
//! removed; the per-agent `McpScope` machinery below is the only remaining
//! production surface.
```

with:

```rust
//! MCP Registrar — plugin-owned transient servers as a disposable effect, and
//! the per-agent MCP server scope (P3 Stage I).
//!
//! Historical note: this file used to host a `McpRegistrar` struct for a
//! two-phase `batch_register` write path, then nothing plugin-shaped at all
//! while `ExtensionManager::sync_mcp_plugin_servers` did the registration
//! inline. [`register_transient_servers`] is the plugin path now: it is the
//! `mcp_server` effect `lifecycle.rs::mount` records, and its disposer is what
//! `unmount` runs. `McpScope` below is the per-agent (sub-agent) scope and is
//! unrelated to plugin mounting.
```

Append before `mod tests` (`:380`):

```rust
use crate::extension::effects::{async_disposer, Disposer};
use crate::mcp::{McpManagerConfig, McpManagerHandle};
use std::collections::HashMap;
use tokio::sync::oneshot;

/// The actor's answer to one enqueued transient-server start.
pub type ServerStartReceiver = oneshot::Receiver<Result<(), String>>;

/// Mount-time effect: hand every server a plugin's `.mcp.json` declares to
/// the MCP manager as a **transient** server, and return the disposer that
/// removes them all plus one receiver per server for the actor's verdict.
///
/// Each add is *enqueued*, not awaited (see
/// `McpManagerHandle::add_transient_server_detached`). The receivers are
/// returned to the caller — `lifecycle.rs::watch_server_starts` awaits them
/// on one task per plugin and logs each outcome; P3 turns that watcher into
/// the readiness (`Pending`) writer. A failed start is therefore NOT a mount
/// failure — exactly today's `sync_mcp_plugin_servers` contract
/// (`mod.rs:503-511`: warn and continue) — but a closed command channel is,
/// because then nothing can ever start.
///
/// Partial failure is all-or-none at this step's granularity: if the k-th
/// enqueue fails, the k−1 already enqueued are removed before returning
/// `Err`, so the caller never has to dispose a half-registered step.
pub(crate) async fn register_transient_servers(
    handle: McpManagerHandle,
    configs: HashMap<String, McpManagerConfig>,
) -> Result<(Disposer, Vec<(String, ServerStartReceiver)>), String> {
    let mut server_ids: Vec<String> = configs.keys().cloned().collect();
    server_ids.sort();
    let mut enqueued: Vec<String> = Vec::with_capacity(server_ids.len());
    let mut receivers: Vec<(String, ServerStartReceiver)> = Vec::with_capacity(server_ids.len());
    for server_id in &server_ids {
        let config = configs
            .get(server_id)
            .cloned()
            .expect("id came from this map");
        match handle.add_transient_server_detached(config).await {
            Ok(rx) => {
                enqueued.push(server_id.clone());
                receivers.push((server_id.clone(), rx));
            }
            Err(e) => {
                for already in &enqueued {
                    if let Err(re) = handle.remove_transient_server(already.clone()).await {
                        tracing::warn!(server_id = %already, error = %re, "rollback remove failed");
                    }
                }
                return Err(format!("cannot enqueue MCP server '{server_id}': {e}"));
            }
        }
    }
    let disposer = async_disposer(move || async move {
        let mut failures = Vec::new();
        for server_id in enqueued {
            if let Err(e) = handle.remove_transient_server(server_id.clone()).await {
                failures.push(format!("{server_id}: {e}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!("remove_transient_server failed for {}", failures.join("; ")))
        }
    });
    Ok((disposer, receivers))
}
```

(`crate::mcp::McpManagerConfig` / `McpManagerHandle` are the paths `loader.rs:30` and `mod.rs:213` already use.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib register_transient_servers -- --nocapture && cargo test -p alephcore --lib add_transient_server_detached -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/mcp/manager/handle.rs src/extension/registrar/mcp_registrar.rs
git commit -m "extension/registrar: register_transient_servers enqueues plugin MCP servers, returns their Disposer + start receivers

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.5: `service` producer — `ExtensionManager::start_services_effect` → `Disposer`

**Files:**
- Modify: `src/extension/service_manager.rs:375-420` (add `forget_plugin` after `stop_plugin_services`)
- Modify: `src/extension/service_ops.rs:1-7` (imports); insert the producer after `stop_service` (`:41-56`); rename `start_autostart_services` (`:57-119`) to `start_autostart_services_transitional` (see the transitional note)
- Test: `src/extension/service_ops.rs` (new `mod tests`), `src/extension/service_manager.rs` (`mod tests` at `:500`)

Today the start half is `start_autostart_services` (`service_ops.rs:57-119`, `pub(crate)`, called by `load_runtime_plugin` `plugin_ops.rs:216` and `sync_plugin_services` `service_ops.rs:154`) and the stop half is inlined in `unload_runtime_plugin` (`plugin_ops.rs:237-266`). Both callers die in P1.11; this task turns the pair into one producer. `ServiceManager` is synchronous (`&mut self`, needs `&PluginLoader` for the guest call) and lives behind `Arc<tokio::sync::RwLock<ServiceManager>>` (`mod.rs:163`); guest calls run on `spawn_blocking` with owned guards — the pattern at `service_ops.rs:74-116` is kept verbatim.

`stop_plugin_services` (`service_manager.rs:381-420`) leaves `Stopped`/`Failed` entries in `services` (only `stop_orphaned` removes them, `:466-472`). A disposer that leaves rows behind is not an inverse, so a `forget_plugin` that drops the plugin's entries after stopping is added.

**Interfaces:**
- Consumes: `ServiceManager::{start_service, stop_plugin_services}`, `PluginRegistry::list_services`, `Disposer`/`async_disposer`.
- Produces:
  ```rust
  impl ServiceManager { pub fn forget_plugin(&mut self, plugin_id: &str) -> usize }
  impl ExtensionManager {
      /// The `service` effect: start the plugin's `auto_start` services now;
      /// the disposer stops EVERY registered service of the plugin (autostarted
      /// or started later via `services.start`) and forgets their rows.
      pub(crate) async fn start_services_effect(&self, plugin_id: &str) -> Disposer
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `src/extension/service_manager.rs`:

```rust
    #[test]
    fn forget_plugin_drops_only_that_plugins_rows() {
        let mut sm = ServiceManager::new();
        let loader = PluginLoader::new();
        let a = ServiceRegistration {
            id: "svc".into(),
            name: "svc".into(),
            start_handler: "start".into(),
            stop_handler: "stop".into(),
            plugin_id: "a".into(),
            auto_start: true,
        };
        let b = ServiceRegistration {
            plugin_id: "b".into(),
            ..a.clone()
        };
        // Both fail to start (no runtime loaded) and are recorded as Failed rows.
        let _ = sm.start_service(&a, &loader);
        let _ = sm.start_service(&b, &loader);
        assert_eq!(sm.list_services().len(), 2);
        assert_eq!(sm.forget_plugin("a"), 1);
        assert_eq!(sm.list_services().len(), 1);
        assert_eq!(sm.list_services()[0].plugin_id, "b");
        assert_eq!(sm.forget_plugin("a"), 0, "idempotent");
    }
```

Create `mod tests` at the end of `src/extension/service_ops.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::discovery::DiscoveryConfig;
    use crate::extension::{ExtensionConfig, ExtensionManager, ServiceRegistration};
    use crate::extension::types::ServiceState;

    /// A manager whose registry holds one WASM plugin with one autostart
    /// service, backed by the empty module fixture (`loader.rs::EMPTY_WASM_MODULE`):
    /// the module links, the `start_ticker` export does not exist, so the
    /// start is recorded as `Failed` — which is the interesting row for a
    /// disposer to have to clean up.
    async fn manager_with_one_service(
        tmp: &std::path::Path,
    ) -> (ExtensionManager, crate::utils::paths::IsolatedAlephHome) {
        // Held by the caller for the whole test: `Config::load()` writes a
        // default config under the Aleph home when none exists (see the
        // `IsolatedAlephHome` doc) and the manager may touch it after `new`.
        let home = crate::utils::paths::IsolatedAlephHome::new();
        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: tmp.to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 0,
            },
            plugins_config_path: Some(tmp.join("plugins.toml")),
            extra_plugin_parents: vec![],
        })
        .await
        .unwrap();
        std::fs::write(tmp.join("plugin.wasm"), crate::extension::loader::EMPTY_WASM_MODULE).unwrap();
        let mut manifest = crate::extension::PluginManifest::new(
            "qa-wasm".into(),
            "QA WASM".into(),
            crate::extension::PluginKind::Wasm,
            std::path::PathBuf::from("plugin.wasm"),
        );
        manifest.root_dir = tmp.to_path_buf();
        manager
            .get_plugin_loader_for_test()
            .write()
            .await
            .load_plugin(&manifest)
            .unwrap();
        {
            let mut reg = manager.get_plugin_registry_mut().await;
            reg.register_plugin(crate::extension::PluginRecord::new(
                "qa-wasm".into(),
                "QA WASM".into(),
                crate::extension::PluginKind::Wasm,
                crate::extension::PluginOrigin::Global,
            ));
            reg.register_service(ServiceRegistration {
                id: "ticker".into(),
                name: "ticker".into(),
                start_handler: "start_ticker".into(),
                stop_handler: "stop_ticker".into(),
                plugin_id: "qa-wasm".into(),
                auto_start: true,
            });
        }
        (manager, home)
    }

    #[tokio::test]
    async fn start_services_effect_starts_autostart_rows_and_its_disposer_forgets_them() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, _home) = manager_with_one_service(tmp.path()).await;

        let disposer = manager.start_services_effect("qa-wasm").await;
        let rows = manager.list_services().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].id, "ticker");
        assert_eq!(rows[0].state, ServiceState::Failed, "empty module has no start_ticker export");

        let outcome = disposer().await;
        // The stop handler does not exist either, so the stop is reported —
        // honestly — as a failure with the service named...
        assert!(outcome.as_ref().is_err_and(|e| e.contains("ticker")), "{outcome:?}");
        // ...and the rows are gone regardless: a disposer that leaves state
        // behind is not an inverse.
        assert!(manager.list_services().await.is_empty());
    }

    #[tokio::test]
    async fn start_services_effect_with_no_registrations_is_an_empty_disposer() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, _home) = manager_with_one_service(tmp.path()).await;
        let disposer = manager.start_services_effect("someone-else").await;
        assert!(manager.list_services().await.is_empty());
        disposer().await.unwrap();
    }
}
```

`get_plugin_loader_for_test` does not exist; `get_plugin_loader` (`plugin_ops.rs:168`) returns a read guard. Add to `plugin_ops.rs` next to it:

```rust
    /// Test-only: the loader handle itself, so a fixture can pre-load a module.
    #[cfg(test)]
    pub(crate) fn get_plugin_loader_for_test(&self) -> Arc<tokio::sync::RwLock<super::PluginLoader>> {
        self.plugin_loader.clone()
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::service_ops -- --nocapture && cargo test -p alephcore --lib forget_plugin_drops -- --nocapture`
Expected: FAIL to compile — `no method named start_services_effect`, `no method named forget_plugin`.

- [ ] **Step 3: Write minimal implementation**

`src/extension/service_manager.rs` — insert after `stop_plugin_services` (`:381-420`):

```rust
    /// Drop every row this plugin has in the state and registration tables.
    /// Called by the `service` effect's disposer after `stop_plugin_services`,
    /// so an unmounted plugin leaves no `Stopped`/`Failed` ghosts behind for
    /// `services.list` to show. Returns how many rows were dropped.
    pub fn forget_plugin(&mut self, plugin_id: &str) -> usize {
        let before = self.services.len();
        self.services.retain(|_, info| info.plugin_id != plugin_id);
        self.registrations.retain(|_, reg| reg.plugin_id != plugin_id);
        before - self.services.len()
    }
```

`src/extension/service_ops.rs` — imports (`:3-6`) become:

```rust
use crate::extension::effects::{async_disposer, Disposer};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::registry::ServiceRegistration;
use crate::extension::types::{ServiceInfo, ServiceState};
```

Insert after `stop_service` (`:41-56`) — the start loop is the body of `start_autostart_services` (`:63-119`) carried over, the stop half is `unload_runtime_plugin`'s (`plugin_ops.rs:237-266`) carried over:

```rust
    /// The `service` effect for one mounted plugin.
    ///
    /// Starts every `auto_start` service the registry holds for `plugin_id`
    /// (the plugin's runtime must already be loaded — `wasm_module` is the
    /// step before this one). The returned disposer stops EVERY registered
    /// service of the plugin, autostarted or started later through
    /// `services.start`, then forgets their rows — so a manual start never
    /// outlives the mount (G1 exempts `start_service` on exactly this ground).
    ///
    /// Start failures are recorded on the `ServiceInfo` row (`Failed`) and do
    /// not fail the mount; the disposer reports a stop failure by service id.
    pub(crate) async fn start_services_effect(&self, plugin_id: &str) -> Disposer {
        let registrations: Vec<ServiceRegistration> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_services()
                .into_iter()
                .filter(|s| s.plugin_id == plugin_id)
                .cloned()
                .collect()
        };
        let pending: Vec<ServiceRegistration> = registrations
            .iter()
            .filter(|s| s.auto_start)
            .cloned()
            .collect();
        if !pending.is_empty() {
            let service_manager = self.service_manager.clone().write_owned().await;
            let loader = self.plugin_loader.clone().read_owned().await;
            let id = plugin_id.to_string();
            // The start handler is untrusted guest code — off the async pool.
            let join = tokio::task::spawn_blocking(move || {
                let mut service_manager = service_manager;
                for registration in &pending {
                    match service_manager.start_service(registration, &loader) {
                        Ok(info) if info.state == ServiceState::Running => {}
                        Ok(info) => tracing::warn!(
                            plugin = %id, service = %registration.id, state = ?info.state,
                            error = ?info.error, "autostart service did not reach Running state"
                        ),
                        Err(e) => tracing::warn!(
                            plugin = %id, service = %registration.id, error = %e,
                            "failed to autostart service"
                        ),
                    }
                }
            })
            .await;
            if let Err(e) = join {
                tracing::warn!(error = %e, "autostart services task join failed");
            }
        }

        let service_manager = self.service_manager.clone();
        let loader = self.plugin_loader.clone();
        let id = plugin_id.to_string();
        async_disposer(move || async move {
            let sm = service_manager.write_owned().await;
            let ld = loader.read_owned().await;
            let plugin = id.clone();
            let results = tokio::task::spawn_blocking(move || {
                let mut sm = sm;
                let results = sm.stop_plugin_services(&plugin, &registrations, &ld);
                sm.forget_plugin(&plugin);
                results
            })
            .await
            .map_err(|e| format!("stop_plugin_services task join failed: {e}"))?;
            let failed: Vec<String> = results
                .iter()
                .filter(|i| i.state == ServiceState::Failed)
                .map(|i| format!("{}:{}", i.plugin_id, i.id))
                .collect();
            if failed.is_empty() {
                Ok(())
            } else {
                Err(format!("services failed to stop cleanly: {}", failed.join(", ")))
            }
        })
    }
```

Transitional (deleted in P1.11): `start_autostart_services` is still called by `load_runtime_plugin` (`plugin_ops.rs:216`) and `sync_plugin_services` (`service_ops.rs:154`). Do NOT delete it here — rename it `start_autostart_services_transitional` (body unchanged, still returns the Running count, visibility `pub(crate)` → private `async fn`), and rename both call sites. The producer above is a new function beside it; P1.11 removes the transitional one together with its two callers.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::service_ops -- --nocapture && cargo test -p alephcore --lib extension::service_manager -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/extension/service_manager.rs src/extension/service_ops.rs src/extension/plugin_ops.rs
git commit -m "extension/service_ops: start_services_effect returns the Disposer that stops and forgets the plugin's services

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.6: `memory_extension` producer — `MemoryExtensionRegistry::unregister` + `loader::register_memory_extension_effect` → `Disposer`

**Files:**
- Modify: `src/memory/extensions/registry.rs:90-135` (add `unregister` after `register_mcp`)
- Modify: `src/extension/loader.rs:478-500` (replace `register_memory_extension_if_declared`) and `:615-650` (replace its two tests)
- Test: `src/memory/extensions/registry.rs` (`mod tests`, existing), `src/extension/loader.rs` (`mod tests`)

Registrations are keyed by `MemoryExtension::name()` (`registry.rs:98-100`, `:117-121`); the plugin path registers under `manifest.name` (`loader.rs:492`). The disposer captures the exact name it registered, so no plugin-id side table is needed. The boot-time bind (`bind_memory_callers`, `mod.rs:420-450`: for each MCP-backed extension, `rebind(ManagerBackedMcpCaller::new(handle, server_id))`) is folded into the producer: with the MCP handle installed before the first mount (P1.8) the extension is bound at registration and `bind_memory_callers` loses its purpose (deleted in P1.11).

**Interfaces:**
- Consumes: `McpMemoryExtension::{new_unbound, rebind, name}` (`mcp_adapter.rs:55-74`), `ManagerBackedMcpCaller::new(handle, server_id)` (`mcp_adapter.rs:283`), `MemoryExtensionRegistry::register_mcp` (`registry.rs:114-129`).
- Produces:
  ```rust
  impl MemoryExtensionRegistry { pub fn unregister(&self, name: &str) -> bool }
  pub(crate) fn register_memory_extension_effect(
      manifest: &PluginManifest,
      server_id: Option<String>,
      registry: &Arc<MemoryExtensionRegistry>,
      mcp_handle: Option<McpManagerHandle>,
  ) -> Result<Option<Disposer>, String>   // Ok(None) when the manifest has no [memory] section
  ```

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `src/memory/extensions/registry.rs` (after `register_mcp_appears_in_both_dispatch_and_snapshot`, `:880-896`):

```rust
    #[tokio::test]
    async fn unregister_removes_from_dispatch_and_from_the_mcp_side_table() {
        use crate::memory::extensions::mcp_adapter::McpMemoryExtension;
        let reg = MemoryExtensionRegistry::new();
        reg.register_mcp(Arc::new(McpMemoryExtension::new_unbound(
            "p".to_string(),
            Some("plugin:p/srv".to_string()),
        )))
        .unwrap();
        reg.register(Arc::new(RecordDelegationExt {
            seen: Arc::new(Mutex::new(Vec::new())),
        }))
        .unwrap();
        assert_eq!(reg.len(), 2);

        assert!(reg.unregister("p"), "a registered name is removed");
        assert_eq!(reg.len(), 1, "the other extension is untouched");
        assert!(reg.mcp_bindings_snapshot().is_empty(), "side-table entry goes with it");
        assert!(!reg.unregister("p"), "second call: nothing to remove");

        // The name is free again: re-registering does not hit the dedup error.
        reg.register_mcp(Arc::new(McpMemoryExtension::new_unbound(
            "p".to_string(),
            None,
        )))
        .unwrap();
        assert_eq!(reg.len(), 2);
    }
```

Replace the two tests `register_memory_extension_registers_when_memory_section_present` and `register_memory_extension_skips_when_no_memory_section` (`loader.rs:623-650`) with:

```rust
    #[tokio::test]
    async fn memory_extension_effect_registers_and_its_disposer_unregisters() {
        let manifest = make_manifest_with_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let disposer = register_memory_extension_effect(
            &manifest,
            Some("plugin:test/srv".to_string()),
            &registry,
            None,
        )
        .expect("fresh name registers")
        .expect("[memory] section present → an effect");
        assert_eq!(registry.len(), 1);
        let snap = registry.mcp_bindings_snapshot();
        assert_eq!(snap[0].name(), "Test Memory Plugin", "keyed by manifest.name");
        assert_eq!(snap[0].server_id(), Some("plugin:test/srv"));

        disposer().await.unwrap();
        assert_eq!(registry.len(), 0);
        assert!(registry.mcp_bindings_snapshot().is_empty());
    }

    #[test]
    fn memory_extension_effect_is_none_without_a_memory_section() {
        let manifest = make_manifest_no_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let effect = register_memory_extension_effect(&manifest, None, &registry, None).unwrap();
        assert!(effect.is_none());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn memory_extension_effect_refuses_a_duplicate_name_loudly() {
        let manifest = make_manifest_with_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let _first = register_memory_extension_effect(&manifest, None, &registry, None)
            .unwrap()
            .unwrap();
        let err = register_memory_extension_effect(&manifest, None, &registry, None)
            .err()
            .expect("a second plugin claiming the same extension name is a mount failure, not a warn");
        assert!(err.contains("Test Memory Plugin"), "{err}");
        assert_eq!(registry.len(), 1, "the loser registered nothing");
    }

    /// With a live MCP handle the extension is bound at registration — the
    /// boot-time `bind_memory_callers` pass this replaces is no longer needed.
    #[tokio::test]
    async fn memory_extension_effect_binds_the_caller_when_a_handle_is_present() {
        let dir = tempfile::tempdir().unwrap();
        let (actor, handle) =
            crate::mcp::manager::McpManagerActor::new(Some(dir.path().join("mcp.json")))
                .await
                .unwrap();
        tokio::spawn(actor.run());
        let manifest = make_manifest_with_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let _d = register_memory_extension_effect(
            &manifest,
            Some("plugin:test/srv".to_string()),
            &registry,
            Some(handle),
        )
        .unwrap()
        .unwrap();
        let ext = &registry.mcp_bindings_snapshot()[0];
        // `UnboundMcpCaller` answers every call with its diagnostic error;
        // a bound caller reaches the manager and gets "server not found".
        let err = ext
            .call_for_test("noop", serde_json::json!({}))
            .await
            .err()
            .expect("no such server on a fresh manager");
        assert!(!err.to_string().contains("not yet bound"), "must be bound: {err}");
    }
```

`call_for_test` does not exist on `McpMemoryExtension`; add to `src/memory/extensions/mcp_adapter.rs` inside `impl McpMemoryExtension` (after `server_id`, `:72-74`):

```rust
    /// Test-only: call the current caller directly, to observe whether the
    /// extension is bound to a manager or still on `UnboundMcpCaller`.
    #[cfg(test)]
    pub(crate) async fn call_for_test(
        &self,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, AlephError> {
        let caller = self.caller.load_full();
        caller.call(tool, args).await
    }
```

(`McpCaller::call(&self, method: &str, args: Value)` is the trait's only method, `mcp_adapter.rs:20-22`; `caller` is `ArcSwap<Arc<dyn McpCaller>>`, `:37`. `UnboundMcpCaller`'s error text contains "not yet bound", `:265-269`, which is what the test excludes.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib unregister_removes_from_dispatch -- --nocapture && cargo test -p alephcore --lib memory_extension_effect -- --nocapture`
Expected: FAIL to compile — `no method named unregister`, `cannot find function register_memory_extension_effect`.

- [ ] **Step 3: Write minimal implementation**

`src/memory/extensions/registry.rs` — insert after `register_mcp` (`:114-129`):

```rust
    /// Remove the extension registered under `name` from the dispatch list
    /// and, if it was MCP-backed, from the typed side-table. Returns whether
    /// anything was removed.
    ///
    /// The inverse of [`register`] / [`register_mcp`]; the plugin lifecycle
    /// calls it from the `memory_extension` effect's disposer. Until it
    /// existed a `[memory]` extension outlived its plugin's disable and kept
    /// routing hooks to a transient MCP server that had already been removed.
    pub fn unregister(&self, name: &str) -> bool {
        let removed = {
            let mut guard = self.extensions.write().unwrap_or_else(|e| e.into_inner());
            let before = guard.len();
            guard.retain(|e| e.name() != name);
            before != guard.len()
        };
        {
            let mut bindings = self.mcp_bindings.write().unwrap_or_else(|e| e.into_inner());
            bindings.retain(|e| e.name() != name);
        }
        removed
    }
```

`src/extension/loader.rs` — replace `register_memory_extension_if_declared` (`:478-500`, from `/// Register a `McpMemoryExtension` into `registry` when `manifest` declares a` through the closing `}`) with:

```rust
/// The `memory_extension` effect: register a `McpMemoryExtension` when
/// `manifest` declares a `[memory]` section, bound to the live MCP manager
/// when one is attached, and return the disposer that unregisters it.
///
/// `Ok(None)` when there is no `[memory]` section — nothing to register,
/// nothing to dispose. `Err` when the name is already taken: two plugins
/// claiming one extension name would double-fire every hook, so the second
/// mount fails at this step instead of logging and carrying on.
///
/// Free function (not a `PluginLoader` method) because it never touches the
/// loader; it lives here so the G1 census finds every plugin-facing memory
/// registration in one file.
pub(crate) fn register_memory_extension_effect(
    manifest: &PluginManifest,
    server_id: Option<String>,
    registry: &Arc<MemoryExtensionRegistry>,
    mcp_handle: Option<crate::mcp::McpManagerHandle>,
) -> Result<Option<Disposer>, String> {
    if manifest.memory_manifest.is_none() {
        return Ok(None);
    }
    let ext = Arc::new(McpMemoryExtension::new_unbound(
        manifest.name.clone(),
        server_id.clone(),
    ));
    if let (Some(handle), Some(sid)) = (mcp_handle, server_id) {
        ext.rebind(Arc::new(
            crate::memory::extensions::ManagerBackedMcpCaller::new(handle, sid),
        ));
    }
    let name = manifest.name.clone();
    registry
        .register_mcp(Arc::clone(&ext))
        .map_err(|e| format!("memory extension '{name}' not registered: {e}"))?;
    info!(plugin = %name, "registered McpMemoryExtension for plugin with [memory] section");
    let registry = Arc::clone(registry);
    Ok(Some(crate::extension::effects::sync_disposer(move || {
        if registry.unregister(&name) {
            Ok(())
        } else {
            Err(format!("memory extension '{name}' was not registered at dispose time"))
        }
    })))
}
```

The `use crate::memory::extensions::{McpMemoryExtension, MemoryExtensionRegistry};` import at `:31` stays. `load_plugin_with_memory` (`:216-243`) still calls the old name; until P1.11 deletes it, change its last statement `register_memory_extension_if_declared(manifest, server_id, memory_registry);` to:

```rust
        // Transitional (deleted in P1.11): the disposer is dropped here
        // because this caller has no scope to put it in.
        if let Err(e) = register_memory_extension_effect(manifest, server_id, memory_registry, None) {
            warn!(plugin = %manifest.id, error = %e, "memory extension not registered");
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib memory::extensions::registry -- --nocapture && cargo test -p alephcore --lib extension::loader -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/memory/extensions/registry.rs src/memory/extensions/mcp_adapter.rs src/extension/loader.rs
git commit -m "memory/extensions: unregister(name) + register_memory_extension_effect returns its Disposer

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.7: `slash_command` producer — `ToolCatalog::unregister_skills` + `slash_effect::register_slash_commands_effect` → `Disposer` + `ExtensionManager::set_tool_catalog`

**Files:**
- Modify: `src/tool_metadata/registry/state.rs:39-56` (add `remove_skills` after `remove_by_mcp_server`)
- Modify: `src/tool_metadata/registry/mod.rs:201-208` (add `unregister_skills` after `remove_by_mcp_server`)
- Create: `src/extension/slash_effect.rs`
- Modify: `src/extension/mod.rs:26-52` (module list), `:147-245` (`ExtensionManager` fields), `:301-357` (constructor), `:398-410` (add `set_tool_catalog` next to `set_mcp_handle`)
- Test: `src/tool_metadata/registry/tests.rs` (existing test module), `src/extension/slash_effect.rs` (`mod tests`)

Today plugin slash entries are registered once at boot in `tool_catalog_init.rs:214-247`: `get_all_commands()` → one `SkillInfo { id: cmd.qualified_name(), name, description, scope: System, version: None, allowed_tools: None }` per command (the literal at `:225-240`) → `tool_catalog.register_skills(&infos)`. **This task's `plugin_command_skill_info` replaces that literal; P1.10 deletes the whole block.** `register_skills` (`registration.rs:229-306`) stores each as `UnifiedTool` with `ToolSource::Skill { id }`. There is no removal path; `ToolState::remove_by_mcp_server` (`state.rs:39-56`) is the shape to mirror. The catalog is not reachable from the manager: it is built in `init_tool_catalog` (`tool_catalog_init.rs:45`) and threaded by parameter; this task adds the manager-side handle in the same `RwLock<Option<_>>` + setter shape as `mcp_handle` (`mod.rs:206-213`, `:410-412`).

**Interfaces:**
- Consumes: `ToolCatalog::register_skills` (`registry/mod.rs:146-153`), `SkillInfo` (`skill/compat.rs:17`), `SkillRegistration::qualified_name` (`registry/types.rs:233`), `Disposer`/`async_disposer`.
- Produces:
  ```rust
  impl ToolState   { pub async fn remove_skills(&self, skill_ids: &[String]) -> usize }
  impl ToolCatalog { pub async fn unregister_skills(&self, skill_ids: &[String]) -> usize }
  // src/extension/slash_effect.rs
  /// R1.3: THE builder of a plugin command's `SkillInfo` — the only place that shape is
  /// written for plugin commands. Replaces the literal at `tool_catalog_init.rs:225-240`
  /// (the block `:214-247` is deleted by P1.10). P2.8 adds `plugin_id` to it; P4.7b projects
  /// `argument_hint` / `allowed_tools` / `model` into it.
  pub(crate) fn plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo;
  /// Filter (`skill_type == Command`, non-empty `plugin_id`) + map over the builder.
  pub(crate) fn plugin_command_skill_infos(commands: &[SkillRegistration]) -> Vec<SkillInfo>;
  pub(crate) async fn register_slash_commands_effect(catalog: Arc<ToolCatalog>, infos: Vec<SkillInfo>) -> Disposer;
  impl ExtensionManager { pub fn set_tool_catalog(&self, catalog: Arc<ToolCatalog>) }
  ```

- [ ] **Step 1: Write the failing test**

Append to the test module in `src/tool_metadata/registry/tests.rs` (open the file to find the `use` block and the helper that builds a `SkillInfo`, if any — otherwise construct one inline as below):

```rust
    #[tokio::test]
    async fn unregister_skills_removes_exactly_the_named_skill_entries() {
        let catalog = ToolCatalog::new();
        let mk = |id: &str| crate::skill::SkillInfo {
            id: id.to_string(),
            name: id.to_string(),
            description: format!("{id} desc"),
            scope: crate::domain::skill::PromptScope::System,
            version: None,
            allowed_tools: None,
        };
        let rejected = catalog
            .register_skills(&[mk("qa-plug:hello"), mk("qa-plug:bye"), mk("other:keep")])
            .await;
        assert!(rejected.is_empty());
        let names = |tools: Vec<UnifiedTool>| -> Vec<String> { tools.into_iter().map(|t| t.name).collect() };
        let before = names(catalog.list_all().await);
        assert!(before.contains(&"qa-plug:hello".to_string()));

        let removed = catalog
            .unregister_skills(&["qa-plug:hello".to_string(), "qa-plug:bye".to_string()])
            .await;
        assert_eq!(removed, 2);
        let after = names(catalog.list_all().await);
        assert!(!after.contains(&"qa-plug:hello".to_string()));
        assert!(!after.contains(&"qa-plug:bye".to_string()));
        assert!(after.contains(&"other:keep".to_string()), "unrelated skill untouched");

        assert_eq!(catalog.unregister_skills(&["qa-plug:hello".to_string()]).await, 0, "idempotent");
    }
```

Create `src/extension/slash_effect.rs` with only its test module (RED — the functions do not exist yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::registry::SkillRegistration;
    use crate::extension::types::SkillType;
    use crate::sync_primitives::Arc;
    use crate::tool_metadata::ToolCatalog;

    fn cmd(plugin: &str, name: &str) -> SkillRegistration {
        SkillRegistration {
            name: name.to_string(),
            plugin_id: plugin.to_string(),
            skill_type: SkillType::Command,
            description: format!("{name} description"),
            ..Default::default()
        }
    }

    #[test]
    fn plugin_command_skill_info_projects_the_qualified_name_and_nothing_else() {
        let info = plugin_command_skill_info(&cmd("qa-plug", "hello"));
        assert_eq!(info.id, "qa-plug:hello", "registry key = slash id");
        assert_eq!(info.name, "hello");
        assert_eq!(info.description, "hello description");
        assert_eq!(info.scope, crate::domain::skill::PromptScope::System);
        assert!(info.version.is_none(), "plugin commands carry no manifest version");
        assert!(info.allowed_tools.is_none(), "no `allowed-tools:` → full tool surface");
    }

    #[test]
    fn plugin_command_skill_infos_keeps_only_plugin_commands() {
        let infos = plugin_command_skill_infos(&[cmd("qa-plug", "hello")]);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "qa-plug:hello");
        // A registration that is not a Command is not a slash entry…
        let mut skill = cmd("qa-plug", "not-a-command");
        skill.skill_type = SkillType::Skill;
        assert!(plugin_command_skill_infos(&[skill]).is_empty());
        // …and neither is a command with no owning plugin.
        let orphan = cmd("", "loose");
        assert!(plugin_command_skill_infos(&[orphan]).is_empty());
    }

    #[tokio::test]
    async fn register_slash_commands_effect_round_trips_the_catalog() {
        let catalog = Arc::new(ToolCatalog::new());
        let baseline: Vec<String> = catalog.list_all().await.into_iter().map(|t| t.name).collect();
        let infos = plugin_command_skill_infos(&[cmd("qa-plug", "hello"), cmd("qa-plug", "bye")]);
        let disposer = register_slash_commands_effect(Arc::clone(&catalog), infos).await;
        let during: Vec<String> = catalog.list_all().await.into_iter().map(|t| t.name).collect();
        assert!(during.contains(&"qa-plug:hello".to_string()));
        assert!(during.contains(&"qa-plug:bye".to_string()));
        disposer().await.unwrap();
        let after: Vec<String> = catalog.list_all().await.into_iter().map(|t| t.name).collect();
        assert_eq!(after, baseline, "unmount leaves the catalog exactly as it was");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib unregister_skills_removes -- --nocapture && cargo test -p alephcore --lib extension::slash_effect -- --nocapture`
Expected: FAIL to compile — `no method named unregister_skills`; `cannot find function plugin_command_skill_infos` (and `slash_effect` is not yet a module — add `mod slash_effect;` to `src/extension/mod.rs` first so the failure is the missing functions, not the missing module).

- [ ] **Step 3: Write minimal implementation**

`src/tool_metadata/registry/state.rs` — insert after `remove_by_mcp_server` (`:39-56`):

```rust
    /// Remove the skill-sourced entries whose `ToolSource::Skill { id }` is
    /// one of `skill_ids`. The inverse of `ToolRegistrar::register_skills`
    /// for a plugin's `commands/*.md` entries; the plugin lifecycle calls it
    /// from the `slash_command` effect's disposer with the exact ids it
    /// registered. Returns how many were removed.
    pub async fn remove_skills(&self, skill_ids: &[String]) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();
        tools.retain(|_, tool| match &tool.source {
            super::super::types::ToolSource::Skill { id } => !skill_ids.contains(id),
            _ => true,
        });
        let removed = initial_count - tools.len();
        debug!(removed, "Removed plugin skill entries");
        removed
    }
```

`src/tool_metadata/registry/mod.rs` — insert after `remove_by_mcp_server` (`:201-208`):

```rust
    /// Remove the slash entries registered for the given skill ids.
    pub async fn unregister_skills(&self, skill_ids: &[String]) -> usize {
        let n = self.state.remove_skills(skill_ids).await;
        if n > 0 {
            self.health.invalidate_all();
        }
        n
    }
```

`src/extension/slash_effect.rs` (above the test module):

```rust
//! The `slash_command` effect: a plugin's `commands/*.md` as `/name` entries
//! in the `ToolCatalog`, registered at mount and removed at unmount.
//!
//! Until this file existed the entries were written once at boot
//! (`bin/aleph-server/…/tool_catalog_init.rs`) and never removed, so a plugin
//! enabled after boot had no slash commands and a disabled one kept its
//! entries for the life of the process (scan-aleph-plugins.md §7 #5).

use crate::extension::effects::{async_disposer, Disposer};
use crate::extension::registry::SkillRegistration;
use crate::extension::types::SkillType;
use crate::skill::SkillInfo;
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolCatalog;

/// THE builder of a plugin command's `SkillInfo` — the one place this shape
/// is written for plugin commands (it replaces the literal that lived in
/// `bin/aleph-server/…/tool_catalog_init.rs`). The id is `qualified_name()`
/// (`<plugin>:<name>`), the registry's own key derivation, so the dispatch id
/// and the lookup key cannot drift apart. Later rounds add fields HERE
/// (`plugin_id` for visibility, `argument_hint` / `allowed_tools` / `model`
/// from the command's frontmatter), never at a second construction site.
pub(crate) fn plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo {
    SkillInfo {
        id: cmd.qualified_name(),
        name: cmd.name.clone(),
        description: cmd.description.clone(),
        // Plugin commands have no SkillManifest behind them; System is the
        // manifest default and matches every other slash command.
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        // No `allowed-tools:` to project: `None` keeps the agent's full tool
        // surface, which is what plugin commands have always done.
        allowed_tools: None,
    }
}

/// The plugin-command subset of a capability list, projected through
/// [`plugin_command_skill_info`].
pub(crate) fn plugin_command_skill_infos(commands: &[SkillRegistration]) -> Vec<SkillInfo> {
    commands
        .iter()
        .filter(|c| c.skill_type == SkillType::Command && !c.plugin_id.is_empty())
        .map(plugin_command_skill_info)
        .collect()
}

/// Register the entries and return the disposer that removes exactly them.
/// A skill the catalog refuses (unknown `allowed-tools:` name — impossible
/// here, `allowed_tools` is always `None`) would simply not be in the id
/// list the disposer removes.
pub(crate) async fn register_slash_commands_effect(
    catalog: Arc<ToolCatalog>,
    infos: Vec<SkillInfo>,
) -> Disposer {
    let rejected = catalog.register_skills(&infos).await;
    let ids: Vec<String> = infos
        .into_iter()
        .map(|i| i.id)
        .filter(|id| !rejected.contains(id))
        .collect();
    async_disposer(move || async move {
        let removed = catalog.unregister_skills(&ids).await;
        if removed == ids.len() {
            Ok(())
        } else {
            Err(format!(
                "expected to remove {} slash entries, removed {removed}",
                ids.len()
            ))
        }
    })
}
```

`src/extension/mod.rs`:

1. Module list (`:26-52`): add `mod slash_effect;` after `mod skill_tool;`.
2. Field — after `mcp_handle` (`:206-213`) add:

```rust
    /// Live tool catalog, so `mount` can register a plugin's `commands/*.md`
    /// as slash entries and `unmount` can remove them. `None` until
    /// [`Self::set_tool_catalog`] is called at server boot (CLI/test paths
    /// leave it unset, and the `slash_command` step is recorded as skipped).
    /// Same injection shape as `mcp_handle` and `memory_registry`, for the
    /// same reason: the manager is behind an `Arc` before the catalog exists.
    tool_catalog: crate::sync_primitives::RwLock<Option<Arc<crate::tool_metadata::ToolCatalog>>>,
```

3. Constructor (`:334-357`): add `tool_catalog: crate::sync_primitives::RwLock::new(None),` after `mcp_handle: crate::sync_primitives::RwLock::new(None),`.
4. Setter — after `set_mcp_handle` (`:410-412`) add:

```rust
    /// Inject the live tool catalog after construction. Call once at server
    /// boot BEFORE the first `load_all`, so every plugin's slash entries are
    /// registered by its own mount rather than by a boot-time catch-up.
    pub fn set_tool_catalog(&self, catalog: Arc<crate::tool_metadata::ToolCatalog>) {
        *self.tool_catalog.write().unwrap_or_else(|e| e.into_inner()) = Some(catalog);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib unregister_skills_removes -- --nocapture && cargo test -p alephcore --lib extension::slash_effect -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/tool_metadata/registry/state.rs src/tool_metadata/registry/mod.rs src/tool_metadata/registry/tests.rs src/extension/slash_effect.rs src/extension/mod.rs
git commit -m "extension/slash_effect: plugin slash entries as a Disposer-returning effect; ToolCatalog::unregister_skills

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.8: boot order — install the MCP handle, memory registry and tool catalog BEFORE the first `load_all`

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:185` (construct the catalog next to `tool_health`), `:319-331` (after `memory_ext_registry`: inject all three handles), `:1959-1970` (pass the catalog into `init_tool_catalog`)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:25-45` (signature + construction), `:506-520` (delete the late `set_memory_registry`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs:1441-1447` (delete the late `set_mcp_handle`)
- Test: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` (a source-level ordering guard; `cargo test -p alephcore --bins` is the only run that reaches `src/bin/` tests — CLAUDE.md verification set)

Today's order on the boot path: `ensure_loaded()` at `agent_init/mod.rs:600` (first `load_all`) → catalog built at `tool_catalog_init.rs:45` → `set_memory_registry` at `tool_catalog_init.rs:518` → `set_mcp_handle` at `start/mod.rs:1445` inside a spawned task. After this task, every handle `mount` needs exists before the first `load_all`; the spawned catch-up task keeps calling `sync_mcp_plugin_servers` / `bind_memory_callers` / `sync_plugin_services` until P1.11 deletes it (they are idempotent). `mcp_handle` is created at `start/mod.rs:266` and the actor is running by `:571-585`, both before the agent handlers builder is called (`:1400-1419`), and it reaches `agent_init` as the `hub_mcp_handle` parameter (`:165`). `memory_ext_registry` is built at `agent_init/mod.rs:319-331`. `tool_health` at `:185`.

**Interfaces:**
- Consumes: `ExtensionManager::{set_mcp_handle, set_memory_registry, set_tool_catalog}` (P1.7), `ToolCatalog::with_health` (`registry/mod.rs:97-106`).
- Produces: `init_tool_catalog(…, tool_catalog: Arc<ToolCatalog>)` (parameter replaces `tool_health: Arc<ToolHealthCache>`).

- [ ] **Step 1: Write the failing test**

Add a `#[cfg(test)] mod boot_order_tests` at the end of `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`:

```rust
#[cfg(test)]
mod boot_order_tests {
    /// The three runtime handles a plugin mount needs must be injected
    /// before the first `ensure_loaded()` in this file. Textual, because the
    /// order is a property of this one function body and a runtime test
    /// would have to boot the server to observe it. Comment lines are
    /// stripped so prose naming these calls does not count.
    #[test]
    fn handles_are_installed_before_the_first_extension_load() {
        let src = include_str!("mod.rs");
        let code: Vec<&str> = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        let first = |needle: &str| {
            code.iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("`{needle}` not found in agent_init/mod.rs"))
        };
        let load = first("ensure_loaded()");
        for needle in ["set_mcp_handle(", "set_memory_registry(", "set_tool_catalog("] {
            let at = first(needle);
            assert!(
                at < load,
                "`{needle}` (line {}) must come before the first `ensure_loaded()` (line {}) — \
                 otherwise the first mount runs without that handle and records a skip",
                at + 1,
                load + 1
            );
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --bins handles_are_installed_before_the_first_extension_load -- --nocapture`
Expected: FAIL — "`set_mcp_handle(` not found in agent_init/mod.rs" (none of the three calls is in this file today).

- [ ] **Step 3: Write minimal implementation**

`agent_init/mod.rs` — at `:185` the line is:

```rust
    let tool_health = Arc::new(alephcore::tool_metadata::ToolHealthCache::new());
```

Directly after it add:

```rust
    // The unified dispatch registry, built HERE rather than inside
    // `init_tool_catalog` so it exists before the first `ensure_loaded()`
    // below: a plugin's `commands/*.md` become slash entries at its own
    // mount (`extension/slash_effect.rs`), and a mount that runs before the
    // catalog exists records that step as skipped. `init_tool_catalog` fills
    // it in; nothing else about it changed.
    let tool_catalog = Arc::new(alephcore::tool_metadata::ToolCatalog::with_health(
        tool_health.clone(),
    ));
```

After the `memory_ext_registry` block ends (`:331`, the line `    };` after `std::sync::Arc::new(reg)`) add:

```rust
    // Every handle a plugin mount needs, installed before the first
    // `ensure_loaded()` (below) so the first `load_all` mounts each plugin
    // completely — MCP servers, memory extension, slash entries — instead of
    // leaving those to a boot-time catch-up task. Order among the three does
    // not matter; order against the load does (see `boot_order_tests`).
    {
        use alephcore::gateway::handlers::plugins::get_extension_manager;
        if let Ok(ext_manager) = get_extension_manager() {
            if let Some(h) = hub_mcp_handle.as_ref() {
                ext_manager.set_mcp_handle(h.clone());
            }
            ext_manager.set_memory_registry(memory_ext_registry.clone());
            ext_manager.set_tool_catalog(tool_catalog.clone());
        }
    }
```

At the call site (`:1959-1970`) replace `tool_health.clone(),` (the last argument) with `tool_catalog.clone(),`.

`tool_catalog_init.rs` — signature (`:25-35`) currently ends:

```rust
    daemon: bool,
    tool_health: Arc<alephcore::tool_metadata::ToolHealthCache>,
) -> Arc<alephcore::tool_metadata::ToolCatalog> {
```

becomes:

```rust
    daemon: bool,
    tool_catalog: Arc<alephcore::tool_metadata::ToolCatalog>,
) -> Arc<alephcore::tool_metadata::ToolCatalog> {
```

Delete the construction (`:39-45`):

```rust
    // Shares the cache the `ExecutionEngine` already holds: the engine is
    // wrapped in an `Arc` long before this runs, so it cannot be handed the
    // catalog's own cache afterwards. Creating one cache up front and giving
    // the same handle to both is what makes the probes registered below
    // reachable from the per-request tool service.
    let tool_catalog = Arc::new(ToolCatalog::with_health(tool_health));
```

and the now-unused `use alephcore::tool_metadata::ToolCatalog;` at `:37` if nothing else in the file names it (check with `rg -n 'ToolCatalog' src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs`). Delete the late memory-registry block (`:506-520`, quoted in full):

```rust
    // ── Spec 4 Task 11: wire memory_registry into ExtensionManager ───────
    // After the registry is constructed we inject it into the global
    // ExtensionManager so any plugin loaded at runtime via
    // `load_runtime_plugin` / `ensure_plugin_loaded` also gets
    // `load_plugin_with_memory` (MCP memory extension auto-registration).
    {
        use alephcore::gateway::handlers::plugins::get_extension_manager;
        if let Ok(ext_manager) = get_extension_manager() {
            // ExtensionManager is stored behind Arc; we can only thread the
            // registry through the methods that take `&self`.
            // Inject via set_memory_registry if available (no-op if the
            // Arc is already shared across threads).
            ext_manager.set_memory_registry(memory_ext_registry.clone());
        }
    }
```

The `memory_ext_registry` parameter stays (it is still used by the scheduler wiring earlier in the file — verify with `rg -n memory_ext_registry` in that file; if the deleted block was its last use, remove the parameter and the argument at the call site too, and update the doc comment at `:9-14` which names "threads the memory extension registry into the global `ExtensionManager`").

`start/mod.rs` — inside the spawned task (`:1443-1447`), delete the line `em.set_mcp_handle(handle);` and the now-unused `let handle = h.clone();` two lines above it; the rest of the task (`sync_mcp_plugin_servers`, `bind_memory_callers`, `sync_plugin_services`) stays until P1.11. Update the comment at `:1432-1440` to say the handle is installed in `agent_init` and this task is the transitional catch-up.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --bins handles_are_installed_before_the_first_extension_load -- --nocapture && cargo check -p alephcore --bins`
Expected: PASS; check clean. Then boot once: `cargo run --bin aleph-server -- --port 18899 start` for ~20 s and confirm the log has `Extension loading complete` and no `panicked`.

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init/mod.rs src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "aleph-server/boot: install MCP handle, memory registry and tool catalog before the first extension load

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.9: `src/extension/lifecycle.rs` — `mount` / `unmount` / `reload_plugin` / `reload` / `load_all` / `after_transition`; `set_plugin_enabled` → primitives

**Files:**
- Create: `src/extension/lifecycle.rs`
- Modify: `src/extension/mod.rs:26-52` (module list + re-exports), `:147-245` (add the `scopes` field), `:334-357` (constructor), `:522-770` (delete `load_all`), `:772-798` (`ensure_loaded` → `load_all_locked`), `:800-837` (delete `reload`), `:952-958` (watcher closure log), `:1284-1341` (delete the old `reload_plugin`)
- Modify: `src/extension/plugin_ops.rs:575-641` (`set_plugin_enabled` body)
- Modify: `src/gateway/handlers/plugins/handlers/runtime.rs:261-271`, `src/builtin_tools/plugin_manage.rs:233-237`, `src/gateway/handlers/hooks_admin.rs:428-440` (return-type follow-ups so the crate compiles)
- Test: `src/extension/lifecycle.rs` (`mod tests`); the three existing activation tests in `src/extension/mod.rs:1556-1684` must keep passing unchanged

**Interfaces:**
- Consumes: the five producers (P1.2–P1.7), `collect_plugin_dirs` (`mod.rs:1016-1073`), `sync_hooks_from_registry` (`:1128`), `sync_user_hooks` (`:1075`), `republish_plugin_projections` (`projection.rs:118`), `publish_plugin_settings` (`plugin_ops.rs:451-471`), `plugin_settings_for_runtime` (`plugin_ops.rs:359-370`), `mcp_config::read_mcp_json` (`mcp_config.rs:127-131`), `manifest::parse_manifest_from_dir_cached_global`, `AdapterRegistry::parse_dir`.
- Produces:
  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum MountError {
      NotFound(String), AlreadyMounted(String), Blocked { id: String, origin: String },
      Disabled(String), Parse { id: String, reason: String }, Step { id: String, step: &'static str, reason: String },
  }
  #[derive(Debug, thiserror::Error)]
  pub enum UnmountError { NotFound(String), NotMounted(String) }
  #[derive(Debug)] pub struct ReloadReport { pub mounted: Vec<String>, pub failed: Vec<(String, MountError)>, pub summary: LoadSummary }
  impl ExtensionManager {
      pub async fn mount(&self, id: &str) -> Result<PluginStatus, MountError>;
      pub async fn unmount(&self, id: &str) -> Result<DisposeReport, UnmountError>;
      pub async fn reload_plugin(&self, id: &str) -> Result<PluginStatus, MountError>;
      pub async fn reload(&self) -> ExtensionResult<ReloadReport>;
      pub async fn load_all(&self) -> ExtensionResult<LoadSummary>;
      async fn after_transition(&self);   // the ONLY caller of republish_plugin_projections / sync_hooks_from_registry
      /// R1.1: one spawned task per plugin that awaits every server-start receiver the `mcp_server`
      /// step handed back and logs each outcome. P3.3 turns this into the `Pending` writer.
      fn watch_server_starts(&self, plugin_id: &str, receivers: Vec<(String, ServerStartReceiver)>);
      /// R1.1: completes when every watcher spawned so far has finished. No timer — the actor's
      /// own handshake cap bounds each receiver. P3.4 awaits it on the boot path.
      pub async fn activation_settled(&self);
      #[cfg(test)] pub(crate) fn scope_steps(&self, id: &str) -> Option<Vec<&'static str>>;
      #[cfg(test)] pub(crate) fn scope_skipped(&self, id: &str) -> Option<Vec<(&'static str, String)>>;
  }
  ```
  **Named anchor sites for later phases (R1.4)** — all in `src/extension/lifecycle.rs`:
  - `ExtensionManager::admit(&self, id: &str, origin: PluginOrigin) -> Result<(), MountError>` — the ONLY place `mount` consults the owner trust policy and `plugins.toml` (`self.plugins_config.read().await.is_enabled(id)`). P4.10 changes the `is_enabled` call to `is_enabled_for(id, origin)`.
  - `ExtensionManager::migrate_legacy_disabled_marker(&self, dir_path: &Path, plugin_id: &str)` — the legacy `.disabled` migration (called from `discover_and_mount` before `admit`). P4.10 origin-gates it.
  - `ExtensionManager::build_record(output: &AdapterOutput, root_dir: PathBuf, origin: PluginOrigin) -> PluginRecord` — the ONE `PluginRecord` construction for every parsed plugin (used by `discover_and_mount` and `mount_inner`). P2.3 stamps `record.scope_key` here.
  - `ExtensionManager::unparsed_record(dir_path: &Path, error: &str) -> PluginRecord` — the fallback `Error` row for a directory whose manifest does not parse (origin `Global`, as today). P2.3 stamps the key here too.
  - `ExtensionManager::write_failed_row(&self, shell: PluginRecord, step: &'static str, reason: &str)` — the ONE site that turns a mount-step failure into `PluginStatus::Error("<step>: <reason>")` (called only by `fail_mount`). P2.3 anchors the error-row key here; G-2: no rename of the variant.
  - `mount_parsed` keys the `mcp_server` step on `kind == PluginKind::Mcp` (the `// 3. mcp_server` block). P4.15 makes the CC adapter infer `Mcp` from `mcpServers` / `.mcp.json`; nothing changes here.

- [ ] **Step 1: Write the failing test**

Create `src/extension/lifecycle.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoveryConfig;
    use crate::extension::{ExtensionConfig, ExtensionManager};
    use std::path::{Path, PathBuf};

    /// Same shape as `mod.rs::tests::isolated_manager`, duplicated here rather
    /// than made `pub(super)` because the two test modules will diverge (this
    /// one grows the six-effect fixture in P1.13).
    async fn isolated_manager(dir: &Path) -> (ExtensionManager, PathBuf) {
        let cfg_path = dir.join("plugins.toml");
        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: dir.to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 0,
            },
            plugins_config_path: Some(cfg_path.clone()),
            extra_plugin_parents: vec![dir.join("plugins")],
        })
        .await
        .unwrap();
        (manager, cfg_path)
    }

    /// A Static plugin with one command, one sub-agent and one command hook:
    /// exercises `registry_row`, the views (hook executor is per-manager, so
    /// it is the view these tests read — the skill-dir / sub-agent
    /// projections are process globals that parallel tests clobber), and
    /// (with a catalog attached) `slash_command`.
    fn write_static_plugin(root: &Path, id: &str) {
        let dir = root.join("plugins").join(id);
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        std::fs::write(
            dir.join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo qa"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".claude-plugin/plugin.toml"),
            format!("name = \"{id}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("commands/hello.md"),
            "---\ndescription: say hello\n---\nHello $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("agents/helper.md"),
            "---\nname: helper\ndescription: helps\n---\nYou help.\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn load_all_mounts_enabled_plugins_and_records_their_scope() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager.set_tool_catalog(catalog.clone());

        let summary = manager.load_all().await.unwrap();
        assert_eq!(summary.plugins_loaded, 1);
        let row = manager.get_plugin_record("alpha").await.unwrap();
        assert!(row.status.is_active());
        assert_eq!(
            manager.scope_steps("alpha").unwrap(),
            vec!["registry_row", "slash_command"],
            "a Static plugin registers exactly these two effects"
        );
        assert!(manager.scope_skipped("alpha").unwrap().is_empty());
        // Views were derived once, after the mount.
        let names: Vec<String> = catalog.list_all().await.into_iter().map(|t| t.name).collect();
        assert!(names.contains(&"alpha:hello".to_string()), "{names:?}");
        let hooks = manager.hook_executor_snapshot().await.inventory();
        assert!(hooks.iter().any(|h| h.source == "alpha"), "hook view derived after mount: {hooks:?}");
    }

    #[tokio::test]
    async fn mount_of_an_unknown_id_is_not_found_and_twice_is_already_mounted() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        assert!(matches!(manager.mount("nope").await, Err(MountError::NotFound(_))));
        assert!(matches!(manager.mount("alpha").await, Err(MountError::AlreadyMounted(_))));
    }

    #[tokio::test]
    async fn unmount_disposes_and_leaves_a_listable_disabled_row() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager.set_tool_catalog(catalog.clone());
        manager.load_all().await.unwrap();

        let report = manager.unmount("alpha").await.unwrap();
        assert!(report.all_ok(), "{report:?}");
        assert_eq!(
            report.steps.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec!["slash_command", "registry_row"],
            "reverse order"
        );
        let row = manager.get_plugin_record("alpha").await.expect("still listable");
        assert_eq!(row.status, PluginStatus::Disabled);
        assert!(row.error.is_none());
        assert_eq!(row.command_count, 1, "counts from the manifest survive on the row");
        assert!(manager.scope_steps("alpha").is_none(), "scope consumed");
        {
            let reg = manager.get_plugin_registry().await;
            assert!(reg.list_skills().is_empty(), "capability rows are gone");
            assert!(reg.list_agents().is_empty());
        }
        let names: Vec<String> = catalog.list_all().await.into_iter().map(|t| t.name).collect();
        assert!(!names.contains(&"alpha:hello".to_string()));
        let hooks = manager.hook_executor_snapshot().await.inventory();
        assert!(!hooks.iter().any(|h| h.source == "alpha"), "hook view derived after unmount: {hooks:?}");

        assert!(matches!(manager.unmount("alpha").await, Err(UnmountError::NotMounted(_))));
        assert!(matches!(manager.unmount("nope").await, Err(UnmountError::NotFound(_))));
    }

    #[tokio::test]
    async fn mount_refuses_a_plugin_the_operator_disabled_and_a_blocked_origin() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, cfg_path) = isolated_manager(tmp.path()).await;
        crate::extension::plugin_state::PluginsConfig {
            entries: [(
                "alpha".to_string(),
                crate::extension::plugin_state::PluginEntryConfig {
                    enabled: Some(false),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }
        .save(&cfg_path)
        .await
        .unwrap();
        let (manager2, _) = isolated_manager(tmp.path()).await;
        drop(manager);
        manager2.load_all().await.unwrap();
        assert_eq!(manager2.get_plugin_record("alpha").await.unwrap().status, PluginStatus::Disabled);
        assert!(matches!(manager2.mount("alpha").await, Err(MountError::Disabled(_))), "mount does not override plugins.toml");

        // Trust gate: refuse every Project-origin plugin.
        manager2.set_owner_trust_policy(crate::extension::plugin_trust::OwnerTrustPolicy::restrictive(vec![]));
        let err = manager2.mount("alpha").await.err().unwrap();
        assert!(matches!(err, MountError::Blocked { .. }), "{err}");
        assert!(matches!(
            manager2.get_plugin_record("alpha").await.unwrap().status,
            PluginStatus::Blocked(_)
        ));
    }

    #[tokio::test]
    async fn a_failing_step_disposes_the_partial_scope_and_writes_the_error_row() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        // A WASM plugin whose entry does not exist: registry_row succeeds,
        // wasm_module fails → all-or-none.
        let dir = tmp.path().join("plugins").join("broken");
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(
            dir.join("aleph.plugin.toml"),
            "[plugin]\nid = \"broken\"\nname = \"Broken\"\nkind = \"wasm\"\nentry = \"missing.wasm\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("commands/x.md"), "---\ndescription: x\n---\nx\n").unwrap();
        let (manager, _) = isolated_manager(tmp.path()).await;
        let summary = manager.load_all().await.unwrap();
        assert_eq!(summary.plugins_loaded, 0);
        assert_eq!(summary.errors.len(), 1, "{:?}", summary.errors);
        let row = manager.get_plugin_record("broken").await.unwrap();
        assert!(matches!(&row.status, PluginStatus::Error(e) if e.starts_with("wasm_module: ")), "{:?}", row.status);
        assert!(manager.scope_steps("broken").is_none(), "no scope survives a failed mount");
        assert!(
            manager.get_plugin_registry().await.list_skills().is_empty(),
            "the registry_row effect was disposed (all-or-none)"
        );
        // And the same through the public verb.
        assert!(matches!(manager.mount("broken").await, Err(MountError::Step { step: "wasm_module", .. })));
    }

    #[tokio::test]
    async fn reload_plugin_is_unmount_then_mount_and_reload_rebuilds_everything() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();

        // Edit on disk, then reload just that plugin: the new command appears.
        std::fs::write(
            tmp.path().join("plugins/alpha/commands/bye.md"),
            "---\ndescription: bye\n---\nBye\n",
        )
        .unwrap();
        let status = manager.reload_plugin("alpha").await.unwrap();
        assert!(status.is_active());
        assert_eq!(manager.get_plugin_registry().await.list_skills().len(), 2);
        assert!(matches!(manager.reload_plugin("nope").await, Err(MountError::NotFound(_))));

        // Full reload: a plugin removed from disk is gone, a new one appears.
        std::fs::remove_dir_all(tmp.path().join("plugins/alpha")).unwrap();
        write_static_plugin(tmp.path(), "beta");
        let report = manager.reload().await.unwrap();
        assert_eq!(report.mounted, vec!["beta".to_string()]);
        assert!(report.failed.is_empty());
        assert!(manager.get_plugin_record("alpha").await.is_none());
        assert!(manager.scope_steps("alpha").is_none(), "alpha's scope was disposed");
        assert!(manager.scope_steps("beta").is_some());
        assert_eq!(manager.reload_count(), 1);
    }

    #[tokio::test]
    async fn activation_settled_returns_at_once_when_nothing_is_being_watched() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        manager.load_all().await.unwrap();
        // A Static plugin enqueues no server, so no watcher exists.
        tokio::time::timeout(std::time::Duration::from_millis(200), manager.activation_settled())
            .await
            .expect("no watchers → settles immediately");
    }

    #[tokio::test]
    async fn a_missing_handle_is_a_recorded_skip_not_a_failure() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        write_static_plugin(tmp.path(), "alpha");
        let (manager, _) = isolated_manager(tmp.path()).await;
        // No set_tool_catalog: the CLI shape.
        manager.load_all().await.unwrap();
        assert!(manager.get_plugin_record("alpha").await.unwrap().status.is_active());
        assert_eq!(manager.scope_steps("alpha").unwrap(), vec!["registry_row"]);
        let skipped = manager.scope_skipped("alpha").unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].0, "slash_command");
        let reg = manager.get_plugin_registry().await;
        assert!(
            reg.diagnostics().iter().any(|d| d.plugin_id.as_deref() == Some("alpha") && d.message.contains("slash_command")),
            "the skip is visible on the row's diagnostics: {:?}",
            reg.diagnostics()
        );
    }
}
```

(Verified: `OwnerTrustPolicy::restrictive(allowlist: impl IntoIterator<Item = String>)` sets `enforce: true` (`plugin_trust.rs:78-81`), and `extra_plugin_parents` are discovered as `DiscoverySource::Project` → `PluginOrigin::Workspace` (`plugins.rs:115`), which the policy gates. `parse_single_agent` (`parsers.rs:422-440`) reads only `name` / `description` / `model` from the frontmatter and leaves `mode` at its default `All`, for which `is_subagent()` is true (`registry/types.rs:347-352`, `types/agents.rs:23`) — so the fixture agent is a delegatable sub-agent.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::lifecycle -- --nocapture`
Expected: FAIL to compile — `MountError`, `scope_steps`, `unmount` … not found.

- [ ] **Step 3: Write minimal implementation**

`src/extension/lifecycle.rs` (above the tests):

```rust
//! The plugin lifecycle: four primitives and one place where views are
//! re-derived.
//!
//! | primitive | semantics |
//! |---|---|
//! | [`ExtensionManager::mount`] | gates (row present → not mounted → owner trust → `plugins.toml`) → parse → six effects in [`STEP_LABELS`] order, all-or-none |
//! | [`ExtensionManager::unmount`] | take the scope out, dispose in reverse, leave a `Disabled` row |
//! | [`ExtensionManager::reload_plugin`] | unmount + mount of one id |
//! | [`ExtensionManager::reload`] | unmount everything, rediscover, mount every admitted plugin |
//!
//! Every public primitive ends with exactly one [`ExtensionManager::after_transition`],
//! which is the ONLY caller of `republish_plugin_projections` and
//! `sync_hooks_from_registry` (guarded by
//! `projection::tests::publishing_plugin_projections_has_exactly_one_author`).
//!
//! Effects vs views: an effect has an inverse and lives in the plugin's
//! [`EffectScope`]; a view is recomputed from the registry here. The test for
//! which is which: "does it have an inverse?" — yes → effect; no but derivable
//! → view; neither → it should not be written by a plugin at all.
//!
//! Transitions serialise on `load_guard` (the same mutex `ensure_loaded` and
//! `reload` already shared), so two toggles of one id cannot interleave and a
//! reload cannot run under a mount. `scopes` is a std mutex that is never held
//! across an `await`: a scope is removed under the lock and disposed after.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::effects::{DisposeReport, EffectScope};
use super::error::ExtensionResult;
use super::hooks::{HookExecutor, ShellHookConsent};
use super::manifest::adapter::AdapterOutput;
use super::manifest::PluginManifest;
use super::registrar::mcp_registrar::ServerStartReceiver;
use super::registry::{DiagnosticLevel, PluginDiagnostic};
use super::types::{LoadSummary, PluginKind, PluginOrigin, PluginRecord, PluginStatus};
use super::{loader, manifest, mcp_config, registrar, slash_effect, ExtensionManager};
use crate::extension::capability::CapabilityDeclaration;
use crate::sync_primitives::Arc;

/// Why a mount did not happen. `Step` carries the label of the effect that
/// failed; the partial scope was disposed before this was returned.
#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("plugin not found: {0}")]
    NotFound(String),
    #[error("plugin '{0}' is already mounted")]
    AlreadyMounted(String),
    #[error(
        "plugin '{id}' refused by the owner trust policy ({origin} origin is not on the \
         allowlist); add \"{id}\" to the allowlist to load it"
    )]
    Blocked { id: String, origin: String },
    #[error("plugin '{0}' is disabled by the operator (plugins.toml)")]
    Disabled(String),
    #[error("plugin '{id}' manifest could not be parsed: {reason}")]
    Parse { id: String, reason: String },
    #[error("plugin '{id}' failed at step '{step}': {reason}")]
    Step {
        id: String,
        step: &'static str,
        reason: String,
    },
}

/// Why an unmount did not happen.
#[derive(Debug, thiserror::Error)]
pub enum UnmountError {
    #[error("plugin not found: {0}")]
    NotFound(String),
    #[error("plugin '{0}' is not mounted")]
    NotMounted(String),
}

/// What a full reload did.
#[derive(Debug)]
pub struct ReloadReport {
    pub mounted: Vec<String>,
    pub failed: Vec<(String, MountError)>,
    pub summary: LoadSummary,
}

/// What one discovery pass did — `load_all` reports the summary, `reload`
/// reports all three.
pub(super) struct LoadOutcome {
    pub summary: LoadSummary,
    pub mounted: Vec<String>,
    pub failed: Vec<(String, MountError)>,
}

impl ExtensionManager {
    // ── Public primitives ─────────────────────────────────────────────────

    /// Mount one discovered plugin: admission gates, manifest parse, then
    /// the six effects. All-or-none: a failing step disposes what was
    /// registered and writes `PluginStatus::Error("<step>: <reason>")`.
    pub async fn mount(&self, id: &str) -> Result<PluginStatus, MountError> {
        let _guard = self.load_guard.lock().await;
        let status = self.mount_inner(id).await?;
        self.after_transition().await;
        Ok(status)
    }

    /// Take the plugin's scope out, dispose it in reverse order, and leave a
    /// `Disabled` row so the plugin stays listable and re-enablable.
    pub async fn unmount(&self, id: &str) -> Result<DisposeReport, UnmountError> {
        let _guard = self.load_guard.lock().await;
        let report = self.unmount_inner(id).await?;
        self.after_transition().await;
        Ok(report)
    }

    /// unmount + mount. Replaces the narrower `reload_plugin` that refreshed
    /// only the tool index and skipped hooks / projections / MCP / services.
    pub async fn reload_plugin(&self, id: &str) -> Result<PluginStatus, MountError> {
        let _guard = self.load_guard.lock().await;
        match self.unmount_inner(id).await {
            Ok(_) | Err(UnmountError::NotMounted(_)) => {}
            Err(UnmountError::NotFound(_)) => return Err(MountError::NotFound(id.to_string())),
        }
        let result = self.mount_inner(id).await;
        self.after_transition().await;
        result
    }

    /// Dispose every mounted plugin, rediscover, mount every admitted
    /// plugin. `stop_orphaned_services` and `sync_mcp_plugin_servers` are
    /// gone because dispose + mount is what they approximated.
    pub async fn reload(&self) -> ExtensionResult<ReloadReport> {
        self.reload_count
            .fetch_add(1, crate::sync_primitives::Ordering::SeqCst);
        let _guard = self.load_guard.lock().await;
        self.cache_state.write().await.loaded = false;
        let out = self.load_all_locked().await?;
        Ok(ReloadReport {
            mounted: out.mounted,
            failed: out.failed,
            summary: out.summary,
        })
    }

    /// Discover and mount everything. Public for the CLI (`aleph-server
    /// plugins list`) and tests; boot goes through `ensure_loaded`.
    pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
        let _guard = self.load_guard.lock().await;
        Ok(self.load_all_locked().await?.summary)
    }

    /// `load_all` for a caller that already holds `load_guard`.
    pub(super) async fn load_all_locked(&self) -> ExtensionResult<LoadOutcome> {
        let out = self.discover_and_mount().await?;
        self.after_transition().await;
        self.cache_state.write().await.loaded = true;
        tracing::info!(
            "Extension loading complete: {} skills, {} agents, {} plugins, {} hooks",
            out.summary.skills_loaded,
            out.summary.agents_loaded,
            out.summary.plugins_loaded,
            out.summary.hooks_loaded,
        );
        Ok(out)
    }

    // ── The one view recomputation ────────────────────────────────────────

    /// Re-derive every view after a transition: hook executor (rebuilt from
    /// the registry, then user hooks re-layered) and the process-global
    /// projections (skill dirs, sub-agents, tool index). Called exactly once
    /// per public primitive.
    async fn after_transition(&self) {
        *self.hook_executor.write().await =
            HookExecutor::empty().with_consent(ShellHookConsent::shared());
        self.sync_hooks_from_registry().await;
        self.sync_user_hooks().await;
        let projection = self.republish_plugin_projections().await;
        tracing::debug!(
            plugin_skill_dirs = projection.plugin_skill_dirs.len(),
            plugin_subagents = projection.subagents.len(),
            "published plugin projections"
        );
    }

    // ── Discovery ─────────────────────────────────────────────────────────

    /// One discovery pass: dispose whatever is mounted, rebuild the rows from
    /// disk, mount every plugin the gates admit. No view recomputation here —
    /// the caller does that once.
    async fn discover_and_mount(&self) -> ExtensionResult<LoadOutcome> {
        let mut summary = LoadSummary::default();
        let mut mounted: Vec<String> = Vec::new();
        let mut failed: Vec<(String, MountError)> = Vec::new();

        self.unmount_all().await;

        // The loader's copy of every plugin's stored configuration must be
        // current before anything loads — a plugin started with an empty
        // config is a plugin the operator configured and cannot tell.
        self.publish_plugin_settings().await;

        let plugin_dirs = self.collect_plugin_dirs()?;
        self.plugin_registry.write().await.clear();

        // Shadow resolution: dirs are walked highest-priority first, so a
        // repeat id lost. `winners` only tracks SUCCESSFUL parses — a parse
        // failure must not poison it (a lower-priority copy with the same id
        // may still take over), so failures are deduped separately.
        let mut winners: HashMap<String, PathBuf> = HashMap::new();
        let mut failed_plugin_ids: HashSet<String> = HashSet::new();

        for found in plugin_dirs.iter().rev() {
            let dir_path = &found.path;
            let output = match self.adapter_registry.parse_dir(dir_path) {
                Ok(output) => output,
                Err(e) => {
                    let fallback_id = dir_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir_path.display().to_string());
                    tracing::warn!(
                        plugin_dir = %dir_path.display(), error = %e,
                        "plugin manifest could not be parsed; listing it as errored"
                    );
                    summary.errors.push(format!("{}: {}", dir_path.display(), e));
                    if failed_plugin_ids.insert(fallback_id) {
                        self.plugin_registry
                            .write()
                            .await
                            .register_plugin(Self::unparsed_record(dir_path, &e.to_string()));
                    }
                    continue;
                }
            };
            let plugin_id = output.plugin_id.clone();
            if let Some(winner) = winners.get(&plugin_id) {
                tracing::info!(
                    plugin_id = %plugin_id, shadowed = %dir_path.display(), winner = %winner.display(),
                    "plugin id already registered from a higher-priority scope"
                );
                self.plugin_registry.write().await.add_diagnostic(PluginDiagnostic {
                    level: DiagnosticLevel::Warn,
                    message: format!("{} is shadowed by the copy at {}", dir_path.display(), winner.display()),
                    plugin_id: Some(plugin_id.clone()),
                    source: Some("discovery".to_string()),
                });
                summary.shadowed += 1;
                continue;
            }
            winners.insert(plugin_id.clone(), dir_path.clone());

            self.migrate_legacy_disabled_marker(dir_path, &plugin_id).await;

            let record = Self::build_record(&output, dir_path.clone(), found.origin);
            match self.admit(&plugin_id, found.origin).await {
                Err(e @ MountError::Blocked { .. }) => {
                    tracing::info!(plugin_id = %plugin_id, origin = ?found.origin, "plugin skipped by owner trust policy (not in allowlist)");
                    summary.skipped_by_trust += 1;
                    // Register it as Blocked rather than dropping it: "refused by
                    // policy" and "not installed" must not render the same.
                    let detail = e.to_string();
                    self.plugin_registry.write().await.register_plugin(
                        record.inactive(PluginStatus::Blocked(format!("{:?}", found.origin)), detail),
                    );
                }
                Err(MountError::Disabled(_)) => {
                    // Registered but not mounted: listable, re-enablable via
                    // `mount`, invisible to the model (no capability rows).
                    let mut record = record;
                    record.status = PluginStatus::Disabled;
                    self.plugin_registry.write().await.register_plugin(record);
                    summary.disabled_by_operator += 1;
                }
                Err(other) => {
                    // `admit` only produces Blocked / Disabled; anything else is
                    // a bug in this file, not a plugin outcome.
                    unreachable!("admit returned {other:?}");
                }
                Ok(()) => match self.mount_parsed(output, record).await {
                    Ok(_) => mounted.push(plugin_id),
                    Err(e) => {
                        summary.errors.push(format!("{plugin_id}: {e}"));
                        failed.push((plugin_id, e));
                    }
                },
            }
        }

        {
            let registry = self.plugin_registry.read().await;
            summary.skills_loaded = registry.list_skills().len();
            summary.agents_loaded = registry.list_agents().len();
            summary.hooks_loaded = registry.list_hooks().len();
        }
        summary.plugins_loaded = mounted.len();
        Ok(LoadOutcome {
            summary,
            mounted,
            failed,
        })
    }

    /// `.disabled` markers written by older builds are migrated into
    /// `plugins.toml` on first sight and then removed, so the answer
    /// converges to one source instead of two.
    async fn migrate_legacy_disabled_marker(&self, dir_path: &std::path::Path, plugin_id: &str) {
        let legacy_marker = dir_path.join(".disabled");
        if !legacy_marker.exists() {
            return;
        }
        {
            let mut cfg = self.plugins_config.write().await;
            if cfg.set_enabled(plugin_id, false) {
                if let Err(e) = cfg.save(&self.plugins_config_path).await {
                    tracing::warn!(error = %e, "failed to persist migrated plugin disable");
                }
            }
        }
        match tokio::fs::remove_file(&legacy_marker).await {
            Ok(()) => tracing::info!(plugin_id, "migrated legacy .disabled marker into plugins.toml"),
            Err(e) => tracing::warn!(plugin_id, error = %e, "legacy .disabled marker migrated but could not be removed"),
        }
    }

    /// The record for a parsed plugin. Adapters hardcode `Global` and `Static`;
    /// where it was found and what runtime it needs are facts of discovery
    /// and of the manifest, applied here in one place.
    fn build_record(output: &AdapterOutput, root_dir: PathBuf, origin: PluginOrigin) -> PluginRecord {
        let mut record = PluginRecord::from_adapter_output(output, root_dir.clone());
        record.origin = origin;
        if let Ok(m) = manifest::parse_manifest_from_dir_cached_global(&root_dir) {
            record.kind = m.kind;
        }
        record
    }

    /// The `Error` row for a directory whose manifest does not parse. It used
    /// to vanish at `debug!` level — on every surface identical to "never
    /// installed" — so it gets a row, an id derived from the directory, and
    /// the parse error. Origin is `Global` (as it always was for this row).
    fn unparsed_record(dir_path: &std::path::Path, error: &str) -> PluginRecord {
        let leaf = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir_path.display().to_string());
        PluginRecord::new(leaf.clone(), leaf, PluginKind::Static, PluginOrigin::Global)
            .with_root_dir(dir_path.to_path_buf())
            .with_error(error.to_string())
    }

    /// The two admission gates, pure: owner trust, then the operator's
    /// durable preference (`plugins.toml`, `is_enabled`). Callers write the
    /// refusal onto the row. P4.10 origin-gates the second check.
    async fn admit(&self, id: &str, origin: PluginOrigin) -> Result<(), MountError> {
        let trust_allows = self
            .owner_trust_policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .allows(id, origin);
        if !trust_allows {
            return Err(MountError::Blocked {
                id: id.to_string(),
                origin: format!("{origin:?}"),
            });
        }
        if !self.plugins_config.read().await.is_enabled(id) {
            return Err(MountError::Disabled(id.to_string()));
        }
        Ok(())
    }

    // ── Mount ─────────────────────────────────────────────────────────────

    async fn mount_inner(&self, id: &str) -> Result<PluginStatus, MountError> {
        if self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
        {
            return Err(MountError::AlreadyMounted(id.to_string()));
        }
        let (root_dir, origin) = self
            .plugin_registry
            .read()
            .await
            .get_plugin(id)
            .map(|r| (r.root_dir.clone(), r.origin))
            .ok_or_else(|| MountError::NotFound(id.to_string()))?;

        if let Err(e) = self.admit(id, origin).await {
            let mut reg = self.plugin_registry.write().await;
            if let Some(row) = reg.get_plugin_mut(id) {
                match &e {
                    MountError::Blocked { .. } => {
                        row.status = PluginStatus::Blocked(format!("{origin:?}"));
                        row.error = Some(e.to_string());
                    }
                    MountError::Disabled(_) => {
                        row.status = PluginStatus::Disabled;
                        row.error = None;
                    }
                    _ => {}
                }
            }
            return Err(e);
        }

        let output = match self.adapter_registry.parse_dir(&root_dir) {
            Ok(o) => o,
            Err(e) => {
                let reason = e.to_string();
                let mut reg = self.plugin_registry.write().await;
                if let Some(row) = reg.get_plugin_mut(id) {
                    row.status = PluginStatus::Error(reason.clone());
                    row.error = Some(reason.clone());
                }
                return Err(MountError::Parse {
                    id: id.to_string(),
                    reason,
                });
            }
        };
        let record = Self::build_record(&output, root_dir, origin);
        self.mount_parsed(output, record).await
    }

    /// The six effects, in [`super::effects::STEP_LABELS`] order. `record`
    /// already carries root_dir / origin / kind.
    async fn mount_parsed(
        &self,
        output: AdapterOutput,
        record: PluginRecord,
    ) -> Result<PluginStatus, MountError> {
        let id = record.id.clone();
        let root_dir = record.root_dir.clone();
        let manifest: Option<PluginManifest> =
            manifest::parse_manifest_from_dir_cached_global(&root_dir).ok();
        let kind = manifest.as_ref().map_or(PluginKind::Static, |m| m.kind);

        // Read what later steps need out of the capabilities BEFORE they move
        // into the registry.
        let command_skills: Vec<super::registry::SkillRegistration> = output
            .capabilities
            .iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::Skill(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let command_infos = slash_effect::plugin_command_skill_infos(&command_skills);
        let has_services = output
            .capabilities
            .iter()
            .any(|c| matches!(c, CapabilityDeclaration::Service(_)));

        let shell = record.clone();
        let mut scope = EffectScope::new(id.clone());

        // 1. registry_row — cannot fail (refused capabilities become diagnostics).
        scope.effect(
            "registry_row",
            registrar::register_plugin_row(
                Arc::clone(&self.plugin_registry),
                record,
                output.permissions,
                output.capabilities,
            )
            .await,
        );

        // 2. wasm_module
        if kind == PluginKind::Wasm {
            if let Some(m) = &manifest {
                match loader::load_wasm_effect(Arc::clone(&self.plugin_loader), m).await {
                    Ok(d) => scope.effect("wasm_module", d),
                    Err(e) => {
                        return Err(self.fail_mount(scope, shell, "wasm_module", e.to_string()).await)
                    }
                }
            }
        }

        // 3. mcp_server — keyed on the manifest's kind. A CC `plugin.json`
        // that declares `mcpServers` without `"aleph": {"runtime": "mcp"}`
        // parses as `Static` today (`cc_plugin_json.rs:217`); P4.15 fixes
        // that in the adapter, not here.
        let mut first_server: Option<String> = None;
        if kind == PluginKind::Mcp {
            let settings = self.plugin_settings_for_runtime(&id).await;
            let configs = match mcp_config::read_mcp_json(&root_dir, &id, &settings) {
                Ok(c) => c,
                Err(e) => return Err(self.fail_mount(scope, shell, "mcp_server", e.to_string()).await),
            };
            first_server = configs.keys().min().cloned();
            if configs.is_empty() {
                tracing::warn!(plugin_id = %id, "MCP plugin has no servers defined in .mcp.json");
            } else {
                let handle = self.mcp_handle.read().unwrap_or_else(|e| e.into_inner()).clone();
                match handle {
                    None => scope.skip("mcp_server", "MCP manager not attached"),
                    Some(h) => match registrar::mcp_registrar::register_transient_servers(h, configs).await {
                        Ok((d, receivers)) => {
                            scope.effect("mcp_server", d);
                            self.watch_server_starts(&id, receivers);
                        }
                        Err(e) => return Err(self.fail_mount(scope, shell, "mcp_server", e).await),
                    },
                }
            }
        }

        // 4. service — never fails the mount; per-service outcomes are on the rows.
        if has_services {
            scope.effect("service", self.start_services_effect(&id).await);
        }

        // 5. memory_extension
        if let Some(m) = manifest.as_ref().filter(|m| m.memory_manifest.is_some()) {
            let registry = self
                .memory_registry
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match registry {
                None => scope.skip("memory_extension", "memory registry not attached"),
                Some(r) => {
                    let handle = self.mcp_handle.read().unwrap_or_else(|e| e.into_inner()).clone();
                    match loader::register_memory_extension_effect(m, first_server.clone(), &r, handle) {
                        Ok(Some(d)) => scope.effect("memory_extension", d),
                        Ok(None) => {}
                        Err(e) => return Err(self.fail_mount(scope, shell, "memory_extension", e).await),
                    }
                }
            }
        }

        // 6. slash_command
        if !command_infos.is_empty() {
            let catalog = self.tool_catalog.read().unwrap_or_else(|e| e.into_inner()).clone();
            match catalog {
                None => scope.skip("slash_command", "tool catalog not attached"),
                Some(c) => scope.effect(
                    "slash_command",
                    slash_effect::register_slash_commands_effect(c, command_infos).await,
                ),
            }
        }

        if !scope.skipped().is_empty() {
            let mut reg = self.plugin_registry.write().await;
            for (step, why) in scope.skipped() {
                tracing::warn!(plugin_id = %id, step, why, "effect skipped: handle not attached");
                reg.add_diagnostic(PluginDiagnostic {
                    level: DiagnosticLevel::Warn,
                    message: format!("{step} skipped: {why}"),
                    plugin_id: Some(id.clone()),
                    source: Some("mount".to_string()),
                });
            }
        }

        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), scope);
        tracing::info!(plugin_id = %id, kind = ?kind, "plugin mounted");
        Ok(PluginStatus::Loaded)
    }

    /// All-or-none: dispose what `mount_parsed` registered so far, then
    /// write the failure onto the row through [`Self::write_failed_row`].
    async fn fail_mount(
        &self,
        scope: EffectScope,
        shell: PluginRecord,
        step: &'static str,
        reason: String,
    ) -> MountError {
        let id = shell.id.clone();
        let report = scope.dispose().await;
        if !report.all_ok() {
            tracing::warn!(plugin_id = %id, ?report, "partial scope did not dispose cleanly after a failed mount");
        }
        self.write_failed_row(shell, step, &reason).await;
        MountError::Step { id, step, reason }
    }

    /// The ONE site that turns a mount-step failure into a status:
    /// `PluginStatus::Error("<step>: <reason>")` on a row rebuilt from the
    /// pre-mount record (the `registry_row` disposer removed the live one).
    /// P2.3 stamps the row's scope key here; the variant name is today's
    /// (G-2 — no rename).
    async fn write_failed_row(&self, shell: PluginRecord, step: &'static str, reason: &str) {
        tracing::warn!(plugin_id = %shell.id, step, error = %reason, "mount failed; partial effects disposed");
        self.plugin_registry
            .write()
            .await
            .register_plugin(shell.with_error(format!("{step}: {reason}")));
    }

    // ── Server-start watchers (R1.1) ──────────────────────────────────────

    /// One task per plugin that awaits every receiver the `mcp_server` step
    /// handed back and logs each outcome. The handle is kept so
    /// [`Self::activation_settled`] can wait for it. P3.3 turns this into
    /// the readiness writer (`Pending { waiting_on }` before, terminal after).
    fn watch_server_starts(&self, plugin_id: &str, receivers: Vec<(String, ServerStartReceiver)>) {
        if receivers.is_empty() {
            return;
        }
        let plugin_id = plugin_id.to_string();
        let handle = tokio::spawn(async move {
            for (server_id, rx) in receivers {
                match rx.await {
                    Ok(Ok(())) => {
                        tracing::info!(plugin_id = %plugin_id, server_id = %server_id, "plugin MCP server registered (transient)");
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, error = %e, "plugin MCP server failed to start");
                    }
                    Err(_) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, "MCP manager dropped the start request");
                    }
                }
            }
        });
        let mut watchers = self
            .activation_watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Finished tasks are dropped opportunistically so a long-lived daemon
        // that nobody ever `activation_settled()`s does not grow the vector.
        watchers.retain(|h| !h.is_finished());
        watchers.push(handle);
    }

    /// Completes when every server-start watcher spawned so far has finished.
    /// No timer: each receiver is bounded by the actor's own handshake cap
    /// (`external/connection.rs:348`, 60 s per step). A watcher spawned while
    /// this is waiting is awaited too (the loop re-checks).
    pub async fn activation_settled(&self) {
        loop {
            let pending: Vec<tokio::task::JoinHandle<()>> = {
                let mut watchers = self
                    .activation_watchers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *watchers)
            };
            if pending.is_empty() {
                return;
            }
            for h in pending {
                if let Err(e) = h.await {
                    tracing::warn!(error = %e, "server-start watcher task failed");
                }
            }
        }
    }

    // ── Unmount ───────────────────────────────────────────────────────────

    async fn unmount_inner(&self, id: &str) -> Result<DisposeReport, UnmountError> {
        let scope = self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        let Some(scope) = scope else {
            return Err(if self.plugin_registry.read().await.get_plugin(id).is_some() {
                UnmountError::NotMounted(id.to_string())
            } else {
                UnmountError::NotFound(id.to_string())
            });
        };
        let shell = self.plugin_registry.read().await.get_plugin(id).cloned();
        let report = scope.dispose().await;
        if let Some(mut row) = shell {
            // The registry_row disposer removed the row and every capability
            // row; put back a Disabled record so the plugin stays listable and
            // re-enablable. Counts from the manifest stay on it.
            row.status = PluginStatus::Disabled;
            row.error = None;
            self.plugin_registry.write().await.register_plugin(row);
        }
        tracing::info!(plugin_id = %id, ok = report.all_ok(), "plugin unmounted");
        Ok(report)
    }

    /// Dispose every scope without writing rows — discovery rebuilds them.
    async fn unmount_all(&self) {
        let scopes: Vec<EffectScope> = self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, s)| s)
            .collect();
        for scope in scopes {
            let report = scope.dispose().await;
            if !report.all_ok() {
                tracing::warn!(?report, "scope did not dispose cleanly during reload");
            }
        }
    }

    // ── Test-only introspection ───────────────────────────────────────────

    #[cfg(test)]
    pub(crate) fn scope_steps(&self, id: &str) -> Option<Vec<&'static str>> {
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(EffectScope::steps)
    }

    #[cfg(test)]
    pub(crate) fn scope_skipped(&self, id: &str) -> Option<Vec<(&'static str, String)>> {
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(|s| s.skipped().to_vec())
    }
}
```

`src/extension/mod.rs`:

1. Module list: add `mod lifecycle;` after `mod loader;`. Re-exports: after the `effects` line from P1.1 add `pub use lifecycle::{MountError, ReloadReport, UnmountError};`.
2. Field — after `reload_count` (`:236-241`):

```rust
    /// One [`EffectScope`] per mounted plugin — everything `mount` put into
    /// the runtime, owned here so `unmount` can take it all back out. A std
    /// mutex, never held across an `await`: `lifecycle.rs` removes the scope
    /// under the lock and disposes it after.
    scopes: StdMutex<HashMap<String, effects::EffectScope>>,

    /// Join handles of the per-plugin server-start watchers
    /// (`lifecycle.rs::watch_server_starts`), so `activation_settled()` can
    /// wait for the MCP half of a mount to reach its verdict. Std mutex,
    /// never held across an `await`.
    activation_watchers: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
```

   Constructor (`:334-357`): add `scopes: StdMutex::new(HashMap::new()),` and `activation_watchers: StdMutex::new(Vec::new()),` after `reload_count: AtomicU64::new(0),`.
3. Delete `load_all` (`:522-770`: from `    // ── Lifecycle ───…` and `    /// Load all extensions.` through the closing `}` of `load_all`). Keep the `// ── Lifecycle` section marker for `ensure_loaded`.
4. `ensure_loaded` (`:782-798`): replace the last two statements

```rust
        // load_all() sets cache_state.loaded = true on success
        self.load_all().await?;
        Ok(())
```

with

```rust
        // `load_guard` is held above; `load_all` would take it again.
        self.load_all_locked().await?;
        Ok(())
```

5. Delete `reload` (`:800-837`, from `    /// Force reload all extensions` through its closing `}`).
6. Watcher closure (`:952-958`):

```rust
                        if let Err(e) = mgr.reload().await {
                            tracing::warn!(error = %e, "Extension hot reload failed");
                        }
```

becomes

```rust
                        match mgr.reload().await {
                            Ok(report) if !report.failed.is_empty() => tracing::warn!(
                                failed = ?report.failed.iter().map(|(id, e)| format!("{id}: {e}")).collect::<Vec<_>>(),
                                "Extension hot reload: some plugins did not mount"
                            ),
                            Ok(_) => {}
                            Err(e) => tracing::warn!(error = %e, "Extension hot reload failed"),
                        }
```

7. Delete the old `reload_plugin` (`:1284-1341`, from `    /// Hot-reload a single plugin by ID.` through its closing `}`). It was the only caller of `CapabilityApi::reload` (deleted in P1.11).

`src/extension/plugin_ops.rs` — `set_plugin_enabled` (`:575-641`). The doc comment (`:575-585`) stays; the body from `        let changed = {` (`:600`) through `        changed || preference_changed` (`:640`) becomes:

```rust
        let changed = if enabled {
            match self.mount(plugin_id).await {
                Ok(_) => true,
                Err(super::MountError::AlreadyMounted(_)) => false,
                Err(e) => {
                    // The row already says why (Blocked / Error / Disabled);
                    // the preference is recorded regardless.
                    tracing::warn!(plugin_id = %plugin_id, error = %e, "enable: mount failed");
                    false
                }
            }
        } else {
            match self.unmount(plugin_id).await {
                Ok(report) => {
                    if !report.all_ok() {
                        tracing::warn!(plugin_id = %plugin_id, ?report, "disable: some effects did not dispose cleanly");
                    }
                    true
                }
                // A row that was never mounted (Error / Blocked) still takes
                // the operator's verdict on its status.
                Err(super::UnmountError::NotMounted(_)) => {
                    self.plugin_registry.write().await.disable_plugin(plugin_id)
                }
                Err(super::UnmountError::NotFound(_)) => false,
            }
        };

        changed || preference_changed
```

Quote of what this replaces (`:600-640`):

```rust
        let changed = {
            let mut registry = self.plugin_registry.write().await;
            if enabled {
                registry.enable_plugin(plugin_id)
            } else {
                registry.disable_plugin(plugin_id)
            }
        };

        if changed {
            if !enabled {
                if let Err(e) = self.unload_runtime_plugin(plugin_id).await {
                    // …
                }
            }
            *self.hook_executor.write().await = crate::extension::hooks::HookExecutor::empty()
                .with_consent(crate::extension::hooks::ShellHookConsent::shared());
            self.sync_hooks_from_registry().await;
            self.sync_user_hooks().await;
            // …
            self.republish_plugin_projections().await;
            // …
            self.publish_plugin_settings().await;
        }
```

`PluginRegistry::enable_plugin` (`plugin_registry/mod.rs:125-133`) loses its last caller here; delete it in P1.11.

Return-type follow-ups (each a one-line edit so the crate compiles):

- `src/gateway/handlers/plugins/handlers/runtime.rs:261-262`: `match manager.reload_plugin(&params.plugin_id).await { Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true, "pluginId": params.plugin_id })),` → `Ok(status) => JsonRpcResponse::success(request.id, json!({ "ok": true, "pluginId": params.plugin_id, "status": status.label() })),`
- `src/builtin_tools/plugin_manage.rs:233-237`: `manager.reload_plugin(name).await.map_err(|e| AlephError::config(format!("Reload failed: {e}")))?;` → `let status = manager.reload_plugin(name).await.map_err(|e| AlephError::config(format!("Reload failed: {e}")))?;` and `data: serde_json::json!({ "name": name })` → `data: serde_json::json!({ "name": name, "status": status.label() })`.
- `src/gateway/handlers/hooks_admin.rs:428-432`: `Ok(summary) => { let count = summary.plugins_loaded;` → `Ok(report) => { let count = report.mounted.len();`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension:: -- --nocapture`
Expected: PASS — including the pre-existing `a_disabled_plugin_stays_inactive_across_a_fresh_load`, `re_enabling_needs_no_reload`, `a_legacy_disabled_marker_is_migrated_then_removed` (`mod.rs:1556-1684`) with no edits to them, and `projection::tests::publishing_plugin_projections_has_exactly_one_author`. Then `cargo test -p alephcore --lib --no-run && cargo test -p alephcore --bins && cargo test -p alephcore --features test-helpers --test '*' --no-run`.

- [ ] **Step 5: Commit**

```bash
git add src/extension/lifecycle.rs src/extension/mod.rs src/extension/plugin_ops.rs src/gateway/handlers/plugins/handlers/runtime.rs src/builtin_tools/plugin_manage.rs src/gateway/handlers/hooks_admin.rs
git commit -m "extension/lifecycle: mount/unmount/reload_plugin/reload primitives over EffectScope; one after_transition

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.10: route every remaining caller to the primitives; retire `plugins.load` / `plugins.unload`; boot catch-up task and boot-only slash registration go

**Files:**
- Modify: `src/gateway/handlers/plugins/handlers/manage.rs:209-224` (uninstall), `:258-266` (enable), `:319-333` (disable)
- Modify: `src/gateway/handlers/extensions/lifecycle.rs:169-171` (uninstall)
- Modify: `src/gateway/handlers/plugins/handlers/runtime.rs:3-5` (imports), `:135-232` (delete `handle_load` + `handle_unload`), `:11-13` (doc comment naming `plugins.load`)
- Modify: `src/gateway/handlers/plugins/types.rs:95-113` (delete `LoadPluginParams` / `UnloadPluginParams`)
- Modify: `src/gateway/handlers/plugins/handlers/tests.rs:2-5` (imports), `:48-60` (two param tests), `:142-248` (six handler tests)
- Modify: `src/gateway/handlers/mod.rs:371-372` (registrations), `:1368-1369` (asserts)
- Modify: `src/gateway/method_census.rs:335`, `:337` (rulings for the two retired methods)
- Modify: `src/bin/aleph-server/commands/start/mod.rs:1429-1459` (delete the catch-up task)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:216-247` (delete the boot-only plugin-command registration)
- Modify: `src/extension/plugin_ops.rs:24-31`, `:120-127`, `:145-151` (runtime-loaded check replaces the lazy load)
- Modify: `src/extension/service_ops.rs:16-27` (`start_service` loaded check)
- Modify: `docs/reference/PLUGIN_SYSTEM.md:481-483` (R1.2 / G-6: the namespace-inequality paragraph names `load` / `unload`; same commit)
- Test: `src/gateway/handlers/mod.rs::tests::test_plugin_handlers_registered`, `src/gateway/method_census.rs::tests::every_registered_rpc_method_has_a_recorded_ruling`

Census (run before editing, keep the output in the commit message): `rg -n '"plugins\.load"|"plugins\.unload"|plugins\.load\b|plugins\.unload\b' src interfaces shared qa crates docs/reference --glob '!**/dist/**'` → hits only in `src/gateway/handlers/mod.rs` (2 registrations + 2 asserts), `src/gateway/method_census.rs` (2 rulings), `src/gateway/handlers/plugins/handlers/tests.rs` (their own tests), `src/gateway/handlers/plugins/types.rs` (doc comments), `src/gateway/handlers/plugins/handlers/runtime.rs:13` (a doc comment). No Panel / TUI / CLI / qa client: these two methods are zero-client and their only bodies are the functions P1.11 deletes.

**Interfaces:**
- Consumes: `ExtensionManager::{mount, unmount, reload, UnmountError}` (P1.9), `PluginLoader::is_loaded`.
- Produces: nothing new; removes `plugins.load`, `plugins.unload` from the RPC surface.

- [ ] **Step 1: Write the failing test**

`src/gateway/handlers/mod.rs:1360-1372` `test_plugin_handlers_registered` — delete the two lines

```rust
        assert!(registry.has_method("plugins.load"));
        assert!(registry.has_method("plugins.unload"));
```

and add after the remaining asserts:

```rust
        // Retired 2026-09-20: their only bodies were the lazy-load / unload
        // paths the lifecycle primitives replaced, and no client ever called
        // them (census in the commit that removed them).
        assert!(!registry.has_method("plugins.load"));
        assert!(!registry.has_method("plugins.unload"));
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_plugin_handlers_registered -- --nocapture`
Expected: FAIL — `assertion failed: !registry.has_method("plugins.load")`.

- [ ] **Step 3: Write minimal implementation**

**`manage.rs` uninstall (`:209-224`)** — replace

```rust
    if let Ok(manager) = get_extension_manager() {
        match manager.unload_runtime_plugin(&params.name).await {
            Ok(()) => {}
            // Never loaded into the runtime — nothing to tear down.
            Err(crate::extension::ExtensionError::PluginNotFound(_)) => {}
            Err(e) => tracing::warn!(
                plugin = %params.name,
                error = %e,
                "Failed to unload plugin runtime before uninstall"
            ),
        }
    }
```

with

```rust
    if let Ok(manager) = get_extension_manager() {
        match manager.unmount(&params.name).await {
            Ok(report) if !report.all_ok() => tracing::warn!(
                plugin = %params.name, ?report,
                "some effects did not dispose cleanly before uninstall"
            ),
            Ok(_) => {}
            // Never mounted (disabled / errored / unknown) — nothing to tear down.
            Err(crate::extension::UnmountError::NotMounted(_))
            | Err(crate::extension::UnmountError::NotFound(_)) => {}
        }
    }
```

**`manage.rs` enable (`:258-266`)** — replace

```rust
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, true).await;
        manager.sync_plugin_services().await;
    }
```

with

```rust
    // `set_plugin_enabled(_, true)` mounts: MCP servers, services, memory
    // extension and slash entries all come up inside the mount, so the
    // `sync_plugin_services` that used to follow here is gone.
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, true).await;
    }
```

and the doc comment above it (`:245-249`) loses "and brings declared autostart services up" → "which mounts the plugin".

**`manage.rs` disable (`:319-333`)** — replace

```rust
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, false).await;
        match manager.unload_runtime_plugin(&params.name).await {
            Ok(()) => {}
            // Never loaded into the runtime — nothing to tear down.
            Err(crate::extension::ExtensionError::PluginNotFound(_)) => {}
            Err(e) => tracing::warn!(
                plugin = %params.name,
                error = %e,
                "Failed to unload plugin runtime on disable"
            ),
        }
    }
```

with

```rust
    // `set_plugin_enabled(_, false)` unmounts: the scope's disposers stop
    // services and remove transient MCP servers, so nothing follows it.
    if let Ok(manager) = get_extension_manager() {
        manager.set_plugin_enabled(&params.name, false).await;
    }
```

**`extensions/lifecycle.rs:169-171`** — replace

```rust
    if let Some(mgr) = crate::extension::try_extension_manager() {
        let _ = mgr.unload_runtime_plugin(plugin_id).await;
    }
```

with

```rust
    if let Some(mgr) = crate::extension::try_extension_manager() {
        match mgr.unmount(plugin_id).await {
            Ok(_) => {}
            // Never mounted — nothing to tear down; the directory goes anyway.
            Err(crate::extension::UnmountError::NotMounted(_))
            | Err(crate::extension::UnmountError::NotFound(_)) => {}
        }
    }
```

**`runtime.rs`** — delete `handle_load` (`:135-194`, from `/// Load a runtime plugin from a path` through its closing `}`) and `handle_unload` (`:196-232`, from `/// Unload a runtime plugin` through its closing `}`). Imports (`:3-5`) become `use super::super::types::{CallToolParams, ExecuteCommandParams, ReloadPluginParams};`. The doc line `:13` "The plugin must be loaded first via `plugins.load`." becomes "The plugin must be mounted (enabled) — WASM modules load at mount."

**`types.rs:95-113`** — delete the `// Load/Unload Parameters` banner and both structs `LoadPluginParams` / `UnloadPluginParams` (quoted at the census above; `:98-105` and `:107-113`).

**`tests.rs`** — imports (`:2-5`) drop `handle_load, handle_unload`; delete `test_load_plugin_params` (`:48-53`), `test_unload_plugin_params` (`:55-60`), and the six handler tests from `test_handle_load_missing_params` (`:142`) through the end of `test_handle_unload_nonexistent_plugin` (`:225-248`, ending just before `#[test] fn test_execute_command_params`).

**`handlers/mod.rs:371-372`** — delete

```rust
        registry.register("plugins.load", plugins::handle_load);
        registry.register("plugins.unload", plugins::handle_unload);
```

**`method_census.rs:335`, `:337`** — delete `("plugins.load", Class::Admin),` and `("plugins.unload", Class::Admin),` (the census test `every_registered_rpc_method_has_a_recorded_ruling` compares this table against the live registry in both directions; read its two asserts at `:704-720` to confirm a stale ruling for an unregistered method is what turns it red).

**`start/mod.rs:1429-1459`** — delete the whole `if let Some(em) = alephcore::extension::try_extension_manager() { … tokio::spawn(async move { … sync_mcp_plugin_servers … bind_memory_callers … sync_plugin_services … }) }` block (quoted in P1.8's note; the `set_mcp_handle` line was already removed there). Replace the comment at `:1420-1428` ("MCP tool bridge — now that …") tail with:

```rust
    // Plugin MCP servers, memory extensions and services are mounted by
    // `load_all` itself (the handles were installed in `agent_init` before the
    // first load — `boot_order_tests`), so there is no catch-up task here any
    // more. Servers a mount enqueued before this bridge subscribed are picked
    // up by its reconcile pass.
```

**`tool_catalog_init.rs:216-247`** — delete the block from `// Register plugin commands (from CC-format plugins' commands/ directories)` through the closing `}` of that scope (the `get_all_commands` → `SkillInfo` → `register_skills` block, quoted in P1.7's header). Its work is `extension/slash_effect.rs`, run by every mount.

**`plugin_ops.rs`** — in `call_plugin_tool` (`:24-31`), `execute_plugin_hook` (`:120-127`) and `execute_plugin_command` (`:145-151`) replace the identical lazy-load block

```rust
        // Auto-load plugin if not already loaded
        {
            let loader = self.plugin_loader.read().await;
            if !loader.is_loaded(plugin_id) {
                drop(loader);
                self.ensure_plugin_loaded(plugin_id).await?;
            }
        }
```

(the first one is preceded by `self.ensure_plugin_active(plugin_id).await?;` and has a slightly different comment) with a call to one new private helper, placed right after `ensure_plugin_active` (`:42-52`):

```rust
    /// A runtime call needs the plugin's module in the loader. Modules load
    /// at mount (`lifecycle.rs`, `wasm_module` step), never lazily here: a
    /// lazily created effect would sit outside the scope's dispose order.
    async fn ensure_runtime_mounted(&self, plugin_id: &str) -> ExtensionResult<()> {
        self.ensure_plugin_active(plugin_id).await?;
        if self.plugin_loader.read().await.is_loaded(plugin_id) {
            Ok(())
        } else {
            Err(ExtensionError::Runtime(format!(
                "Plugin '{plugin_id}' has no runtime mounted (WASM modules load at mount; \
                 MCP plugin tools are called through McpManager, not the plugin loader)"
            )))
        }
    }
```

so each of the three bodies starts with `self.ensure_runtime_mounted(plugin_id).await?;` and then takes the owned guard as today.

**`service_ops.rs:16-27`** `start_service` — replace

```rust
        if let Err(e) = self.ensure_plugin_loaded(plugin_id).await {
            tracing::warn!(
                plugin = %plugin_id,
                error = %e,
                "start_service: failed to load plugin runtime"
            );
        }
```

with

```rust
        if !self.plugin_loader.read().await.is_loaded(plugin_id) {
            return Err(ExtensionError::Runtime(format!(
                "Plugin '{plugin_id}' has no runtime mounted; enable it first"
            )));
        }
```

**`docs/reference/PLUGIN_SYSTEM.md:481-483`** (R1.2 — the doc line this commit falsifies) — currently:

```markdown
⚠️ **两个命名空间的能力集并不相等**：`callTool` / `executeCommand` / `load` /
`unload` **只**在复数上，`update` / `reload` / `config.*` / `marketplace.*` **只**在
单数上。
```

becomes:

```markdown
⚠️ **两个命名空间的能力集并不相等**：`callTool` / `executeCommand` **只**在复数上，`update` /
`reload` / `config.*` / `marketplace.*` **只**在单数上（`load` / `unload` 于 2026-09-20 CUT——零客户端，
且它们绕过 registry 直接对 WASM loader 寻址，与 mount/unmount 生命周期相悖）。
```

(P4.7d drops `executeCommand` from this sentence when it cuts that method; plan-P5P8 P5.3 holds the final wording — this commit writes only what is true at this point in the sequence.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib test_plugin_handlers_registered -- --nocapture && cargo test -p alephcore --lib method_census -- --nocapture && cargo test -p alephcore --lib gateway::handlers::plugins -- --nocapture && cargo check -p alephcore --bins`
Expected: PASS; `cargo check` clean (warnings about now-unused `ensure_plugin_loaded` / `sync_plugin_services` etc. are expected and are what P1.11 deletes).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/plugins/handlers/manage.rs src/gateway/handlers/extensions/lifecycle.rs src/gateway/handlers/plugins/handlers/runtime.rs src/gateway/handlers/plugins/types.rs src/gateway/handlers/plugins/handlers/tests.rs src/gateway/handlers/mod.rs src/gateway/method_census.rs src/bin/aleph-server/commands/start/mod.rs src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs src/extension/plugin_ops.rs src/extension/service_ops.rs docs/reference/PLUGIN_SYSTEM.md
git commit -m "gateway/plugins: route enable/disable/uninstall through mount/unmount; retire zero-client plugins.load/unload; drop boot catch-up task

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.11: delete the bodies the primitives replaced (with zero-reference proofs)

**Files:**
- Modify: `src/extension/mod.rs:181-189` (field doc), `:206-213` (field doc), `:398-519` (`set_mcp_handle` doc, delete `bind_memory_callers` + `sync_mcp_plugin_servers`), `:1685-1692` (delete test), `:73` (drop the `CapabilityApi` re-export)
- Modify: `src/extension/plugin_ops.rs:53-105`, `:187-220`, `:222-276` (delete three fns)
- Modify: `src/extension/service_ops.rs` (delete `start_autostart_services_transitional`, `sync_plugin_services`, `stop_orphaned_services`)
- Modify: `src/extension/service_manager.rs:445-489` (delete `stop_orphaned`), `:703-7xx` (delete its test)
- Modify: `src/extension/loader.rs` (delete the `.mcp.json` mirror: field, six fns, two match arms; adjust three tests; header doc)
- Modify: `src/extension/registrar/api.rs:93-113` (delete `reload`), `:280-3xx` (delete `test_reload_clears_and_reregisters`), visibility of `CapabilityApi` + `new` + `register_capability` + `registry`
- Modify: `src/extension/registrar/mod.rs:8` (drop the `CapabilityApi` re-export)
- Modify: `src/extension/skill_ops.rs:1-35` (delete `get_all_commands` + its header sentence)
- Modify: `src/extension/registry/plugin_registry/mod.rs:121-134` (delete `enable_plugin`), `:36-40` (doc pointer to `get_all_commands`)
- Modify: `src/memory/extensions/mcp_adapter.rs:53`, `:247`, `:267`, `:275` (four comment strings naming `bind_memory_callers`)
- Modify: `src/extension/registrar/mcp_registrar.rs` (already rewritten in P1.4 — verify no `sync_mcp_plugin_servers` remains)
- Test: `cargo test -p alephcore --lib extension::` + the grep proofs below; no new test (this task removes code)

**Deleted, one line each (path:line at 3ddc1f2e7 → what replaces it):**

| Deleted | Replaced by |
|---|---|
| `mod.rs:413-450 bind_memory_callers` | `register_memory_extension_effect` binds at registration (P1.6) |
| `mod.rs:452-519 sync_mcp_plugin_servers` + test `:1685-1692` | `mcp_server` step in `mount_parsed` (P1.9) |
| `plugin_ops.rs:53-105 ensure_plugin_loaded` | `wasm_module` step at mount; `ensure_runtime_mounted` (P1.10) for callers |
| `plugin_ops.rs:187-220 load_runtime_plugin` | `mount` |
| `plugin_ops.rs:222-276 unload_runtime_plugin` | `unmount` (its stop/unload/remove sequence is the scope's reverse dispose) |
| `service_ops.rs start_autostart_services_transitional` (P1.5 rename of `:57-119`) | `start_services_effect` |
| `service_ops.rs:121-157 sync_plugin_services` | `service` step at mount |
| `service_ops.rs:175-197 stop_orphaned_services` | dispose on unmount / reload |
| `service_manager.rs:445-489 stop_orphaned` + test `:703-…` | `stop_plugin_services` + `forget_plugin` via the disposer |
| `loader.rs:49-50 mcp_configs` field, `:100-121 get_mcp_configs` + `all_mcp_configs_map`, `:184-214 load_mcp_plugin`, `:216-243 load_plugin_with_memory`, `:453-463 has_mcp_plugins` + `mcp_server_count`, the `Mcp` arms of `load_plugin` (`:148`) and `unload_plugin` (`:262-270`), `:8-20` header lines about the MCP flow | `mcp_config::read_mcp_json` called by `mount_parsed`; the loader is WASM-only |
| `api.rs:93-113 CapabilityApi::reload` + test `:280-3xx` | `reload_plugin` = unmount + mount |
| `skill_ops.rs:22-35 get_all_commands` | `slash_effect::plugin_command_skill_infos` over the mount's own capabilities |
| `plugin_registry/mod.rs:121-134 enable_plugin` | `mount` writes `Loaded` through `register_plugin_row` |
| `mod.rs:73 pub use registrar::CapabilityApi;` and `registrar/mod.rs:8 pub use api::CapabilityApi;` | `CapabilityApi` is `pub(crate)` with `pub(super)` methods — internal to `register_plugin_row` |

- [ ] **Step 1: Write the failing test**

No new test. The guard for this task is the grep proof in Step 4; the RED is `cargo check` warnings `function … is never used` after P1.10 (confirm with `cargo check -p alephcore 2>&1 | rg 'never used'` — expected to name `ensure_plugin_loaded`, `load_runtime_plugin`, `unload_runtime_plugin`, `sync_plugin_services`, `stop_orphaned_services`, `bind_memory_callers`, `sync_mcp_plugin_servers`, `start_autostart_services_transitional`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo check -p alephcore 2>&1 | rg -c 'never used'`
Expected: a non-zero count naming the functions above.

- [ ] **Step 3: Write minimal implementation**

Delete each item in the table. Specifics that are not plain deletions:

`src/extension/loader.rs`:
- `load_plugin` (`:141-155`) becomes:

```rust
    /// Load a plugin's in-process runtime. Only `Wasm` has one; `Mcp`
    /// plugins are MCP *servers* and are mounted by `lifecycle.rs` through
    /// `McpManager` (`mcp_server` step), `Static` plugins have none.
    pub(super) fn load_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
        if self.is_loaded(&manifest.id) {
            warn!("Plugin {} is already loaded, skipping", manifest.id);
            return Ok(());
        }
        match manifest.kind {
            PluginKind::Wasm => self.load_wasm_plugin(manifest),
            PluginKind::Mcp | PluginKind::Static => {
                info!("Plugin {} has no in-process runtime, skipping loader", manifest.id);
                Ok(())
            }
        }
    }
```

- `unload_plugin` (`:246-281`): delete the `Some(PluginKind::Mcp) => { … }` arm (`:262-270`) and fold it into `Some(PluginKind::Static) | Some(PluginKind::Mcp) => { info!("Plugin '{}' removed from tracking", plugin_id); }`.
- `is_any_runtime_active` (`:69-72`) → `self.wasm_runtime.is_some()`; `shutdown` (`:436-449`) drops `self.mcp_configs.clear();`; `new()` drops the field init; the `use crate::mcp::McpManagerConfig;` import goes; the header (`:8-20`) drops the `mcp_configs` line of the diagram and the "MCP Plugin Flow" paragraph, replaced by one sentence: "MCP plugins have no in-process runtime here; `lifecycle.rs` hands their `.mcp.json` servers to `McpManager`."
- Tests: delete `test_plugin_loader_mcp_configs_empty`; in `test_plugin_loader_mcp_tool_call_rejected` and `test_plugin_loader_mcp_hook_rejected` (they load an MCP manifest and assert the "must go through McpManager" error) change the assertion to `matches!(err, ExtensionError::PluginNotFound(_))` and the test names to `…_is_not_loaded_into_the_loader`, because an MCP manifest no longer enters `loaded_plugins` at all; the `use crate::memory::extensions::{McpMemoryExtension, MemoryExtensionRegistry};` import stays (the effect producer uses it).

`src/extension/registrar/api.rs`:
- `pub struct CapabilityApi<'a>` → `pub(crate) struct CapabilityApi<'a>`; `pub const fn new` → `pub(super) const fn new`; `pub fn register_capability` → `pub(super) fn register_capability`; `pub const fn registry` → `pub(super) const fn registry`. Delete `reload` (`:93-113`) and `test_reload_clears_and_reregisters` (`:280` through its closing brace).
- The module doc (`:1-5`) "It is the single entry point that all registrars (MCP, WASM, manifest adapters) use to write into the registry" → "It is the write path `register_plugin_row` (the `registry_row` effect) uses; nothing else writes capabilities into the registry."

`src/extension/mod.rs`:
- `memory_registry` field doc (`:181-189`) → "When set, `mount` registers a plugin's `[memory]` section as a `McpMemoryExtension` (`memory_extension` step); `None` records a skip."
- `mcp_handle` field doc (`:206-213`) → "…used by `mount` to register plugin-owned MCP servers as transient servers (`mcp_server` step). `None` until `set_mcp_handle` at boot; CLI/test paths leave it unset and the step is recorded as skipped."
- `set_mcp_handle` doc (`:398-409`) → "Inject the live MCP manager handle. Call it at boot BEFORE the first `load_all` (see `agent_init::boot_order_tests`); a mount that runs without it records `mcp_server` as skipped."
- Delete `bind_memory_callers` (`:413-450`), `sync_mcp_plugin_servers` (`:452-519`), the test `sync_mcp_plugin_servers_is_noop_without_handle` (`:1685-1692`), and `pub use registrar::CapabilityApi;` (`:73`).

`src/memory/extensions/mcp_adapter.rs` — the four comments: `:53` "`bind_memory_callers` will route this plugin's hook calls to" → "the `memory_extension` mount step binds this plugin's hook calls to"; `:247` "`ExtensionManager::bind_memory_callers` replaces this with a real binding" → "`register_memory_extension_effect` replaces this with a real binding when the MCP handle is attached"; `:267` (inside the error string) "bind_memory_callers wires the real McpManager" → "the plugin was mounted without an MCP manager handle"; `:275` "Constructed at boot by `bind_memory_callers` once the manager handle is available" → "Constructed at mount by `register_memory_extension_effect` when the manager handle is available".

`src/extension/skill_ops.rs` — delete `get_all_commands` (`:22-35`) and the sentence in the header (`:11-12`) "What remains: [`ExtensionManager::get_all_commands`] (the only list query still used at boot)," → "What remains:". `plugin_registry/mod.rs:36-40` doc "see [`crate::extension::ExtensionManager::get_all_commands`]" → "see `extension/slash_effect.rs`".

- [ ] **Step 4: Run test to verify it passes**

Run, and paste the (empty) outputs into the commit body:

```
rg -n 'ensure_plugin_loaded|load_runtime_plugin|unload_runtime_plugin|sync_mcp_plugin_servers|bind_memory_callers|sync_plugin_services|stop_orphaned_services|start_autostart_services|stop_orphaned\(|load_plugin_with_memory|load_mcp_plugin|all_mcp_configs_map|get_mcp_configs|has_mcp_plugins|mcp_server_count\(\)|get_all_commands|enable_plugin\(|CapabilityApi::reload|api\.reload\(' src interfaces shared qa crates tests
```

Expected: no output (note the `mcp_server_count()` with parens — the `PluginRecord.mcp_server_count` *field* stays and is read by `tools/usage/report.rs`). Then:

```
cargo check -p alephcore 2>&1 | rg -c 'never used'      # expected: 0
cargo test -p alephcore --lib extension::                # PASS
cargo test -p alephcore --lib memory::extensions         # PASS
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
```

`src/harness/` must show 0 lines in `git diff --stat 3ddc1f2e7 -- src/harness/`.

- [ ] **Step 5: Commit**

```bash
git add src/extension/mod.rs src/extension/plugin_ops.rs src/extension/service_ops.rs src/extension/service_manager.rs src/extension/loader.rs src/extension/registrar/api.rs src/extension/registrar/mod.rs src/extension/skill_ops.rs src/extension/registry/plugin_registry/mod.rs src/memory/extensions/mcp_adapter.rs
git commit -m "extension: delete the lazy-load / sync / orphan-sweep bodies the lifecycle primitives replaced

Zero-reference proof (rg over src interfaces shared qa crates tests): <paste>

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.12: G1 — census: every crate-visible registration function in the effect-owning files returns a `Disposer`

**Files:**
- Create: `src/extension/effects/census.rs` (`#[cfg(test)]` only)
- Modify: `src/extension/effects/mod.rs` (declare `#[cfg(test)] mod census;`)
- Test: `src/extension/effects/census.rs`

Shape: same family as `src/extension/projection.rs:162` (source scan, comment lines stripped, self-count so a blind scanner cannot pass) and `src/capability/census.rs` (derived membership, not a hand-written roster). The rule this pins is scan-dsh-cordis.md Top-8 #1's risk: "a Rust registry that stores disposers but lets some registration paths bypass the handle recreates the leak".

**Predicate:**
- Files: `src/extension/registrar/api.rs`, `src/extension/registrar/mcp_registrar.rs`, `src/extension/loader.rs`, `src/extension/service_ops.rs`, `src/extension/slash_effect.rs` — the files that own the six producers. (`src/memory/extensions/registry.rs` is NOT in the set: its `register` is the first-party boot path (`agent_init/mod.rs:326`), and its `register_mcp` is covered by the second assertion below — one caller, in `loader.rs`.)
- A declaration `pub fn` / `pub async fn` / `pub(crate) fn` / `pub(crate) async fn` whose name starts with `register_`, `load_`, `start_`, `add_` or `mount_` must have `Disposer` in its return type. `pub(super)` and private fns are module-internal write paths and are not census'd (that is why P1.3 / P1.11 narrowed `load_plugin`, `CapabilityApi::new`, `register_capability`).
- `EXEMPT`: `(service_ops.rs, start_service)` — the operator verb behind `services.start`; the plugin's `service` step disposer (`start_services_effect`) stops **every** registered service of the plugin, so a manual start never outlives the mount. Each exempt entry must still exist in the scan, or the census fails ("stale exemption").
- Second assertion: `.register_mcp(` has exactly one non-test, non-comment call site in `src/`, in `src/extension/loader.rs` — the memory-extension effect is the only plugin path into that registry.

- [ ] **Step 1: Write the failing test**

`src/extension/effects/census.rs`:

```rust
//! G1 — the registration-returns-a-Disposer census (spec §3.5).
//!
//! Source-level, because at runtime a registration that bypassed the scope
//! looks exactly like one that went through it: both mutate shared state,
//! and only the next unmount would tell — silently.

use std::path::{Path, PathBuf};

/// Files that own the six effect producers. A new producer goes in one of
/// these, or this list grows in the same commit.
const CENSUS_FILES: [&str; 5] = [
    "src/extension/registrar/api.rs",
    "src/extension/registrar/mcp_registrar.rs",
    "src/extension/loader.rs",
    "src/extension/service_ops.rs",
    "src/extension/slash_effect.rs",
];

/// Name prefixes that mean "this writes something into the runtime".
const EFFECT_VERBS: [&str; 5] = ["register_", "load_", "start_", "add_", "mount_"];

/// (file, fn, why) — a crate-visible effect verb that legitimately does not
/// return a `Disposer`. Every entry must still exist, or the census is red.
const EXEMPT: [(&str, &str, &str); 1] = [(
    "src/extension/service_ops.rs",
    "start_service",
    "operator verb behind `services.start`; the plugin's `service` step disposer \
     stops every registered service of the plugin, so a manual start never \
     outlives the mount",
)];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The non-test, non-comment text of a source file.
fn production_text(path: &Path) -> String {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let cut = src.find("#[cfg(test)]\nmod tests").unwrap_or(src.len());
    src[..cut]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every crate-visible `fn` declaration in `text` whose name starts with an
/// effect verb, with its signature (declaration up to the opening brace).
fn crate_visible_effect_fns(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        let vis_ok = t.starts_with("pub fn ")
            || t.starts_with("pub async fn ")
            || t.starts_with("pub(crate) fn ")
            || t.starts_with("pub(crate) async fn ");
        if !vis_ok {
            continue;
        }
        let after_fn = &t[t.find("fn ").unwrap() + 3..];
        let name: String = after_fn
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !EFFECT_VERBS.iter().any(|v| name.starts_with(v)) {
            continue;
        }
        // Signature may span lines: read until the body's `{`.
        let mut sig = t.to_string();
        while !sig.contains('{') {
            match lines.next() {
                Some(l) => {
                    sig.push(' ');
                    sig.push_str(l.trim());
                }
                None => break,
            }
        }
        out.push((name, sig));
    }
    out
}

#[test]
fn every_crate_visible_registration_returns_a_disposer() {
    let root = repo_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut disposer_returning = 0usize;
    let mut exempt_seen: Vec<(&str, &str)> = Vec::new();

    for rel in CENSUS_FILES {
        let text = production_text(&root.join(rel));
        for (name, sig) in crate_visible_effect_fns(&text) {
            if let Some((f, n, _)) = EXEMPT.iter().find(|(f, n, _)| *f == rel && *n == name) {
                exempt_seen.push((f, n));
                continue;
            }
            let returns_disposer = sig
                .split("->")
                .nth(1)
                .is_some_and(|ret| ret.contains("Disposer"));
            if returns_disposer {
                disposer_returning += 1;
            } else {
                offenders.push(format!("{rel}::{name} — `{}`", sig.trim()));
            }
        }
    }

    // Self-count: six producers exist today; a scanner that finds fewer is
    // not reading what it thinks it is.
    assert!(
        disposer_returning >= 6,
        "census found only {disposer_returning} Disposer-returning registration fns — \
         the scanner is blind (six producers are known to exist)"
    );
    for (f, n, _) in EXEMPT {
        assert!(
            exempt_seen.contains(&(f, n)),
            "stale exemption: {f}::{n} no longer exists — remove it from EXEMPT"
        );
    }
    assert!(
        offenders.is_empty(),
        "crate-visible registration fns that do not return a Disposer (spec §3.1: every \
         effect is owned by the plugin's EffectScope; either return `Disposer`, make it \
         `pub(super)` so it is an inner write path of one that does, or add an EXEMPT \
         entry with a reason):\n  {}",
        offenders.join("\n  ")
    );
}

/// The memory registry's plugin path has exactly one caller, in `loader.rs`
/// (`register_memory_extension_effect`). A second caller would be a
/// `[memory]` registration outside the scope.
#[test]
fn register_mcp_has_exactly_one_production_caller_and_it_is_the_effect() {
    let root = repo_root().join("src");
    let mut hits: Vec<String> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = production_text(&path);
            for (i, line) in text.lines().enumerate() {
                if line.contains(".register_mcp(") && !line.contains("fn ") {
                    hits.push(format!("{}:{}", path.strip_prefix(&root).unwrap().display(), i + 1));
                }
            }
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "`.register_mcp(` must have exactly one production caller (the memory_extension \
         effect); found: {hits:?}"
    );
    assert!(
        hits[0].starts_with("extension/loader.rs:"),
        "the one caller must be register_memory_extension_effect in loader.rs, not {}",
        hits[0]
    );
}
```

`src/extension/effects/mod.rs` — add `#[cfg(test)] mod census;` after `mod scope;`.

- [ ] **Step 2: Run test to verify it fails**

Run it against the P1.11 tree first — it should be GREEN there (all producers exist). The RED is the mutation step below; do it once now, record the output, revert:

1. In `src/extension/registrar/api.rs` add `pub fn register_extra(_x: u8) {}` at the end of the file (outside tests). Run `cargo test -p alephcore --lib effects::census -- --nocapture`. Expected: FAIL `every_crate_visible_registration_returns_a_disposer` naming `src/extension/registrar/api.rs::register_extra`. Revert.
2. In `src/extension/service_ops.rs` rename `start_service` to `start_service_manual` (both def and its caller in `services.rs` handlers). Expected: FAIL "stale exemption: src/extension/service_ops.rs::start_service no longer exists". Revert.
3. In `src/extension/service_ops.rs` add inside `start_services_effect`, before the return: `let _ = self.memory_registry.read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.register_mcp(std::sync::Arc::new(crate::memory::extensions::McpMemoryExtension::new_unbound("x".into(), None))));`. Expected: FAIL `register_mcp_has_exactly_one_production_caller_and_it_is_the_effect` "found: [\"extension/loader.rs:…\", \"extension/service_ops.rs:…\"]". Revert.

- [ ] **Step 3: Write minimal implementation**

None beyond the test file — the production shape was built in P1.2–P1.7 and narrowed in P1.11. If the first (unmutated) run reports an offender, that offender is a real finding: fix its visibility or return type in the producer file, do not widen `EXEMPT`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib effects::census -- --nocapture`
Expected: PASS (2 tests). Record the three mutation outputs from Step 2 in the commit body.

- [ ] **Step 5: Commit**

```bash
git add src/extension/effects/census.rs src/extension/effects/mod.rs
git commit -m "extension/effects: G1 census — crate-visible registration fns return a Disposer; register_mcp has one caller

Mutations recorded red: register_extra in api.rs; start_service renamed; second register_mcp caller.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.13: G2 — round-trip: two fixtures covering all six effects; mount → snapshot → unmount → snapshot == before

**Files:**
- Modify: `src/extension/lifecycle.rs` (`mod tests` — add the fixture writers, the `Surfaces` snapshot and two tests)
- Test: `src/extension/lifecycle.rs`

Two fixtures because no single plugin kind produces all six effects: a `Wasm` plugin (empty module, P1.3) yields `registry_row` + `wasm_module` + `service` + `memory_extension` + `slash_command`; an `Mcp` plugin yields `registry_row` + `mcp_server` + `memory_extension` + `slash_command`. The MCP *server* surface itself (a process that starts and stops) cannot be observed in-process with a nonexistent binary; that half is `tests/plugin_lifecycle_roundtrip.rs` (P1.14) with a real mock server. Here the `mcp_server` step is asserted by label and its disposer runs on unmount (a remove for a never-started server, `actor.rs:624-641`).

**Interfaces:**
- Consumes: `ExtensionManager::{set_mcp_handle, set_memory_registry, set_tool_catalog, load_all, set_plugin_enabled, reload, scope_steps}`, `loader::EMPTY_WASM_MODULE`, `McpManagerActor`, `MemoryExtensionRegistry::mcp_bindings_snapshot`, `ToolCatalog::list_all`, `HookExecutor::inventory`, `list_services`.
- Produces: nothing new (guard only).

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` of `src/extension/lifecycle.rs`:

```rust
    /// WASM fixture: every effect kind except `mcp_server`.
    fn write_wasm_fixture(root: &Path) {
        let dir = root.join("plugins").join("qa-wasm");
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        std::fs::write(dir.join("plugin.wasm"), crate::extension::loader::EMPTY_WASM_MODULE).unwrap();
        std::fs::write(
            dir.join("aleph.plugin.toml"),
            r#"[plugin]
id = "qa-wasm"
name = "QA WASM"
kind = "wasm"
entry = "plugin.wasm"

[permissions]
background = true

[[services]]
id = "ticker"
start_handler = "start_ticker"
stop_handler = "stop_ticker"
auto_start = false

[memory]
hooks = ["on_retrieve"]
priority = 50
"#,
        )
        .unwrap();
        std::fs::write(dir.join("commands/hello.md"), "---\ndescription: hi\n---\nHi $ARGUMENTS\n").unwrap();
        std::fs::write(dir.join("agents/helper.md"), "---\nname: helper\ndescription: helps\n---\nYou help.\n").unwrap();
        std::fs::write(
            dir.join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"echo qa"}]}]}}"#,
        )
        .unwrap();
    }

    /// MCP fixture: `registry_row` + `mcp_server` + `memory_extension` + `slash_command`.
    fn write_mcp_fixture(root: &Path) {
        let dir = root.join("plugins").join("qa-mcp");
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(
            dir.join("aleph.plugin.toml"),
            r#"[plugin]
id = "qa-mcp"
name = "QA MCP"
kind = "mcp"

[memory]
hooks = ["on_retrieve"]
priority = 60
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"mock":{"command":"qa-nonexistent-mcp-binary-9f3a","args":[]}}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("commands/ping.md"), "---\ndescription: ping\n---\nPong\n").unwrap();
    }

    /// Everything a mount can leave behind, read from the six surfaces.
    #[derive(Debug, PartialEq, Eq)]
    struct Surfaces {
        rows: Vec<(String, String)>,
        capability_rows: (usize, usize, usize, usize, usize), // tools, hooks, services, skills, agents
        loaded: Vec<String>,
        memory: Vec<String>,
        slash: Vec<String>,
        hook_view: Vec<String>,
        services: Vec<String>,
    }

    async fn snapshot(
        manager: &ExtensionManager,
        memory: &crate::memory::extensions::MemoryExtensionRegistry,
        catalog: &crate::tool_metadata::ToolCatalog,
    ) -> Surfaces {
        let (rows, capability_rows) = {
            let reg = manager.get_plugin_registry().await;
            let mut rows: Vec<(String, String)> = reg
                .list_plugins()
                .into_iter()
                .map(|r| (r.id.clone(), r.status.label().to_string()))
                .collect();
            rows.sort();
            (
                rows,
                (
                    reg.list_tools().len(),
                    reg.list_hooks().len(),
                    reg.list_services().len(),
                    reg.list_skills().len(),
                    reg.list_agents().len(),
                ),
            )
        };
        let mut loaded: Vec<String> = manager
            .get_plugin_loader()
            .await
            .loaded_plugin_ids()
            .into_iter()
            .map(str::to_string)
            .collect();
        loaded.sort();
        let mut memory: Vec<String> = memory
            .mcp_bindings_snapshot()
            .iter()
            .map(|e| e.name().to_string())
            .collect();
        memory.sort();
        let mut slash: Vec<String> = catalog.list_all().await.into_iter().map(|t| t.name).collect();
        slash.sort();
        let mut hook_view: Vec<String> = manager
            .hook_executor_snapshot()
            .await
            .inventory()
            .into_iter()
            .map(|h| h.source)
            .collect();
        hook_view.sort();
        let mut services: Vec<String> = manager
            .list_services()
            .await
            .into_iter()
            .map(|s| format!("{}:{}", s.plugin_id, s.id))
            .collect();
        services.sort();
        Surfaces {
            rows,
            capability_rows,
            loaded,
            memory,
            slash,
            hook_view,
            services,
        }
    }

    /// A manager with every handle attached and both fixtures on disk,
    /// disabled in `plugins.toml` so the baseline snapshot is "discovered,
    /// nothing mounted".
    async fn six_effect_bench(
        tmp: &Path,
    ) -> (
        ExtensionManager,
        std::sync::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
        std::sync::Arc<crate::tool_metadata::ToolCatalog>,
    ) {
        write_wasm_fixture(tmp);
        write_mcp_fixture(tmp);
        let (manager, cfg_path) = isolated_manager(tmp).await;
        crate::extension::plugin_state::PluginsConfig {
            entries: ["qa-wasm", "qa-mcp"]
                .into_iter()
                .map(|id| {
                    (
                        id.to_string(),
                        crate::extension::plugin_state::PluginEntryConfig {
                            enabled: Some(false),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
        .save(&cfg_path)
        .await
        .unwrap();
        let (manager2, _) = isolated_manager(tmp).await;
        drop(manager);
        let (actor, handle) =
            crate::mcp::manager::McpManagerActor::new(Some(tmp.join("mcp.json")))
                .await
                .unwrap();
        tokio::spawn(actor.run());
        manager2.set_mcp_handle(handle);
        let memory = std::sync::Arc::new(crate::memory::extensions::MemoryExtensionRegistry::new());
        manager2.set_memory_registry(memory.clone());
        let catalog = std::sync::Arc::new(crate::tool_metadata::ToolCatalog::new());
        manager2.set_tool_catalog(catalog.clone());
        manager2.load_all().await.unwrap();
        (manager2, memory, catalog)
    }

    /// G2. Comment out any one `scope.effect(...)` in `mount_parsed` and the
    /// matching surface stays populated after the disable → this fails by
    /// naming the surface.
    #[tokio::test]
    async fn g2_mount_then_unmount_returns_every_surface_to_its_baseline() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        let (manager, memory, catalog) = six_effect_bench(tmp.path()).await;

        let before = snapshot(&manager, &memory, &catalog).await;
        assert_eq!(
            before.rows,
            vec![("qa-mcp".to_string(), "disabled".to_string()), ("qa-wasm".to_string(), "disabled".to_string())]
        );
        assert_eq!(before.capability_rows, (0, 0, 0, 0, 0), "disabled rows carry no capability rows");
        assert!(before.loaded.is_empty() && before.memory.is_empty() && before.services.is_empty());

        assert!(manager.set_plugin_enabled("qa-wasm", true).await);
        assert!(manager.set_plugin_enabled("qa-mcp", true).await);
        assert_eq!(
            manager.scope_steps("qa-wasm").unwrap(),
            vec!["registry_row", "wasm_module", "service", "memory_extension", "slash_command"]
        );
        assert_eq!(
            manager.scope_steps("qa-mcp").unwrap(),
            vec!["registry_row", "mcp_server", "memory_extension", "slash_command"]
        );
        {
            let all: std::collections::BTreeSet<&str> = manager
                .scope_steps("qa-wasm")
                .unwrap()
                .into_iter()
                .chain(manager.scope_steps("qa-mcp").unwrap())
                .collect();
            let expected: std::collections::BTreeSet<&str> =
                crate::extension::effects::STEP_LABELS.into_iter().collect();
            assert_eq!(all, expected, "the two fixtures together cover every effect kind");
        }
        assert!(manager.scope_skipped("qa-wasm").unwrap().is_empty());
        assert!(manager.scope_skipped("qa-mcp").unwrap().is_empty());
        // The MCP half settles (the nonexistent binary fails inside the actor)
        // without a timer of our own: the actor's handshake cap bounds it.
        tokio::time::timeout(std::time::Duration::from_secs(90), manager.activation_settled())
            .await
            .expect("every server-start watcher finished");

        let during = snapshot(&manager, &memory, &catalog).await;
        assert_ne!(during, before);
        assert_eq!(during.loaded, vec!["qa-wasm".to_string()]);
        assert_eq!(during.memory, vec!["QA MCP".to_string(), "QA WASM".to_string()]);
        assert!(during.slash.contains(&"qa-wasm:hello".to_string()) && during.slash.contains(&"qa-mcp:ping".to_string()));
        assert!(during.hook_view.contains(&"qa-wasm".to_string()));
        assert_eq!(during.capability_rows.2, 1, "one service row");
        assert!(during.services.is_empty(), "auto_start = false: registered, not started");

        assert!(manager.set_plugin_enabled("qa-wasm", false).await);
        assert!(manager.set_plugin_enabled("qa-mcp", false).await);
        let after = snapshot(&manager, &memory, &catalog).await;
        assert_eq!(after, before, "an effect leaked past its unmount");
    }

    /// `reload()` = unmount everything + mount everything: the surfaces
    /// after a reload equal the surfaces before it (no duplicate memory
    /// registration, no doubled slash entries, no stale loader entry).
    #[tokio::test]
    async fn g2_reload_is_a_fixed_point_of_the_surfaces() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        let (manager, memory, catalog) = six_effect_bench(tmp.path()).await;
        manager.set_plugin_enabled("qa-wasm", true).await;
        manager.set_plugin_enabled("qa-mcp", true).await;
        let mounted = snapshot(&manager, &memory, &catalog).await;
        let report = manager.reload().await.unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(snapshot(&manager, &memory, &catalog).await, mounted);
    }
```

- [ ] **Step 2: Run test to verify it fails**

The unmutated tree is expected GREEN. Record the RED with the mutation:

In `mount_parsed` (P1.9), change

```rust
                Some(c) => scope.effect(
                    "slash_command",
                    slash_effect::register_slash_commands_effect(c, command_infos).await,
                ),
```

to

```rust
                Some(c) => {
                    let _leaked = slash_effect::register_slash_commands_effect(c, command_infos).await;
                }
```

Run: `cargo test -p alephcore --lib g2_mount_then_unmount -- --nocapture`
Expected: FAIL — `assertion left == right failed: an effect leaked past its unmount` with the diff showing `slash: ["qa-mcp:ping", "qa-wasm:hello"]` on the left. Revert. Repeat once for the `memory_extension` arm (`Ok(Some(d)) => scope.effect("memory_extension", d)` → `Ok(Some(_d)) => {}`): expected diff on `memory: ["QA MCP", "QA WASM"]`. Revert.

- [ ] **Step 3: Write minimal implementation**

None — guard only. If the unmutated run is red, the diff names the leaking surface; fix the producer/disposer, not the test.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::lifecycle -- --nocapture`
Expected: PASS (all lifecycle tests including the two G2 tests). Also `cargo test -p alephcore --lib extension::loader` (the WASM fixture is shared).

- [ ] **Step 5: Commit**

```bash
git add src/extension/lifecycle.rs
git commit -m "extension/lifecycle: G2 round-trip guard over all six effect kinds (two fixtures) + reload fixed point

Mutations recorded red: slash_command effect dropped; memory_extension effect dropped.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.14: G3 — `publishing_plugin_projections_has_exactly_one_author` also pins the view recomputation to `lifecycle.rs::after_transition`

**Files:**
- Modify: `src/extension/projection.rs:148-232` (the census test) and `:1-46` (module doc — the "every path that can change plugin activation calls it" sentence is now false; see P1.17 for the rewrite, this task only touches the test)
- Test: `src/extension/projection.rs`

**Produces (for later phases):** `projection.rs::tests::G3_PINNED: &[(&str, &str)]` — the pinned (needle → owner file) list as a `const` slice; P6.4 appends `("notify_tools_list_changed(", "lifecycle.rs")` to it in the same commit that adds the call inside `after_transition` (R1.5).

Today's census (`projection.rs:162-232`) pins the two process-global publish calls to `projection.rs`. After P1.9 the derivation has exactly one *trigger*, `after_transition`, and that is the fact G3 adds: `republish_plugin_projections(` and `sync_hooks_from_registry(` may be **called** only from `src/extension/lifecycle.rs`, and `after_transition(` may be called only from that file too. Definition lines (containing `fn `) are excluded; comment lines already are.

- [ ] **Step 1: Write the failing test**

Replace the body of `publishing_plugin_projections_has_exactly_one_author` (`:162-232`) with the following, and add the `G3_PINNED` const at module level inside `mod tests` (above the test) so a later phase extends it by appending one tuple:

```rust
    /// (needle, the only file allowed to contain a non-definition line with it).
    /// A module-level const on purpose: P6 appends
    /// `("notify_tools_list_changed(", "lifecycle.rs")` when `after_transition`
    /// gains the MCP-face broadcast — one line here, no test-body edit.
    pub(super) const G3_PINNED: &[(&str, &str)] = &[
        ("publish_plugin_skill_dirs(", "projection.rs"),
        ("publish_plugin_subagents(", "projection.rs"),
        // The view recomputation has one trigger: lifecycle.rs::after_transition.
        ("republish_plugin_projections(", "lifecycle.rs"),
        ("sync_hooks_from_registry(", "lifecycle.rs"),
        ("after_transition(", "lifecycle.rs"),
    ];

    #[test]
    fn publishing_plugin_projections_has_exactly_one_author() {

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/extension");
        let mut offenders: Vec<String> = Vec::new();
        let mut checked_files = 0usize;
        let mut found: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();

        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                checked_files += 1;
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                for (lineno, line) in src.lines().enumerate() {
                    let code = line.trim_start();
                    if code.starts_with("//") {
                        continue;
                    }
                    for &(needle, owner) in G3_PINNED {
                        if !code.contains(needle) {
                            continue;
                        }
                        // A definition is not a call.
                        if code.contains(&format!("fn {}", needle.trim_end_matches('('))) {
                            continue;
                        }
                        if file_name == owner {
                            *found.entry(needle).or_default() += 1;
                        } else {
                            offenders.push(format!(
                                "{}:{} — `{}` (only {owner} may call this)",
                                path.display(),
                                lineno + 1,
                                code.trim()
                            ));
                        }
                    }
                }
            }
        }

        assert!(
            checked_files > 10,
            "census scanned only {checked_files} files — it is not looking where it thinks it is"
        );
        // Self-check: every pinned call must exist in its owner, or the
        // census is blind to a rename.
        for &(needle, owner) in G3_PINNED {
            assert!(
                found.get(needle).copied().unwrap_or(0) >= 1,
                "expected `{needle}` inside {owner}, found none — did it move? the census is now blind"
            );
        }
        assert!(
            offenders.is_empty(),
            "plugin projections are published only from projection.rs and re-derived only \
             from lifecycle.rs::after_transition (spec §3.2: one trigger after every \
             transition). Second author(s) found:\n  {}",
            offenders.join("\n  ")
        );
    }
```

Update the doc comment above the test (`:151-161`) to name the five needles and the two owners.

- [ ] **Step 2: Run test to verify it fails**

The unmutated P1.11 tree is expected GREEN. Record the RED with the mutation: in `src/extension/plugin_ops.rs`, inside `set_plugin_enabled` after the `let changed = …;` block, insert `self.republish_plugin_projections().await;` (the exact line `set_plugin_enabled` used to have before P1.9).

Run: `cargo test -p alephcore --lib publishing_plugin_projections_has_exactly_one_author -- --nocapture`
Expected: FAIL naming `src/extension/plugin_ops.rs:<line> — self.republish_plugin_projections().await; (only lifecycle.rs may call this)`. Revert.

- [ ] **Step 3: Write minimal implementation**

None — guard only. If the unmutated run reports an offender, it is a real second trigger left behind by P1.9–P1.11 (the watcher's `sync_user_hooks` on a `HooksConfig` change at `mod.rs:936-944` is NOT one — it is not in the pinned list, because a hooks-file edit is not a plugin transition).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::projection -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/extension/projection.rs
git commit -m "extension/projection: G3 census pins republish/sync_hooks/after_transition calls to lifecycle.rs

Mutation recorded red: a republish call re-added in set_plugin_enabled.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.15: integration test `tests/plugin_lifecycle_roundtrip.rs` — a real MCP-kind plugin: enable → disable → enable, server and memory extension appear / disappear / reappear

**Files:**
- Create: `tests/plugin_lifecycle_roundtrip.rs`
- Modify: `Cargo.toml:450-490` (add a `[[test]]` entry with `harness = false`)
- Test: itself (`cargo test -p alephcore --features test-helpers --test plugin_lifecycle_roundtrip`)

Why `harness = false` and a self-re-exec: the MCP server must be a real stdio child that speaks the handshake (`external/connection.rs:335-379`: `server/discover` → `-32601` means legacy → `initialize` → `notifications/initialized` → `tools/list`). There is no MCP mock binary in the repo, `python3` is not on every CI runner (`windows-latest` is in the integration matrix, `aleph-core-ci.yml:73`), an HTTP mock is refused by the transport's SSRF policy (`transport/http.rs:267` uses `SsrfPolicy::default()`, loopback blocked), and a `[[bin]]` would rebuild `alephcore` with `test-helpers` for QA. So the test binary IS the mock: when started with `ALEPH_QA_MCP_MOCK=1` it runs a ~40-line JSON-RPC loop on stdin/stdout and exits. `.mcp.json` `env` is honoured per entry (`mcp_config.rs:213-218`), and the discovery walk finds `$ALEPH_HOME/plugins/installed/<id>` through the one-level monorepo rule (`discovery/scanner.rs:287-306`). One process, so `std::env::set_var("ALEPH_HOME", …)` is safe.

**Interfaces:**
- Consumes: `ExtensionManager::{new, set_mcp_handle, set_memory_registry, set_tool_catalog, load_all, set_plugin_enabled, get_plugin_record}`, `McpManagerActor::new` + `McpManagerHandle::subscribe`, `McpManagerEvent::{ServerStarted, ServerRemoved}`, `MemoryExtensionRegistry::mcp_bindings_snapshot`, `ToolCatalog::list_all`.
- Produces: nothing.

- [ ] **Step 1: Write the failing test**

`Cargo.toml` — next to the other `[[test]]` entries (after `resume_coordinator_integration`, `:489-490`):

```toml
# Self-re-executing: the test binary doubles as the stdio MCP mock server
# (`ALEPH_QA_MCP_MOCK=1`), so it owns `main`. See the file header.
[[test]]
name = "plugin_lifecycle_roundtrip"
harness = false
```

`tests/plugin_lifecycle_roundtrip.rs`:

```rust
//! Plugin lifecycle round trip against a REAL MCP server process.
//!
//! enable → disable → enable of an MCP-kind plugin that also declares a
//! `[memory]` section and a `commands/*.md`: the transient server starts /
//! is removed / starts again (observed on the manager's event bus), the
//! memory extension appears / disappears / reappears, the slash entry too.
//!
//! `harness = false` and self-re-exec: with `ALEPH_QA_MCP_MOCK=1` this same
//! binary runs a minimal stdio MCP server (legacy handshake: `server/discover`
//! → `-32601`, `initialize` → `2025-03-26`, `tools/list` → one tool). No
//! python, no network, no second crate — the mock is the four `match` arms in
//! `run_mock_server`.
//!
//! Exit code is the verdict: 0 = every claim held; 1 = at least one did not
//! (each is printed as `[PASS]` / `[FAIL]`).

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alephcore::discovery::DiscoveryConfig;
use alephcore::extension::{ExtensionConfig, ExtensionManager};
use alephcore::mcp::manager::{McpManagerActor, McpManagerEvent};
use alephcore::memory::extensions::MemoryExtensionRegistry;
use alephcore::tool_metadata::ToolCatalog;
use serde_json::json;
use tokio::sync::broadcast;

const PLUGIN_ID: &str = "qa-mcp-mock";
const PLUGIN_NAME: &str = "QA MCP Mock";
const SERVER_ID: &str = "plugin:qa-mcp-mock/mock";
const SLASH: &str = "qa-mcp-mock:ping";
const WAIT: Duration = Duration::from_secs(30);

fn main() {
    if std::env::var_os("ALEPH_QA_MCP_MOCK").is_some() {
        run_mock_server();
        return;
    }
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let failures = rt.block_on(drive());
    if failures == 0 {
        println!("VERDICT: PASS");
    } else {
        println!("VERDICT: FAIL ({failures} claim(s))");
        std::process::exit(1);
    }
}

// ── the mock server ───────────────────────────────────────────────────────

fn run_mock_server() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        // Notifications (no id) get no answer.
        let Some(id) = msg.get("id").cloned() else { continue };
        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "qa-mock", "version": "0" }
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "tools": [{
                    "name": "qa_echo",
                    "description": "echoes its input",
                    "inputSchema": { "type": "object", "properties": {} }
                }] }
            }),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            other => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            }),
        };
        if writeln!(out, "{response}").is_err() {
            break;
        }
        let _ = out.flush();
    }
}

// ── the drive ─────────────────────────────────────────────────────────────

struct Ledger(usize);
impl Ledger {
    fn check(&mut self, claim: &str, ok: bool, detail: impl std::fmt::Display) {
        println!("  [{}] {claim} — {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.0 += 1;
        }
    }
}

fn plant_plugin(aleph_home: &Path) {
    let dir = aleph_home.join("plugins").join("installed").join(PLUGIN_ID);
    std::fs::create_dir_all(dir.join("commands")).unwrap();
    std::fs::write(
        dir.join("aleph.plugin.toml"),
        format!(
            "[plugin]\nid = \"{PLUGIN_ID}\"\nname = \"{PLUGIN_NAME}\"\nkind = \"mcp\"\n\n\
             [memory]\nhooks = [\"on_retrieve\"]\npriority = 50\n"
        ),
    )
    .unwrap();
    let me = std::env::current_exe().unwrap();
    std::fs::write(
        dir.join(".mcp.json"),
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "mock": {
                    "command": me.to_string_lossy(),
                    "args": [],
                    "env": { "ALEPH_QA_MCP_MOCK": "1" }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("commands/ping.md"), "---\ndescription: ping\n---\nPong\n").unwrap();
}

async fn wait_for(
    events: &mut broadcast::Receiver<McpManagerEvent>,
    pred: impl Fn(&McpManagerEvent) -> bool,
) -> bool {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(e) if pred(&e) => break true,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

async fn has_slash(catalog: &ToolCatalog) -> bool {
    catalog.list_all().await.iter().any(|t| t.name == SLASH)
}

fn started(e: &McpManagerEvent) -> bool {
    matches!(e, McpManagerEvent::ServerStarted { server_id, .. } if server_id == SERVER_ID)
}
fn removed(e: &McpManagerEvent) -> bool {
    matches!(e, McpManagerEvent::ServerRemoved { server_id, .. } if server_id == SERVER_ID)
}

async fn drive() -> usize {
    let mut led = Ledger(0);
    let tmp = tempfile::tempdir().unwrap();
    // One process, set before anything reads it (DiscoveryManager::new, Config::load).
    std::env::set_var("ALEPH_HOME", tmp.path());
    plant_plugin(tmp.path());

    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: tmp.path().to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
        },
        plugins_config_path: Some(tmp.path().join("plugins.toml")),
        ..Default::default()
    })
    .await
    .unwrap();

    let (actor, handle) = McpManagerActor::new(Some(tmp.path().join("mcp.json")))
        .await
        .unwrap();
    tokio::spawn(actor.run());
    let mut events = handle.subscribe(); // BEFORE the first mount
    manager.set_mcp_handle(handle.clone());
    let memory = Arc::new(MemoryExtensionRegistry::new());
    manager.set_memory_registry(memory.clone());
    let catalog = Arc::new(ToolCatalog::new());
    manager.set_tool_catalog(catalog.clone());

    let memory_names = || -> Vec<String> {
        memory.mcp_bindings_snapshot().iter().map(|e| e.name().to_string()).collect()
    };

    // ── boot: load_all mounts it ──
    let summary = manager.load_all().await.unwrap();
    led.check("load_all mounted the plugin", summary.plugins_loaded == 1, format!("plugins_loaded={} errors={:?}", summary.plugins_loaded, summary.errors));
    let row = manager.get_plugin_record(PLUGIN_ID).await;
    led.check("row is Loaded", row.as_ref().is_some_and(|r| r.status.is_active()), format!("{:?}", row.map(|r| r.status)));
    led.check("transient server started (real child, real handshake)", wait_for(&mut events, started).await, SERVER_ID);
    led.check("memory extension registered", memory_names() == vec![PLUGIN_NAME.to_string()], format!("{:?}", memory_names()));
    led.check("slash entry registered", has_slash(&catalog).await, SLASH);

    // ── disable: everything goes ──
    led.check("disable reports a change", manager.set_plugin_enabled(PLUGIN_ID, false).await, "");
    led.check("transient server removed", wait_for(&mut events, removed).await, SERVER_ID);
    led.check("memory extension gone", memory_names().is_empty(), format!("{:?}", memory_names()));
    led.check("slash entry gone", !has_slash(&catalog).await, SLASH);
    let row = manager.get_plugin_record(PLUGIN_ID).await;
    led.check("row is Disabled and listable", row.as_ref().is_some_and(|r| !r.status.is_active()), format!("{:?}", row.map(|r| r.status)));

    // ── enable again: everything comes back (the asymmetry this round fixes) ──
    led.check("enable reports a change", manager.set_plugin_enabled(PLUGIN_ID, true).await, "");
    led.check("transient server started AGAIN", wait_for(&mut events, started).await, SERVER_ID);
    led.check("memory extension back", memory_names() == vec![PLUGIN_NAME.to_string()], format!("{:?}", memory_names()));
    led.check("slash entry back", has_slash(&catalog).await, SLASH);

    // ── leave the fixture clean so the child does not outlive the test ──
    let _ = manager.set_plugin_enabled(PLUGIN_ID, false).await;
    let _ = wait_for(&mut events, removed).await;
    led.0
}
```

- [ ] **Step 2: Run test to verify it fails**

Run against the tree BEFORE P1.9 (e.g. `git stash`-free: check out `3ddc1f2e7` into a scratch worktree, or simply read the reasoning): on the old code `set_plugin_enabled(_, true)` never re-adds the server (`plugin_ops.rs:586-641` has no enable-side runtime work), so "transient server started AGAIN" would FAIL. On the P1.11 tree the test is expected GREEN — run it there:

Run: `cargo test -p alephcore --features test-helpers --test plugin_lifecycle_roundtrip -- --nocapture`
Expected on P1.11: 14 `[PASS]` lines and `VERDICT: PASS`. If "transient server started" fails with a handshake error in the log, run the binary by hand with `ALEPH_QA_MCP_MOCK=1` and pipe `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}` into it to see the mock's answer.

- [ ] **Step 3: Write minimal implementation**

None beyond the test and the `Cargo.toml` entry; it drives P1.9's primitives.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --features test-helpers --test plugin_lifecycle_roundtrip -- --nocapture`
Expected: `VERDICT: PASS`, exit 0. Then `cargo test -p alephcore --features test-helpers --test '*' --no-run` still builds.

- [ ] **Step 5: Commit**

```bash
git add tests/plugin_lifecycle_roundtrip.rs Cargo.toml
git commit -m "tests: plugin lifecycle round trip against a real (self-re-exec) MCP mock server

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.16: `qa/plugins/run.sh scope` — MCP plugin enable → disable → enable on a real daemon; `tools.catalog` changes every time

**Files:**
- Create: `qa/plugins/mcp_mock_server.py` (the same three-arm contract as the Rust mock in P1.15, in python because every QA driver here is python and the QA runs the shipped `aleph-server`, not a test build)
- Create: `qa/plugins/plant_scope.py`
- Create: `qa/plugins/drive_scope.py`
- Modify: `qa/plugins/run.sh:1-16` (usage header), `:338-341` (the `*)` arm's scenario list), insert a `scope)` case before `panel)` (`:301`)
- Modify: `qa/README.md:618-625` (add the stage line)
- Test: `./qa/plugins/run.sh scope` on this worktree's binary

Conventions taken from the existing stages (`run.sh:17-134`): scratch `HOME`/`ALEPH_HOME` via `qa_redirect_home`; `qa_build` with the exit code intact; the generated config is patched by `qa/busy_input/patch_config.py` to an inert provider with an inline fake API key (without it `tools.invoke` answers a boot-phase placeholder — memory note 2026-09-03); WebSocket RPC through `qa/browser_managed/qa_rpc.py::Rpc` (`connect` then `call`/`invoke`); the pass/fail ledger prints as it goes; the fixture greps `$ALEPH_HOME/logs/*.log` at the end.

What this stage proves that the unit and integration tests cannot: the daemon's own boot order (P1.8) and the real tool bridge — a plugin's MCP tools reach `tools.catalog` (`tools/handlers/registration.rs:72-74`: catalog id `mcp:<server_id>:<qualified>`, name `<server_id>__<tool>` sanitized) and leave it on disable, and come back on enable. The log grep for `not attached` is the boot-order detector: a mount that ran before a handle was installed records a skip (design decision 3) and the daemon must never do that.

**Interfaces:**
- Consumes: RPC `plugins.list`, `tools.catalog`, `tools.invoke` (`plugin_manage` `enable`/`disable`); log line `effect skipped: handle not attached` (P1.9).
- Produces: nothing.

- [ ] **Step 1: Write the failing test**

`qa/plugins/mcp_mock_server.py`:

```python
#!/usr/bin/env python3
"""Minimal stdio MCP server for the `scope` stage.

Answers exactly what Aleph's connection layer needs to call a server "started":
`server/discover` → -32601 (so the client falls back to the legacy handshake),
`initialize` → protocol 2025-03-26 with a tools capability, `tools/list` → one
tool, `ping` → {}. Everything else → -32601. Notifications are ignored.

Same contract as the Rust mock inside `tests/plugin_lifecycle_roundtrip.rs`;
python here because every QA driver is python and this stage runs the shipped
`aleph-server`, not a test-feature build.
"""
import json
import sys

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue
    try:
        msg = json.loads(raw)
    except json.JSONDecodeError:
        continue
    if "id" not in msg:
        continue  # notification
    rid, method = msg["id"], msg.get("method", "")
    if method == "initialize":
        result = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "qa-mock", "version": "0"}}
        out = {"jsonrpc": "2.0", "id": rid, "result": result}
    elif method == "tools/list":
        out = {"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "qa_echo", "description": "echoes its input",
             "inputSchema": {"type": "object", "properties": {}}}]}}
    elif method == "ping":
        out = {"jsonrpc": "2.0", "id": rid, "result": {}}
    else:
        out = {"jsonrpc": "2.0", "id": rid,
               "error": {"code": -32601, "message": f"method not found: {method}"}}
    sys.stdout.write(json.dumps(out) + "\n")
    sys.stdout.flush()
```

`qa/plugins/plant_scope.py`:

```python
#!/usr/bin/env python3
"""Plant one MCP-kind plugin whose server is `mcp_mock_server.py`.

`aleph.runtime = "mcp"` is required: a CC `plugin.json` without it parses as
`PluginKind::Static` (`manifest/cc_plugin_json.rs:217`) and its servers are
never mounted — which is also why the `manifest` stage's `qa-inline` fixture
can point at `echo` and pass.
"""
import json
import sys
from pathlib import Path

installed = Path(sys.argv[1])
mock = Path(sys.argv[2]).resolve()
d = installed / "qa-scope"
(d / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(d / "commands").mkdir(parents=True, exist_ok=True)
(d / ".claude-plugin" / "plugin.json").write_text(json.dumps({
    "name": "qa-scope",
    "version": "1.0.0",
    "description": "scope stage: MCP server + one command",
    "aleph": {"runtime": "mcp"},
}, indent=2))
(d / ".mcp.json").write_text(json.dumps({
    "mcpServers": {"mock": {"command": sys.executable, "args": [str(mock)]}}
}, indent=2))
(d / "commands" / "qa-scope-cmd.md").write_text("---\ndescription: scope stage command\n---\nPong\n")
print(f"planted {d}")
```

`qa/plugins/drive_scope.py`:

```python
#!/usr/bin/env python3
"""enable → disable → enable of `qa-scope`; the catalogue must change each time.

Two entries are watched in `tools.catalog`: the plugin's slash command
(`qa-scope:qa-scope-cmd`, registered by the `slash_command` effect) and the
mock server's tool (name ends with `__qa_echo`; the exact prefix is the
sanitised server id, which this driver does not re-derive). Presence is
polled, because the MCP half is asynchronous on purpose: the mount enqueues
the server start and the tool bridge registers the tool when the handshake
completes.
"""
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc, ws_connect  # noqa: E402

WS = sys.argv[1]
L = Ledger()
SLASH = "qa-scope:qa-scope-cmd"


def names(node, out):
    if isinstance(node, dict):
        if isinstance(node.get("name"), str):
            out.add(node["name"])
        for v in node.values():
            names(v, out)
    elif isinstance(node, list):
        for v in node:
            names(v, out)
    return out


async def catalog(rpc):
    msg = await rpc.call("tools.catalog", {})
    return names(msg.get("result", {}), set())


async def poll(rpc, want_present, label, seconds=30):
    """Poll until both watched entries are (present|absent), or time out."""
    for _ in range(seconds * 2):
        cat = await catalog(rpc)
        slash = SLASH in cat
        echo = any(n.endswith("__qa_echo") for n in cat)
        if slash == want_present and echo == want_present:
            L.check(label, True, f"slash={slash} echo={echo}")
            return True
        await asyncio.sleep(0.5)
    cat = await catalog(rpc)
    L.check(label, False, f"timed out; slash={SLASH in cat} echo={any(n.endswith('__qa_echo') for n in cat)}")
    return False


async def status(rpc):
    msg = await rpc.call("plugins.list", {})
    for row in msg.get("result", {}).get("plugins", []):
        if row.get("name") == "qa-scope":
            return row.get("status")
    return None


async def main():
    async with ws_connect(WS) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-scope")

        L.log("\n--- boot: the plugin is mounted by load_all ---")
        L.check("plugins.list shows qa-scope loaded", await status(rpc) == "loaded", await status(rpc))
        await poll(rpc, True, "slash command AND mock tool are in tools.catalog after boot")

        L.log("\n--- disable ---")
        ok, body = await rpc.invoke("plugin_manage", {"action": "disable", "name": "qa-scope"})
        L.check("plugin_manage disable answered", ok, str(body)[:200])
        L.check("plugins.list shows disabled", await status(rpc) == "disabled", await status(rpc))
        await poll(rpc, False, "both entries left tools.catalog on disable")

        L.log("\n--- enable again (the direction that used to do nothing) ---")
        ok, body = await rpc.invoke("plugin_manage", {"action": "enable", "name": "qa-scope"})
        L.check("plugin_manage enable answered", ok, str(body)[:200])
        L.check("plugins.list shows loaded again", await status(rpc) == "loaded", await status(rpc))
        await poll(rpc, True, "both entries are back in tools.catalog on enable")

    L.log(f"\nVERDICT: {'PASS' if not L.failures else 'FAIL'}")
    return 0 if not L.failures else 1


sys.exit(asyncio.run(main()))
```

(`Ledger.check` records into `self.failures` — confirm the attribute name at `qa/browser_managed/qa_rpc.py:72-96`; `plugins.list` row keys `name` / `status` are what `drive_plugins.py::rows` walks and `PluginInfo` serialises, `plugin_ops.rs:282-300`.)

`qa/plugins/run.sh` — usage header (`:3-14`): add

```
#   ./qa/plugins/run.sh scope      # MCP plugin enable → disable → enable on a real
#                                  # daemon; `tools.catalog` changes each time; no
#                                  # mount ran before its handle was installed
```

Insert before `panel)` (`:301`):

```bash
scope)
  # The claim: a plugin's runtime footprint is one scope that mount creates and
  # unmount takes back — observed from OUTSIDE the process, on the tool
  # catalogue a client actually reads. Until this round `enable` after
  # `disable` re-flipped the status and did nothing else (no server, no slash
  # entry), and the boot-only slash registration meant a plugin enabled after
  # boot had no `/command` at all. Two of the three flips below were silent
  # no-ops that reported success.
  command -v python3 >/dev/null || { echo "UNRUN: python3 not on PATH (the mock MCP server is python)"; exit 2; }
  say "plant an MCP plugin whose server is the python mock"
  python3 "$HERE/plant_scope.py" "$INSTALLED" "$HERE/mcp_mock_server.py" || exit 1

  say "start server"
  start_server || exit 1

  say "drive: enable → disable → enable"
  python3 "$HERE/drive_scope.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" || RC=$?

  say "the daemon never mounted without a handle (boot order)"
  # A mount that ran before its MCP handle / memory registry / tool catalog was
  # installed logs `effect skipped: handle not attached` and records a Warn
  # diagnostic. On the daemon that is a boot-order regression, not a state.
  if grep -h "handle not attached" "$ALEPH_HOME"/logs/*.log 2>/dev/null | grep -q .; then
    echo "  [FAIL] a mount ran before its handle was installed:"; grep -h "handle not attached" "$ALEPH_HOME"/logs/*.log | head -5; RC=1
  else
    echo "  [PASS] no 'handle not attached' skip in the server log"
  fi
  ;;
```

Update the `*)` arm's list (`:339`) to `(manifest | scaffold | trust | browse | marketplaces | scope | panel)`.

`qa/README.md` — after the `trust` line (`:623-624`) add:

```
./qa/plugins/run.sh scope        # MCP plugin enable → disable → enable through `plugin_manage`;
                                 # the slash entry AND the server's tool leave and re-enter
                                 # `tools.catalog` each time (polled — the mount enqueues the
                                 # server start). Also greps the log for a mount that ran
                                 # before its handle was installed. python3 required: UNRUN
                                 # (exit 2), never PASS, without it.
```

- [ ] **Step 2: Run test to verify it fails**

On `3ddc1f2e7` (a scratch worktree of main) the same stage would fail at "both entries are back in tools.catalog on enable" (no re-add on enable) and at "slash command … after boot" would pass only by the boot-time registration; on this worktree at P1.10 (before the boot catch-up task was removed) the log grep would still pass. The RED to record is the stage run **with P1.8 reverted** (`git stash`-free: temporarily move the three `set_*` calls in `agent_init/mod.rs` back below `ensure_loaded()`): expected `[FAIL] a mount ran before its handle was installed` plus `effect skipped: handle not attached` lines naming `mcp_server` / `slash_command`. Restore.

- [ ] **Step 3: Write minimal implementation**

None beyond the fixture files — the stage drives P1.8–P1.11. If `[FAIL] slash command AND mock tool are in tools.catalog after boot` shows `echo=False` with `slash=True`, read `$ALEPH_HOME/logs/*.log` for the server's handshake error (`MCP server 'plugin:qa-scope/mock' initialize failed`) before touching anything: the mock and the transport disagree, and the log says which side.

- [ ] **Step 4: Run test to verify it passes**

Run: `./qa/plugins/run.sh scope` (from the worktree root; it builds this worktree's `aleph-server` — no `CARGO_TARGET_DIR` override).
Expected: every `[PASS]`, `VERDICT: PASS`, `verdict: rc=0`.

- [ ] **Step 5: Commit**

```bash
git add qa/plugins/run.sh qa/plugins/mcp_mock_server.py qa/plugins/plant_scope.py qa/plugins/drive_scope.py qa/README.md
git commit -m "qa/plugins: scope stage — MCP plugin enable/disable/enable changes tools.catalog on a real daemon

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P1.17: rewrite the `projection.rs` module comment to state approach C (effects in the scope, views by derivation)

**Files:**
- Modify: `src/extension/projection.rs:1-46` (module doc), `:66-76` (`derive_plugin_projection` doc still says "`Disabled` / `Overridden` / `Error` plugins contribute nothing" — keep, it is true), `:110-117` (`republish_plugin_projections` doc: "Call this from every path that can change which plugins are active — load, reload, enable, disable, unload" is now false)
- Test: `cargo test -p alephcore --lib extension::projection` (G3 from P1.14 is the guard; a doc change cannot be red, so this task's check is that the new text names the function the census pins)

The sentence at `:14-24` is the code-side statement of the three "不引 fiber" rulings; the spec (§1.1) requires it rewritten in the same round as the mechanism it describes, or it becomes the lying copy (判据 §1). `HARNESS_PHILOSOPHY.md §8 第五课` is P8's (docs phase) — not touched here.

- [ ] **Step 1: Write the failing test**

None (documentation). The verifiable claim: after the edit, `rg -n 'every path that can change plugin activation calls it' src/extension/` returns nothing, and `rg -n 'after_transition' src/extension/projection.rs` returns at least one line (the comment now points at the one trigger G3 pins).

- [ ] **Step 2: Run test to verify it fails**

Run: `rg -n 'every path that can change plugin activation calls it' src/extension/`
Expected: one hit, `src/extension/projection.rs:23` (the old sentence).

- [ ] **Step 3: Write minimal implementation**

Replace `src/extension/projection.rs:1-46` — currently:

```rust
//! The single place where plugin state becomes a **process-global projection**.
//!
//! # Why this module exists
//!
//! A plugin does not only live in [`PluginRegistry`]. Loading one publishes it
//! into surfaces that outlive any single call:
//!
//! * `utils::paths::PLUGIN_SKILL_DIRS` — read by `get_all_skills_dirs`, i.e. the
//!   search set of the `skill_read` / `skill_list` tools.
//! * `agents::PLUGIN_SUBAGENTS` — read by `AgentRegistry::resolve` (delegation)
//!   and the harness prompt builder (`<available_agents>`).
//! * `SkillSystem` — the scan that feeds the model's `<available_skills>` index.
//! * `ExtensionManager::active_plugin_tools` — the tool-name index.
//!
//! Those are *effects*, not return values: nothing about a later call reminds
//! you they are still installed. Cordis (the DeepSeek-Harness plugin framework)
//! solves the same problem by making every registration an effect on the
//! plugin's fiber, so one `dispose()` unwinds all of them. Aleph deliberately
//! does **not** adopt a fiber runtime (R10 — see `HARNESS_PHILOSOPHY.md` §2.3);
//! the equivalent guarantee here is cheaper and more Aleph-shaped: **one
//! function derives the whole set from the registry, and every path that can
//! change plugin activation calls it.**
//!
//! # The bug this replaces
//!
//! Before this module there were two authors of that derivation — `load_all`
//! and `set_plugin_enabled` — and **they disagreed about the predicate**:
//!
//! | | skill dirs | sub-agents |
//! |---|---|---|
//! | `load_all` | `list_plugins()` — every status | `list_agents()` — unfiltered |
//! | `set_plugin_enabled` | `list_active_plugins()` | filtered by `status.is_active()` |
//!
//! So a boot (or any `reload()`, which the file watcher triggers) published the
//! skills and sub-agents of plugins that were **disabled, shadowed, or failed to
//! load** — the model could read their SKILL.md and delegate to their agents —
//! while a runtime toggle used the correct predicate. Two code paths, opposite
//! answers, and the wrong one ran on every start.
//!
//! The predicate is now stated once, in [`PluginProjection::derive`], and
//! `projection.rs::tests::publishing_plugin_projections_has_exactly_one_author`
//! fails by name if a second author appears.
```

with:

```rust
//! The single place where plugin state becomes a **process-global projection**
//! — the *view* half of the plugin lifecycle.
//!
//! # Effects and views (spec 2026-09-20 §3.1, ruling U8 "C")
//!
//! A plugin's footprint on the running process splits by one question: **does
//! it have an inverse?**
//!
//! * **Effects** have one, and live in the plugin's `EffectScope`
//!   (`effects/scope.rs`): the registry row, the WASM module, transient MCP
//!   servers, background services, the memory extension, slash entries. Each
//!   registration returns a `Disposer`; `lifecycle.rs::unmount` runs the
//!   list in reverse. Guard: `effects::census` (G1) and
//!   `lifecycle::tests::g2_*` (G2).
//! * **Views** have none but can be recomputed from the registry, and that is
//!   what this module does:
//!   - `utils::paths::PLUGIN_SKILL_DIRS` — read by `get_all_skills_dirs`, i.e.
//!     the search set of the `skill_read` / `skill_list` tools.
//!   - `agents::PLUGIN_SUBAGENTS` — read by `AgentRegistry::resolve`
//!     (delegation) and the harness prompt builder (`<available_agents>`).
//!   - `SkillSystem` — the scan that feeds the model's `<available_skills>`
//!     index.
//!   - `ExtensionManager::active_plugin_tools` — the tool-name index.
//!   (The hook executor is the fifth view; `sync_hooks_from_registry` in
//!   `mod.rs` rebuilds it and is triggered from the same place.)
//!
//! **One function derives the whole view set from the registry, and exactly
//! one trigger calls it: `lifecycle.rs::after_transition`, once at the end of
//! every public lifecycle primitive** (`mount` / `unmount` / `reload_plugin` /
//! `reload` / `load_all`). Guard:
//! `tests::publishing_plugin_projections_has_exactly_one_author` (G3) pins
//! both the publish calls to this file and the trigger to `lifecycle.rs`.
//!
//! # What was narrowed, and why
//!
//! Three rounds (2026-08-15 dsh, 08-16 plugin-system, 08-19 compat) ruled
//! "对照 Cordis 但架构不移植", and this comment used to say the derivation was
//! the cheaper equivalent of a fiber's `dispose()` because "every path that
//! can change plugin activation calls it". That was a list (判据 §5): the
//! derivation covered three surfaces, and four effects outside it leaked
//! past a disable — memory extensions (no unregister), slash entries
//! (boot-only), MCP servers on re-enable (only `reload()` re-added them), and
//! the narrow `reload_plugin` twin. The 2026-09-20 round kept the rulings'
//! substance (no DI container, no Proxy context, no cascade restart, no HMR)
//! and absorbed the one thing Cordis actually enforces: **a registration
//! returns its disposer and the plugin handle owns it**
//! (scan-dsh-cordis.md Top-8 #1). `src/harness/` is untouched.
//!
//! # The bug the single derivation fixed (still true)
//!
//! Before this module there were two authors of the derivation — `load_all`
//! and `set_plugin_enabled` — and **they disagreed about the predicate**:
//!
//! | | skill dirs | sub-agents |
//! |---|---|---|
//! | `load_all` | `list_plugins()` — every status | `list_agents()` — unfiltered |
//! | `set_plugin_enabled` | `list_active_plugins()` | filtered by `status.is_active()` |
//!
//! So a boot (or any `reload()`, which the file watcher triggers) published the
//! skills and sub-agents of plugins that were **disabled, shadowed, or failed to
//! load** while a runtime toggle used the correct predicate. The predicate is
//! stated once, in [`ExtensionManager::derive_plugin_projection`].
```

And `republish_plugin_projections`'s doc (`:110-117`):

```rust
    /// Re-derive **all** plugin-owned process-global projections and install
    /// them, replacing whatever was published before.
    ///
    /// Publish is replace-semantics (not append), so this doubles as the
    /// retraction path: a plugin that stopped being active simply is not in the
    /// new vector. Call this from every path that can change which plugins are
    /// active — load, reload, enable, disable, unload.
    ///
    /// Returns the projection that was installed, for logging and tests.
```

becomes:

```rust
    /// Re-derive **all** plugin-owned process-global projections and install
    /// them, replacing whatever was published before.
    ///
    /// Publish is replace-semantics (not append), so this doubles as the
    /// retraction path: a plugin that stopped being active simply is not in the
    /// new vector. Called from exactly one place,
    /// `lifecycle.rs::after_transition` (G3 pins it); do not add a second
    /// caller — route the new path through a lifecycle primitive instead.
    ///
    /// Returns the projection that was installed, for logging and tests.
```

(`PluginProjection::derive` in the old text never existed under that name — the function is `ExtensionManager::derive_plugin_projection`; the new text names the real one.)

- [ ] **Step 4: Run test to verify it passes**

Run: `rg -n 'every path that can change plugin activation calls it' src/extension/ ; rg -c 'after_transition' src/extension/projection.rs && cargo test -p alephcore --lib extension::projection`
Expected: first command prints nothing; second prints a count ≥ 1; tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/extension/projection.rs
git commit -m "extension/projection: module doc states approach C — effects in the scope, views derived from one trigger

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```


---

## Contract deltas

1. `Disposer` = `Box<dyn FnOnce() -> BoxFuture<'static, Result<(), String>> + Send>` (contract: `… -> BoxFuture<'static, ()>`). `sync_disposer` / `async_disposer` take closures returning `Result<(), String>` (`DisposeOutcome`). Reason: `unload_plugin`, `remove_transient_server`, `stop_plugin_services` all report failure; `EffectScope::dispose` logs `step + error` in one place (spec §4). Other phases only consume `mount`/`unmount`, so this is contained to P1.
2. `PluginId` = `String` (type alias in `effects/scope.rs`); every primitive takes `&str`.
3. `EffectScope` gains `skip(step, why)` / `skipped()` / `steps()` / `is_empty()`; `DisposeReport` gains `failures()` / `empty(id)`. `STEP_LABELS` is a `pub const`.
4. `CapabilityApi::register_capability` does NOT return a `Disposer` (it borrows `&mut PluginRegistry` inside the lock guard and cannot own the handle its disposer needs). The `registry_row` producer is the free function `registrar::register_plugin_row(Arc<RwLock<PluginRegistry>>, record, permissions, caps) -> Disposer`. `CapabilityApi` becomes `pub(crate)` with `pub(super)` methods; its `reload` method and both re-exports are deleted.
5. `PluginRegistry::unregister_plugin(id)` already exists (`plugin_registry/mod.rs:432`) — nothing added. `PluginRegistry::enable_plugin` is deleted (last caller was `set_plugin_enabled`).
6. WASM producer is `loader::load_wasm_effect(Arc<RwLock<PluginLoader>>, &PluginManifest) -> ExtensionResult<Disposer>` in `src/extension/loader.rs` (`runtime/wasm/loader.rs` does not exist). `PluginLoader::load_plugin` becomes `pub(super)`; the loader's `.mcp.json` mirror (`mcp_configs` + six accessors + `load_plugin_with_memory`) is deleted.
7. MCP producer is `registrar::mcp_registrar::register_transient_servers(McpManagerHandle, HashMap<String, McpManagerConfig>) -> Result<(Disposer, Vec<(String, ServerStartReceiver)>), String>` (R1.1); it uses a NEW `McpManagerHandle::add_transient_server_detached(config) -> Result<oneshot::Receiver<Result<(), String>>>` so the mount does not await the child's handshake (boot path). The receivers go to `lifecycle.rs::watch_server_starts` (one task per plugin, logs outcomes; P3.3 makes it the `Pending` writer) and `ExtensionManager::activation_settled()` (awaits every watcher; P3.4 uses it at boot; `activation_watchers: StdMutex<Vec<JoinHandle<()>>>` field). A failed server start is logged, not a mount failure (parity with today); a closed actor channel is.
8. Service producer is a method: `ExtensionManager::start_services_effect(&self, plugin_id) -> Disposer` in `service_ops.rs` (the sync `ServiceManager` cannot own the guest-call handles). New `ServiceManager::forget_plugin(plugin_id) -> usize`. `stop_orphaned`, `stop_orphaned_services`, `sync_plugin_services`, `start_autostart_services` are deleted.
9. Memory: `MemoryExtensionRegistry::unregister(name: &str) -> bool` keyed by `MemoryExtension::name()` (= `manifest.name`), not by plugin id — the disposer captures the exact name it registered. Producer: `loader::register_memory_extension_effect(&manifest, server_id, &Arc<MemoryExtensionRegistry>, Option<McpManagerHandle>) -> Result<Option<Disposer>, String>` (binds the caller at registration; `bind_memory_callers` deleted; a duplicate name is now a mount failure, not a warn).
10. Slash: `ToolCatalog::unregister_skills(&[String]) -> usize` (exact ids, not `unregister_skills_for_plugin(plugin_id)` — the catalog has no plugin-id key; the disposer owns the ids it registered; R1.8: P2.8's `plugin_id` on `SkillInfo` is for visibility only and P1 does not need it). `slash_effect::plugin_command_skill_info(&SkillRegistration) -> SkillInfo` is THE builder of a plugin command's `SkillInfo` (R1.3; P2.8 / P4.7b extend it); `plugin_command_skill_infos` is filter + map over it. Producer `slash_effect::register_slash_commands_effect(Arc<ToolCatalog>, Vec<SkillInfo>) -> Disposer`. ToolCatalog reachability = `ExtensionManager::set_tool_catalog(Arc<ToolCatalog>)` (same shape as `set_mcp_handle`), not a `CapabilitySlot`; boot constructs the catalog before the first `load_all` (P1.8).
11. `scopes: crate::sync_primitives::Mutex<HashMap<String, EffectScope>>` (std mutex, never held across `await`), not the registry's `tokio::sync::RwLock`. Transitions serialise on the existing `load_guard`.
12. `reload(&self) -> ExtensionResult<ReloadReport>` (contract: bare `ReloadReport`) — discovery can fail (`collect_plugin_dirs`), and ~10 call sites already `if let Err(e)`. `ReloadReport` gains `summary: LoadSummary`. `load_all` keeps returning `ExtensionResult<LoadSummary>`.
13. `MountError` variants: `NotFound`, `AlreadyMounted`, `Blocked { id, origin }`, `Disabled`, `Parse { id, reason }`, `Step { id, step, reason }`. `UnmountError`: `NotFound`, `NotMounted`.
14. G-2: `PluginStatus` keeps today's names; mount failure is written as `PluginStatus::Error("<step>: <reason>")` at the ONE site `lifecycle.rs::write_failed_row`. Named anchors for later phases (R1.4): `admit` (trust + `plugins.toml` `is_enabled`), `migrate_legacy_disabled_marker`, `build_record` (the one `PluginRecord` construction), `unparsed_record` (the fallback Error row), `write_failed_row`. `Overridden` is not touched here (P3 removes it).
15. A missing handle (MCP / memory registry / catalog) is a recorded skip (`EffectScope::skip` + `Warn` diagnostic), not a mount failure, so CLI paths keep working; P3 should derive `Pending { waiting_on }` from `scope.skipped()` plus the `add_transient_server_detached` receivers.
16. `after_transition` is `async fn` (private) on `ExtensionManager` in `lifecycle.rs`; it does NOT yet call `McpFace::notify_tools_list_changed` — P6.4 adds that one line there and appends `("notify_tools_list_changed(", "lifecycle.rs")` to `projection.rs::tests::G3_PINNED` (a `const` slice, R1.5).
17. `plugins.load` / `plugins.unload` RPC (+ params, handlers, 8 tests, 2 registrations, 2 census rulings) are deleted in P1.10 — zero clients; their only bodies were the functions P1.11 removes. P1.10's commit also carries the `PLUGIN_SYSTEM.md:481-483` sentence (R1.2); it says only what is true at that point (`executeCommand` still exists until P4.7d), so P4.7d edits the same sentence once more.

## Open questions for the lead

Resolved by the reconciliation round (kept here so the record shows what was asked): P5 CUT-list overlap → R1.2 (P1.10 keeps `plugins.{load,unload}`, P5.3 becomes a census); `get_all_commands` deletion → P4 uses `PluginRegistry::get_skill` (R4.4 re-anchors P4.7b onto `plugin_command_skill_info`); CC `plugin.json` without `aleph.runtime` → **P4.15** (R1.6; pointer left in `mount_parsed`); extism empty module / two mocks / eager WASM / `sync_runtime_snapshots` kept → accepted (R1.7); `PluginStatus` rename → dropped (G-2).

Still open:

1. **Doc sentence sequencing (R1.2)**: the `PLUGIN_SYSTEM.md:481-483` text plan-P5P8 P5.3 holds says `executeCommand` is CUT; at P1.10 it is not yet (P4.7d cuts it). P1.10 writes the true-at-that-point wording and P4.7d must edit the sentence again — confirm P4.7d's task lists `PLUGIN_SYSTEM.md:481-483` (its Files list in R4.4 mentions only `EXTENSION_SYSTEM.md:613-709`).
2. **`activation_settled` mechanism** (R1.1 left it to me): a `StdMutex<Vec<JoinHandle<()>>>` drained by the awaiter, re-checked in a loop so a watcher spawned mid-wait is awaited too; finished handles are pruned on every `watch_server_starts`. If P3.4 wants a count rather than a wait, it can read `activation_watchers.len()` — say so and I will expose it.

## Coverage map

| Spec item | Task(s) |
|---|---|
| §3.1 `Disposer` / `EffectScope` / `DisposeReport`, reverse order, fault isolation | P1.1 |
| §3.1 effect 1 registry row ↔ `unregister_plugin` | P1.2 |
| §3.1 effect 2 WASM load ↔ unload | P1.3 |
| §3.1 effect 3 MCP transient server add ↔ remove | P1.4 |
| §3.1 effect 4 service start ↔ stop | P1.5 |
| §3.1 effect 5 memory extension register ↔ **new** `unregister` | P1.6 |
| §3.1 effect 6 slash entries register ↔ **new** `unregister_skills` | P1.7 |
| §3.1 views stay derived; trigger converges to one place | P1.9 (`after_transition`), P1.14 (G3), P1.17 (comment) |
| §3.2 `mount` / `unmount` / `reload_plugin` / `reload`; all-or-none; `set_plugin_enabled` → primitives; watcher + `plugin.reload` / `hooks.reload` → primitives; old narrow twins deleted | P1.9, P1.10, P1.11 |
| §3.2 "每次迁移之后，且只在这里" republish + hook sync | P1.9, P1.14 |
| §3.5 G1 census | P1.12 |
| §3.5 G2 round trip | P1.13 (+ P1.15 for the live MCP surface) |
| §3.5 G3 single author pinned to `lifecycle.rs` | P1.14 |
| §4 mount failure → dispose partial → terminal status; dispose single failure → log + continue; locks `unwrap_or_else(into_inner)` | P1.1, P1.9 |
| §5 unit: disposer order / async / single failure | P1.1 |
| §5 integration: mock MCP plugin enable → disable → enable | P1.15 |
| §6 `qa/plugins/run.sh scope` | P1.16 |
| Boot order prerequisite (handles before first load) | P1.8 |
| R1.1 server-start receivers → `watch_server_starts` / `activation_settled` | P1.4, P1.9 (+ P1.13 awaits it) |
| R1.3 one `SkillInfo` builder for plugin commands | P1.7 |
| R1.4 named admit / migration / record / failed-row sites | P1.9 |
| R1.5 `G3_PINNED` const | P1.14 |
| R1.2 `PLUGIN_SYSTEM.md:481-483` same-commit edit | P1.10 |
| `projection.rs:14-24` rewrite (spec §1.1 / §3.9 "说谎的注释") | P1.17 |
