# P0 File Refactoring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor two P0 files (`commands/start.rs` and `extension/mod.rs`) to align with the patterns defined in `docs/CODE_ORGANIZATION.md`, reducing file complexity and improving maintainability.

**Architecture:** Apply Pattern D (Builder Split) to `start.rs` — extract the 700-line monolithic `start_server` function into a `ServerBuilder` that orchestrates named subsystem initializers. Apply Pattern C (Manager Facade) to `extension/mod.rs` — the existing sub-modules are already well structured; the issue is that `mod.rs` acts as a God Object accumulating all 46 methods. Reorganize so `mod.rs` stays thin and delegates to its existing sub-components.

**Tech Stack:** Rust, Tokio, `#[cfg(feature = "...")]` conditional compilation, `Arc<T>` shared ownership.

**Reference:** `docs/CODE_ORGANIZATION.md` — Pattern C and Pattern D sections.

---

## Task 1: Introduce `ServerBuilder` struct in `start.rs`

**Context:** `start_server()` in `core/src/bin/aleph_server/commands/start.rs` (L82–L789) initializes 10+ subsystems inline. The goal is to extract each subsystem into a named method on a new `ServerBuilder` struct, making the main function a thin orchestrator.

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start.rs`

**Step 1: Read the current file**

Read `core/src/bin/aleph_server/commands/start.rs` fully to understand the flow.

**Step 2: Define the `ServerBuilder` struct**

Add the following struct definition just above the `start_server` function (around L80). This struct accumulates the initialized state:

```rust
#[cfg(feature = "gateway")]
struct ServerBuilder<'a> {
    args: &'a Args,
    server: GatewayServer,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    full_config: FullGatewayConfig,
    final_bind: String,
    final_port: u16,
    session_manager: Arc<SessionManager>,
    run_manager: Arc<AgentRunManager>,
    router: Arc<AgentRouter>,
}
```

**Step 3: Extract `initialize_config()` method**

Extract lines L120–L196 (config loading + banner printing) into:

```rust
#[cfg(feature = "gateway")]
impl<'a> ServerBuilder<'a> {
    fn initialize_config(args: &'a Args) -> Result<(FullGatewayConfig, String, u16), Box<dyn std::error::Error>> {
        // Load configuration from file or defaults
        let full_config = match &args.config { ... };
        let final_bind = ...;
        let final_port = ...;
        Ok((full_config, final_bind, final_port))
    }
}
```

**Step 4: Extract `initialize_session_manager()` method**

Extract lines L206–L228 (SessionManager setup) into a standalone async function:

```rust
#[cfg(feature = "gateway")]
async fn initialize_session_manager(daemon: bool) -> Arc<SessionManager> {
    match SessionManager::with_defaults() { ... }
}
```

**Step 5: Extract `initialize_extension_manager()` method**

Extract lines L230–L248 (ExtensionManager setup) into:

```rust
#[cfg(feature = "gateway")]
async fn initialize_extension_manager(daemon: bool) {
    match alephcore::extension::ExtensionManager::with_defaults().await { ... }
}
```

**Step 6: Extract `register_agent_handlers()` method**

Extract lines L250–L351 (provider check + agent.run/status/cancel handler registration) into:

```rust
#[cfg(feature = "gateway")]
async fn register_agent_handlers(
    server: &mut GatewayServer,
    session_manager: Arc<SessionManager>,
    event_bus: Arc<...>,
    router: Arc<AgentRouter>,
    full_config: &FullGatewayConfig,
    daemon: bool,
) -> Arc<AgentRunManager> { ... }
```

**Step 7: Extract `register_poe_handlers()` method**

Extract lines L374–L476 (POE services setup) into:

```rust
#[cfg(feature = "gateway")]
async fn register_poe_handlers(
    server: &mut GatewayServer,
    event_bus: Arc<...>,
    daemon: bool,
) { ... }
```

**Step 8: Extract `initialize_auth_context()` method**

Extract lines L478–L593 (DeviceStore, SecurityStore, TokenManager, PairingManager, mDNS) into:

```rust
#[cfg(feature = "gateway")]
fn initialize_auth_context(
    port: u16,
    event_bus: Arc<...>,
    require_auth: bool,
    daemon: bool,
) -> (Arc<auth_handlers::AuthContext>, Option<MdnsBroadcaster>, ...) { ... }
```

**Step 9: Extract `initialize_app_config()` method**

Extract lines L545–L592 (secrets vault migration + config loading) into:

```rust
#[cfg(feature = "gateway")]
fn initialize_app_config(daemon: bool) -> alephcore::Config { ... }
```

**Step 10: Extract `initialize_channels()` method**

Extract lines L617–L699 (ChannelRegistry + channel registration) into:

```rust
#[cfg(feature = "gateway")]
async fn initialize_channels(
    server: &mut GatewayServer,
    daemon: bool,
) -> Arc<ChannelRegistry> { ... }
```

**Step 11: Extract `initialize_inbound_router()` method**

Extract lines L702–L736 (PairingStore + InboundMessageRouter) into:

```rust
#[cfg(feature = "gateway")]
async fn initialize_inbound_router(
    channel_registry: Arc<ChannelRegistry>,
    router: Arc<AgentRouter>,
    daemon: bool,
) { ... }
```

**Step 12: Rewrite `start_server()` as orchestrator**

Replace the body of `start_server()` with calls to the extracted functions:

```rust
#[cfg(feature = "gateway")]
pub async fn start_server(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    initialize_tracing(args);
    check_port_availability(args)?;

    let (full_config, final_bind, final_port) = initialize_config(args)?;
    alephcore::pii::PiiEngine::init(full_config.privacy.clone());

    let mut server = GatewayServer::with_config(addr, config);
    let event_bus = server.event_bus().clone();

    let session_manager = initialize_session_manager(args.daemon).await;
    initialize_extension_manager(args.daemon).await;

    let router = Arc::new(AgentRouter::new());
    let run_manager = register_agent_handlers(&mut server, session_manager.clone(), event_bus.clone(), router.clone(), &full_config, args.daemon).await;
    register_poe_handlers(&mut server, event_bus.clone(), args.daemon).await;

    let (auth_ctx, mdns_broadcaster, invitation_manager, guest_session_manager) =
        initialize_auth_context(final_port, event_bus.clone(), full_config.gateway.require_auth, args.daemon);
    register_auth_handlers(&mut server, &auth_ctx);
    register_guest_handlers(&mut server, &invitation_manager, &guest_session_manager, &event_bus);
    server.set_guest_session_manager(guest_session_manager.clone());

    let app_config = initialize_app_config(args.daemon);
    let app_config = Arc::new(tokio::sync::RwLock::new(app_config));
    register_config_handlers(&mut server, app_config, event_bus.clone(), device_store.clone());

    register_session_handlers(&mut server, &session_manager);

    let channel_registry = initialize_channels(&mut server, args.daemon).await;
    initialize_inbound_router(channel_registry, router.clone(), args.daemon).await;

    let _config_watcher = setup_config_watcher(&mut server, config_path, &event_bus, args.daemon).await;
    start_webchat_server(args, &final_bind, final_port).await;

    #[cfg(feature = "control-plane")]
    start_control_plane_server(&final_bind, final_port, args.daemon).await;

    setup_shutdown_handlers(args, mdns_broadcaster).await;
    server.run_until_shutdown(shutdown_rx).await?;
    Ok(())
}
```

**Step 13: Build and verify**

```bash
cargo build --bin aleph-server 2>&1 | head -50
```
Expected: Builds successfully with 0 errors.

**Step 14: Verify line count of `start_server()`**

```bash
grep -n "^pub async fn start_server\|^}" core/src/bin/aleph_server/commands/start.rs | head -20
```
Expected: `start_server` body spans fewer than 60 lines.

**Step 15: Commit**

```bash
git add core/src/bin/aleph_server/commands/start.rs
git commit -m "refactor: extract ServerBuilder subsystem initializers from start_server"
```

---

## Task 2: Audit `extension/mod.rs` and thin out the facade

**Context:** `extension/mod.rs` already has sub-modules (`service_manager.rs`, `registry.rs`, `loader.rs`, `plugin_loader.rs`). The 46 public methods on `ExtensionManager` are a mix of direct delegation to these sub-components and inline logic. The goal is to ensure `mod.rs` is a thin facade that delegates rather than reimplementing.

**Files:**
- Read: `core/src/extension/mod.rs` (full)
- Read: `core/src/extension/service_manager.rs`
- Read: `core/src/extension/registry.rs`

**Step 1: Read the full `ExtensionManager` impl**

Read `core/src/extension/mod.rs` lines 165–1159 to catalog all 46 methods. Group them:
- Methods that directly call `self.service_manager` → already delegating
- Methods that directly call `self.registry` → already delegating
- Methods that contain inline logic → candidates for extraction

**Step 2: Identify inline logic not delegating to sub-modules**

Look for `impl ExtensionManager` methods that:
- Have more than 10 lines of body
- Do NOT call `self.service_manager`, `self.registry`, `self.loader`, `self.plugin_loader`, or `self.hook_executor`

These are the methods to move.

**Step 3: Move `load_all()` logic to `loader.rs`**

`load_all()` (L197–~L350) iterates through discovered skill/command/agent/plugin dirs and calls `self.loader.*`. This coordination logic belongs in a new method on `ComponentLoader`:

```rust
// In extension/loader.rs
impl ComponentLoader {
    pub async fn load_all(
        &self,
        discovery: &DiscoveryManager,
        registry: &Arc<RwLock<ComponentRegistry>>,
        hook_executor: &Arc<RwLock<HookExecutor>>,
    ) -> ExtensionResult<LoadSummary> { ... }
}
```

Then `ExtensionManager::load_all()` becomes:

```rust
pub async fn load_all(&self) -> ExtensionResult<LoadSummary> {
    let mut cache = self.cache_state.write().await;
    let summary = self.loader.load_all(&self.discovery, &self.registry, &self.hook_executor).await?;
    cache.loaded = true;
    cache.loaded_at = Some(Instant::now());
    Ok(summary)
}
```

**Step 4: Verify `ExtensionManager` method count after delegation**

After Step 3, verify that each method in `impl ExtensionManager` is either:
- A `new()` / `with_defaults()` constructor
- A thin delegation: `self.sub_component.method(...).await`
- A facade combining 2–3 sub-component calls

Any method with more than 15 lines should be reconsidered.

**Step 5: Add section comments to organize the impl block**

Even if all methods stay, organize them with `// --- Lifecycle ---`, `// --- Skill/Command/Agent queries ---`, `// --- Plugin execution ---`, `// --- Service management ---`, `// --- Hook execution ---` section headers. This is a low-risk improvement that makes the God Object problem visible to future maintainers.

**Step 6: Build and test**

```bash
cargo build 2>&1 | head -30
cargo test -p alephcore extension 2>&1 | tail -20
```
Expected: All green.

**Step 7: Commit**

```bash
git add core/src/extension/mod.rs core/src/extension/loader.rs
git commit -m "refactor: thin extension/mod.rs facade, move load_all coordination to ComponentLoader"
```

---

## Task 3: Extract `commands/start.rs` to a `builder/` submodule (optional follow-up)

**Context:** This task is only needed if Task 1 results in `start.rs` still being over 500 lines due to the many `register_*` functions remaining at the bottom of the file (L792–L1664).

**Files:**
- Create: `core/src/bin/aleph_server/commands/builder/mod.rs`
- Create: `core/src/bin/aleph_server/commands/builder/handlers.rs`
- Modify: `core/src/bin/aleph_server/commands/start.rs`

**Step 1: Move `register_*` functions to `builder/handlers.rs`**

Move these standalone functions from `start.rs` to the new file:
- `register_auth_handlers()` (L792–L838)
- `register_guest_handlers()` (L840–L896)
- `register_session_handlers()` (L898–L930)
- `register_channel_handlers()` (L932–L1020)
- `register_config_handlers()` (L1253–end)

**Step 2: Create `builder/mod.rs` that re-exports**

```rust
// core/src/bin/aleph_server/commands/builder/mod.rs
mod handlers;
pub(super) use handlers::*;
```

**Step 3: Update `start.rs` to use new module**

```rust
// In start.rs, add:
mod builder;
use builder::*;
```

**Step 4: Check line counts**

```bash
wc -l core/src/bin/aleph_server/commands/start.rs core/src/bin/aleph_server/commands/builder/*.rs
```
Expected: `start.rs` under 200 lines, `handlers.rs` under 400 lines.

**Step 5: Build and commit**

```bash
cargo build --bin aleph-server && \
git add core/src/bin/aleph_server/commands/start.rs core/src/bin/aleph_server/commands/builder/ && \
git commit -m "refactor: move register_* handler functions to commands/builder/handlers.rs"
```

---

## Completion Checklist

- [ ] `start_server()` function body < 60 lines
- [ ] Each extracted initializer function < 100 lines
- [ ] `extension/mod.rs` `impl ExtensionManager` methods organized with section comments
- [ ] `load_all()` logic delegated to `ComponentLoader`
- [ ] `cargo build` passes with 0 errors
- [ ] `cargo test` passes with 0 failures
- [ ] All commits reference the refactoring pattern used

---

*Created: 2026-02-23. Implements patterns from `docs/CODE_ORGANIZATION.md`.*
