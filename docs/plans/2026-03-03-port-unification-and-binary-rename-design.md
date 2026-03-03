# Port Unification & Binary Rename Design

> Date: 2026-03-03
> Status: Approved

## Summary

Merge Gateway WebSocket (18789) and ControlPlane UI (18790) into a single port **18790**, and rename the binary from `aleph-server` to `aleph`.

## Context

Currently Aleph runs two separate servers:
- **Gateway**: `tokio-tungstenite` WebSocket on port 18789 (JSON-RPC 2.0)
- **ControlPlane UI**: `axum` HTTP on port 18790 (rust-embed static files, hardcoded as `port+1`)

OpenClaw serves both UI and WebSocket on a single port. Aleph should do the same for simplicity.

## Design

### Part 1: Unified Port (18790)

Single `axum::Router` serves both HTTP and WebSocket:

```
axum::Router (port 18790)
├── GET /ws              → WebSocket upgrade → reuse handle_connection logic
├── GET /                → ControlPlane index.html (rust-embed)
└── GET /*path           → ControlPlane static assets / SPA fallback
```

#### Key Changes

1. **`GatewayServer`** (`core/src/gateway/server.rs`)
   - Stop binding TCP directly; expose WS logic as axum handler
   - New method: `fn into_axum_router(self) -> axum::Router` or `fn run_with_router(self, extra: Router)`
   - Merge ControlPlane router into the same Router

2. **WebSocket handling**
   - Use `axum::extract::ws::WebSocketUpgrade` or bridge `tokio-tungstenite` via `on_upgrade`
   - Existing `handle_connection` JSON-RPC logic unchanged — only the TCP accept layer changes

3. **Startup** (`start/mod.rs`)
   - Remove `start_control_plane_server()` call
   - `server.run_until_shutdown()` serves both WS + static files on one port

4. **UI WebSocket URL** (`core/ui/control_plane/src/context.rs:62`)
   - Change from `ws://127.0.0.1:18789` to derive from `window.location` (same-origin `/ws`)

5. **Default port**
   - `cli.rs` `--port` default: 18789 → 18790
   - `GatewayServerConfig` default: 18789 → 18790

6. **macOS native app** — already uses 18790, no changes needed

### Part 2: Binary Rename (`aleph-server` → `aleph`)

| File | Change |
|------|--------|
| `core/Cargo.toml` | `name = "aleph-server"` → `name = "aleph"` |
| `core/src/bin/aleph_server/` | Rename directory to `core/src/bin/aleph/` |
| `justfile` | `server_bin := "aleph-server"` → `server_bin := "aleph"` |
| `apps/macos-native/project.yml` | All `aleph-server` refs → `aleph` |
| `apps/macos-native/Aleph/Server/ServerPaths.swift` | `forResource: "aleph-server"` → `forResource: "aleph"` |
| `apps/macos-native/Aleph/Server/ServerManager.swift` | Comment updates |
| `apps/macos-native/Aleph/AppDelegate.swift` | Comment updates |
| `CLAUDE.md` | `cargo run --bin aleph-server` → `cargo run --bin aleph` |
| `docs/reference/SERVER_DEVELOPMENT.md` | Sync updates |

## Non-Goals

- No REST API endpoints (future work)
- No TLS support changes
- WebChat server remains as-is (already defaults to same port)
