# DEPRECATED — macOS Swift App

> **Status:** Deprecated as of 2026-02-25
> **Replaced by:** `aleph-server` (Rust, single binary) + `aleph-bridge` (Tauri, cross-platform)

## Why Deprecated

The Server-Centric Build Architecture replaces this Swift app with:

1. **aleph-server** — The AI brain. Single Rust binary with Gateway, Agent Loop, Panel UI (Leptos/WASM), and Bridge Manager.
2. **aleph-bridge** (Tauri) — The system shell. Provides desktop capabilities (screenshot, OCR, keyboard, mouse, AX inspection, canvas overlay, tray, hotkey) via UDS JSON-RPC 2.0.

## What Replaced What

| Swift Component | New Location | Technology |
|-----------------|-------------|------------|
| ScreenCaptureKit | `apps/desktop/src-tauri/src/bridge/perception.rs` | xcap (cross-platform) |
| Vision (OCR) | `apps/desktop/src-tauri/src/bridge/perception.rs` | objc + Vision (macOS) |
| Accessibility (AX tree) | `apps/desktop/src-tauri/src/bridge/perception.rs` | objc + AX API (macOS) |
| CoreGraphics (mouse/keyboard) | `apps/desktop/src-tauri/src/bridge/action.rs` | enigo (cross-platform) |
| NSWorkspace (app launch) | `apps/desktop/src-tauri/src/bridge/action.rs` | std::process::Command |
| WKWebView (canvas overlay) | `apps/desktop/src-tauri/src/bridge/canvas.rs` | Tauri WebView |
| NSStatusItem (tray) | `apps/desktop/src-tauri/src/tray.rs` | Tauri tray plugin |
| All SwiftUI business pages | `core/ui/control_plane/` | Leptos/WASM |

## Preserved For Reference

The Swift implementations in `Sources/DesktopBridge/` contain well-documented examples of Vision OCR, Accessibility tree walking, CoreGraphics event generation, and Canvas overlay. These serve as reference for the Rust/Tauri equivalents.

## Do Not Delete

Keep this directory until the Tauri bridge has been battle-tested in production.
