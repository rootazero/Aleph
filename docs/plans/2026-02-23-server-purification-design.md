# Aleph Server Purification: Remove Desktop Control, Embrace MCP Plugins

> Date: 2026-02-23
> Status: Approved

## Motivation

Aleph was originally designed to control desktop computers, which led to desktop API control components (perception, input simulation, browser automation via CDP). The project is now refocused as a **pure server** — all desktop control capabilities are removed, and browser automation is delegated to Playwright via MCP plugin.

## Core Decisions

1. **Delete all desktop perception components** — AX tree, screenshots, State Bus, PAL, input simulation, vision/OCR
2. **Delete browser module** — Remove chromiumoxide CDP; replace with Playwright MCP Server (zero Rust code)
3. **Simplify execution architecture** — Remove Client-side tool routing (ExecutionPolicy, ToolRouter, RoutedExecutor, ReverseRpc, ClientManifest); all tools execute on Server
4. **Playwright via MCP** — Configuration-only integration, leveraging existing MCP Client

## Deletion List

### Entire Directories/Files to Delete

| Category | Path | Description |
|----------|------|-------------|
| Perception | `core/src/perception/` (entire) | AX tree, screenshots, State Bus, PAL, input simulation, ActionDispatcher |
| Vision | `core/src/vision/` (entire) | OCR, vision analysis service |
| Browser | `core/src/browser/` (entire) | chromiumoxide CDP automation (1695+ lines) |
| Browser RPC | `core/src/gateway/handlers/browser.rs` | browser.* JSON-RPC methods |
| State Bus RPC | `core/src/gateway/handlers/state_bus.rs` | State Bus subscription handlers |
| OCR RPC | `core/src/gateway/handlers/ocr.rs` | OCR handler (depends on vision) |
| Snapshot Tool | `core/src/builtin_tools/snapshot_capture.rs` | AX tree + OCR snapshot |
| Canvas Tool | `core/src/builtin_tools/canvas/` (entire) | A2UI protocol, WebView rendering |
| ReverseRpc | `core/src/gateway/reverse_rpc.rs` | Server→Client reverse RPC |
| ClientManifest | `core/src/gateway/client_manifest.rs` | Client capability declaration |
| ToolRouter | `core/src/executor/router.rs` | Tool routing decision engine |
| RoutedExecutor | `core/src/executor/routed_executor.rs` | Routed execution wrapper |
| ExecutionPolicy | `core/src/dispatcher/types/execution_policy.rs` | Client/Server routing policy enum |

### Cargo.toml Changes

Remove dependencies:
- `chromiumoxide` (browser feature)
- `accessibility-sys` (macOS)
- `core-foundation` (macOS)
- `core-graphics` (macOS)

Remove feature flag:
- `browser = ["dep:chromiumoxide", "gateway"]`

## Modification List

### High Impact (Structural Changes)

| File | Changes |
|------|---------|
| `core/src/lib.rs` | Remove `pub mod perception`, `pub mod vision`, `#[cfg(feature = "browser")] pub mod browser` |
| `core/src/core/types.rs` | Remove `PerceptionRef` struct, `FocusHint` import, `CapturedContext.perception` field |
| `core/src/gateway/execution_engine.rs` | Remove `ClientContext.manifest` / `reverse_rpc` fields; remove `ToolRouter` / `RoutedExecutor` references; simplify to use `SingleStepExecutor` directly |
| `core/src/gateway/server.rs` | Remove `SessionConnection.manifest` / `reverse_rpc` fields and related methods |
| `core/src/gateway/mod.rs` | Remove `reverse_rpc` / `client_manifest` module declarations and re-exports |

### Medium Impact (Remove References)

| File | Changes |
|------|---------|
| `core/src/gateway/handlers/mod.rs` | Remove browser/state_bus/ocr handler registrations |
| `core/src/gateway/handlers/auth.rs` | Remove `ClientManifest` parameter |
| `core/src/executor/mod.rs` | Remove `routed_executor` / `router` exports |
| `core/src/dispatcher/types/mod.rs` | Remove `ExecutionPolicy` export |
| `core/src/dispatcher/mod.rs` | Update re-exports |

### Low Impact (Tool Registration)

| File | Changes |
|------|---------|
| `core/src/builtin_tools/mod.rs` | Remove `snapshot_capture` / `canvas` module declarations and all re-exports |

## Architecture Change

**Before:**
```
Agent Loop → Dispatcher → RoutedExecutor → ToolRouter → { Server | Client }
                                              ↑
                                    ExecutionPolicy + ClientManifest
```

**After:**
```
Agent Loop → Dispatcher → SingleStepExecutor → Server local execution
                              ↑
                        MCP Client → Playwright MCP Server (plugin)
```

All tools execute on Server. Browser operations are transparently delegated to Playwright via MCP protocol.

## Playwright MCP Integration

Zero-code approach — configure in `~/.aleph/config.toml`:

```toml
[[mcp.servers]]
name = "playwright"
command = "npx"
args = ["-y", "@anthropic/playwright-mcp", "--headless"]
```

Agent accesses Playwright capabilities through existing MCP tool calls:
- `browser_navigate` — Navigate to URL
- `browser_screenshot` — Screenshot
- `browser_click` — Click element
- `browser_type` — Type text
- `browser_evaluate` — Execute JavaScript

## Out of Scope

These components are **not affected**:
- `exec/` — Shell execution security (pure server capability)
- `builtin_tools/web_fetch.rs` — HTTP fetch (not browser-based)
- `builtin_tools/bash_exec.rs` — Command execution
- `mcp/` — MCP Client (core capability, serves Playwright)
- `extension/` — Plugin system
- Gateway core — WebSocket, Session, Event Bus
