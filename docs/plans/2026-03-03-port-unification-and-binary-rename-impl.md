# Port Unification & Binary Rename — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Merge Gateway WebSocket and ControlPlane UI onto a single port 18790 via axum, and rename the binary from `aleph-server` to `aleph`.

**Architecture:** Replace the separate `tokio-tungstenite` TCP accept loop with an axum Router that handles both WebSocket upgrades (`/ws`) and static file serving (ControlPlane UI). The axum Router is created inside `GatewayServer`, consuming the existing handler registry and event bus.

**Tech Stack:** axum 0.8 (ws feature), tokio, rust-embed, existing JSON-RPC 2.0 protocol

---

## Task 1: Enable axum WebSocket feature

**Files:**
- Modify: `core/Cargo.toml:161`

**Step 1: Add ws feature to axum dependency**

In `core/Cargo.toml`, change:
```toml
axum = "0.8"
```
to:
```toml
axum = { version = "0.8", features = ["ws"] }
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: SUCCESS (no new errors)

**Step 3: Commit**

```bash
git add core/Cargo.toml Cargo.lock
git commit -m "build: enable axum ws feature for port unification"
```

---

## Task 2: Refactor GatewayServer to use axum Router

This is the core change. Replace the raw TCP accept loop with an axum-based unified server.

**Files:**
- Modify: `core/src/gateway/server.rs`

**Step 1: Add new imports and GatewaySharedState**

At the top of `server.rs`, add axum imports and create a shared state struct that holds everything `ConnectionContext` needs, plus config:

```rust
use axum::{
    Router,
    routing::get,
    extract::{State, ConnectInfo, ws::{WebSocket, WebSocketUpgrade, Message as WsMessage}},
    response::IntoResponse,
};
use super::control_plane::create_control_plane_router;

/// Shared state for axum WebSocket handler
#[derive(Clone)]
pub struct GatewaySharedState {
    pub handlers: Arc<HandlerRegistry>,
    pub event_bus: Arc<GatewayEventBus>,
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    pub subscription_manager: Arc<SubscriptionManager>,
    pub guest_session_manager: Option<Arc<crate::gateway::security::GuestSessionManager>>,
    pub require_auth: bool,
    pub max_connections: usize,
}
```

**Step 2: Add `build_router` method to GatewayServer**

```rust
impl GatewayServer {
    /// Build a unified axum Router with WebSocket + ControlPlane UI routes
    pub fn build_router(&self) -> Router {
        let shared = Arc::new(GatewaySharedState {
            handlers: self.handlers.clone(),
            event_bus: self.event_bus.clone(),
            connections: self.connections.clone(),
            subscription_manager: self.subscription_manager.clone(),
            guest_session_manager: self.guest_session_manager.clone(),
            require_auth: self.config.require_auth,
            max_connections: self.config.max_connections,
        });

        let control_plane = create_control_plane_router();

        Router::new()
            .route("/ws", get(ws_upgrade_handler))
            .fallback_service(control_plane)
            .with_state(shared)
    }
}
```

**Step 3: Add ws_upgrade_handler function**

```rust
/// axum handler for WebSocket upgrade on /ws
async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<GatewaySharedState>>,
) -> impl IntoResponse {
    // Check connection limit before upgrade
    let current = state.connections.read().await.len();
    if current >= state.max_connections {
        warn!("Connection limit reached, rejecting {}", peer_addr);
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Connection limit reached").into_response();
    }

    ws.on_upgrade(move |socket| async move {
        let ctx = ConnectionContext {
            handlers: state.handlers.clone(),
            event_bus: state.event_bus.clone(),
            connections: state.connections.clone(),
            subscription_manager: state.subscription_manager.clone(),
            guest_session_manager: state.guest_session_manager.clone(),
            require_auth: state.require_auth,
        };

        if let Err(e) = handle_connection(socket, peer_addr, ctx).await {
            error!("Connection error from {}: {}", peer_addr, e);
        }
    }).into_response()
}
```

**Step 4: Rewrite `run_until_shutdown` to use axum::serve**

Replace the existing `run_until_shutdown` method:

```rust
pub async fn run_until_shutdown(
    &self,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), GatewayError> {
    let router = self.build_router();

    let listener = tokio::net::TcpListener::bind(&self.addr).await.map_err(|e| {
        GatewayError::BindFailed { addr: self.addr, source: e }
    })?;

    info!("Aleph listening on http://{}", self.addr);
    info!("  WebSocket: ws://{}/ws", self.addr);
    info!("  Panel UI:  http://{}/", self.addr);

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async { let _ = shutdown.await; })
    .await
    .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;

    Ok(())
}
```

Also update `run(&self)`:

```rust
pub async fn run(&self) -> Result<(), GatewayError> {
    let router = self.build_router();

    let listener = tokio::net::TcpListener::bind(&self.addr).await.map_err(|e| {
        GatewayError::BindFailed { addr: self.addr, source: e }
    })?;

    info!("Aleph listening on http://{}", self.addr);

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| GatewayError::ConnectionError(e.to_string()))?;

    Ok(())
}
```

**Step 5: Adapt `handle_connection` to axum ws types**

Change the function signature and message types:

```rust
async fn handle_connection(
    socket: WebSocket,
    peer_addr: SocketAddr,
    ctx: ConnectionContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut write, mut read) = socket.split();
    // ... rest of function
```

Replace all `Message::` variants with `WsMessage::` (the axum import alias):

| Old (tokio-tungstenite) | New (axum ws) |
|---|---|
| `Some(Ok(Message::Text(text)))` | `Some(Ok(WsMessage::Text(text)))` |
| `Some(Ok(Message::Binary(data)))` | `Some(Ok(WsMessage::Binary(data)))` |
| `Some(Ok(Message::Ping(data)))` | `Some(Ok(WsMessage::Ping(data)))` |
| `Some(Ok(Message::Pong(_)))` | `Some(Ok(WsMessage::Pong(_)))` |
| `Some(Ok(Message::Close(frame)))` | `Some(Ok(WsMessage::Close(frame)))` |
| `Some(Ok(Message::Frame(_)))` | **DELETE** (no Frame variant in axum ws) |
| `write.send(Message::Text(s.into()))` | `write.send(WsMessage::Text(s))` |
| `write.send(Message::Pong(data))` | `write.send(WsMessage::Pong(data))` |

Key difference: axum `WsMessage::Text` takes `String` directly, no `.into()` needed.

Also remove the now-unused `accept_loop` method.

**Step 6: Remove unused imports**

Remove:
```rust
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};
```

Keep `tokio::net::TcpListener` if needed by `run_until_shutdown` (it is — used for binding).

**Step 7: Verify compilation**

Run: `cargo check -p alephcore`
Expected: SUCCESS

**Step 8: Run existing tests**

Run: `cargo test -p alephcore --lib gateway::server`
Expected: tests pass (the existing tests don't use the server bind/accept — they test process_request directly)

**Step 9: Commit**

```bash
git add core/src/gateway/server.rs
git commit -m "gateway: unify WS + ControlPlane into single axum Router"
```

---

## Task 3: Update startup sequence

Remove the separate ControlPlane server spawn and simplify startup.

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start/mod.rs:1120-1122`
- Modify: `core/src/bin/aleph_server/commands/start/builder/handlers.rs:317-354`

**Step 1: Remove start_control_plane_server call**

In `start/mod.rs`, remove line 1122:
```rust
start_control_plane_server(&final_bind, final_port, args.daemon).await;
```

And update the startup message. Before the `server.run_until_shutdown(shutdown_rx).await?;` line, if `!args.daemon`, print the unified URL:
```rust
if !args.daemon {
    println!("Aleph Server:");
    println!("  - URL:       http://{}:{}", final_bind, final_port);
    println!("  - WebSocket: ws://{}:{}/ws", final_bind, final_port);
    println!("  - Panel UI:  http://{}:{}/", final_bind, final_port);
    println!();
}
```

**Step 2: Remove start_control_plane_server function**

In `handlers.rs`, delete the entire `start_control_plane_server` function (lines 317-354).

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: SUCCESS

**Step 4: Commit**

```bash
git add core/src/bin/aleph_server/commands/start/mod.rs core/src/bin/aleph_server/commands/start/builder/handlers.rs
git commit -m "server: remove separate ControlPlane server, now served via unified Router"
```

---

## Task 4: Update default port from 18789 to 18790

**Files:**
- Modify: `core/src/bin/aleph_server/cli.rs:43` (CLI default)
- Modify: `core/src/bin/aleph_server/cli.rs:15` (command name)
- Modify: `core/src/bin/aleph_server/cli.rs:196,218,230,238,244,254,269,282,297,307,316` (subcommand URL defaults)
- Modify: `core/src/gateway/config.rs:83` (GatewayServerConfig default)
- Modify: `core/src/bin/aleph_server/commands/start/mod.rs:152` (sentinel value)

**Step 1: Update CLI default port**

In `cli.rs` line 43, change:
```rust
#[arg(long, default_value = "18789")]
```
to:
```rust
#[arg(long, default_value = "18790")]
```

**Step 2: Update CLI command name**

In `cli.rs` line 15, change:
```rust
#[command(name = "aleph-gateway")]
```
to:
```rust
#[command(name = "aleph")]
```

**Step 3: Update all CLI subcommand URL defaults**

Search and replace all occurrences of `ws://127.0.0.1:18789` → `ws://127.0.0.1:18790/ws` in `cli.rs`. There are 11 instances in `GatewayAction::Call`, `ConfigAction::*`, `ChannelsAction::*`, and `CronAction::*`.

**Step 4: Update GatewayServerConfig default**

In `config.rs` line 83, change:
```rust
port: 18789,
```
to:
```rust
port: 18790,
```

**Step 5: Update sentinel value in load_gateway_config**

In `start/mod.rs` line 152, change:
```rust
let final_port = if args.port != 18789 {
```
to:
```rust
let final_port = if args.port != 18790 {
```

**Step 6: Update module-level doc example in gateway/mod.rs**

In `mod.rs` line 20, change:
```rust
//! let addr: SocketAddr = "127.0.0.1:18789".parse().unwrap();
```
to:
```rust
//! let addr: SocketAddr = "127.0.0.1:18790".parse().unwrap();
```

**Step 7: Update doc example in server.rs**

In `server.rs` line 105, change:
```rust
///     let addr: SocketAddr = "127.0.0.1:18789".parse().unwrap();
```
to:
```rust
///     let addr: SocketAddr = "127.0.0.1:18790".parse().unwrap();
```

**Step 8: Verify compilation**

Run: `cargo check -p alephcore`
Expected: SUCCESS

**Step 9: Commit**

```bash
git add core/src/bin/aleph_server/cli.rs core/src/gateway/config.rs core/src/bin/aleph_server/commands/start/mod.rs core/src/gateway/mod.rs core/src/gateway/server.rs
git commit -m "config: change default port from 18789 to 18790"
```

---

## Task 5: Update UI WebSocket URL to same-origin

**Files:**
- Modify: `core/ui/control_plane/src/context.rs:62`

**Step 1: Derive WebSocket URL from window.location**

In `context.rs`, change the hardcoded gateway_url from:
```rust
gateway_url: RwSignal::new("ws://127.0.0.1:18789".to_string()),
```
to a dynamic derivation:
```rust
gateway_url: RwSignal::new(derive_gateway_url()),
```

Add a helper function:
```rust
/// Derive the Gateway WebSocket URL from the current page location.
/// Since the Panel UI and Gateway share the same port, we use same-origin.
fn derive_gateway_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(location) = window.location().host() {
                let protocol = window.location().protocol().unwrap_or_default();
                let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
                return format!("{}//{}/ws", ws_protocol, location);
            }
        }
        // Fallback
        "ws://127.0.0.1:18790/ws".to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "ws://127.0.0.1:18790/ws".to_string()
    }
}
```

**Step 2: Verify WASM compilation**

Run: `cargo check -p aleph-control-plane --target wasm32-unknown-unknown`
Expected: SUCCESS

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/context.rs
git commit -m "ui: derive WS URL from same-origin instead of hardcoded port"
```

---

## Task 6: Rename binary from aleph-server to aleph

**Files:**
- Modify: `core/Cargo.toml:197-198`
- Rename: `core/src/bin/aleph_server/` → `core/src/bin/aleph/`
- Modify: `justfile:14`
- Modify: `apps/macos-native/project.yml` (5 references)
- Modify: `apps/macos-native/Aleph/Server/ServerPaths.swift:24`
- Modify: `apps/macos-native/Aleph/Server/ServerManager.swift` (comments)
- Modify: `apps/macos-native/Aleph/AppDelegate.swift` (comments)

**Step 1: Update Cargo.toml binary definition**

Change:
```toml
[[bin]]
name = "aleph-server"
path = "src/bin/aleph_server/main.rs"
```
to:
```toml
[[bin]]
name = "aleph"
path = "src/bin/aleph/main.rs"
```

**Step 2: Rename the binary source directory**

```bash
cd core && git mv src/bin/aleph_server src/bin/aleph
```

**Step 3: Update internal module declarations**

Search for `mod commands;` and `use crate::commands::` in files under the renamed directory — these use relative paths so the module names shouldn't change. But verify that `main.rs` and any `mod.rs` files don't reference the old directory name.

In `core/src/bin/aleph/main.rs`, check for `use crate::` references. The binary crate name in Cargo.toml `name = "aleph"` means `crate::` refers to the binary crate root. No changes needed since module paths are relative.

**Step 4: Update justfile**

Change line 14:
```
server_bin      := "aleph-server"
```
to:
```
server_bin      := "aleph"
```

**Step 5: Update macOS native app project.yml**

Replace all `aleph-server` with `aleph` in `apps/macos-native/project.yml`:
- `Resources/aleph-server` → `Resources/aleph`
- All build script references

**Step 6: Update ServerPaths.swift**

Change:
```swift
Bundle.main.url(forResource: "aleph-server", withExtension: nil)
```
to:
```swift
Bundle.main.url(forResource: "aleph", withExtension: nil)
```

**Step 7: Update comments in Swift files**

In `ServerManager.swift` and `AppDelegate.swift`, replace comment references from `aleph-server` to `aleph`.

**Step 8: Update CLI tests**

In `cli.rs` tests, replace all `"aleph-gateway"` test strings with `"aleph"`:
```rust
// All test_cli_parses_* tests use:
Args::try_parse_from(["aleph-gateway", ...])
// Change to:
Args::try_parse_from(["aleph", ...])
```

**Step 9: Verify compilation**

Run: `cargo check -p alephcore`
Expected: SUCCESS

**Step 10: Run tests**

Run: `cargo test -p alephcore --lib cli`
Expected: PASS

**Step 11: Commit**

```bash
git add -A
git commit -m "rename: aleph-server → aleph (binary and all references)"
```

---

## Task 7: Update documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/reference/SERVER_DEVELOPMENT.md`
- Modify: `docs/reference/GATEWAY.md` (if references exist)

**Step 1: Update CLAUDE.md**

Replace all `aleph-server` references with `aleph`:
- `cargo run --bin aleph-server` → `cargo run --bin aleph`
- Any other mentions

Update port references:
- `18789` → `18790` where appropriate (Gateway default port)
- Remove mentions of separate ControlPlane port

**Step 2: Update SERVER_DEVELOPMENT.md**

Replace binary name and port references.

**Step 3: Update GATEWAY.md**

If it references port 18789 or separate ControlPlane port, update.

**Step 4: Commit**

```bash
git add CLAUDE.md docs/reference/
git commit -m "docs: update binary name and port references"
```

---

## Task 8: Final verification

**Step 1: Full compilation check**

Run: `cargo check -p alephcore`
Expected: SUCCESS

**Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (except pre-existing markdown_skill failures)

**Step 3: Verify binary builds**

Run: `cargo build --bin aleph`
Expected: SUCCESS, binary at `target/debug/aleph`

**Step 4: Verify binary runs**

Run: `target/debug/aleph --help`
Expected: Shows help text with "aleph" as command name

**Step 5: Verify WASM builds**

Run: `cargo check -p aleph-control-plane --target wasm32-unknown-unknown`
Expected: SUCCESS

**Step 6: Commit any remaining fixes**

If any issues found, fix and commit.
