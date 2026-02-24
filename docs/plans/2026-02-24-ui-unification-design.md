# Aleph UI Unification Design

> "美感由 CSS/Leptos 定义一次，力量由 Swift/Rust 分别提供。"

**Date**: 2026-02-24
**Status**: Approved
**Scope**: Settings UI, Halo UI, macOS App, Tauri App, Control Plane

---

## Problem

Aleph currently maintains three separate UI codebases:

| System | Tech Stack | Settings UI Status |
|--------|-----------|-------------------|
| Control Plane (Web) | Leptos/WASM + Tailwind | Complete — 19 pages, 40+ RPC methods |
| macOS App | Swift/SwiftUI + AppKit | Deleted (Feb 9, 2026) |
| Tauri App | React + Radix UI + Tailwind | Never implemented (infrastructure ready) |

This creates unsustainable maintenance burden. Every new feature (image display, file upload, markdown rendering) must be implemented in up to three different UI frameworks.

## Decision

**Leptos/WASM becomes the single UI codebase.** Native apps become thin shells that host WebViews.

### Tech Stack Evolution

```
Before: Rust + Swift/SwiftUI + TypeScript/React + Leptos/WASM (4 UI technologies)
After:  Rust + Leptos/WASM (1 UI) + Swift (native shell + desktop capabilities)
```

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│           Aleph Server (Rust Core + Leptos UI Hub)       │
│                                                          │
│  Core: Gateway, Agent Loop, Memory, Tool System          │
│  UI:   Leptos/WASM (compiled into server binary)         │
│        ├── /settings  → 19 settings pages (complete)     │
│        ├── /halo      → Conversation window (new)        │
│        └── /dashboard → Status panel (exists)            │
│                                                          │
│  Ports: 18789 (Gateway WS) + 18790 (UI HTTP)            │
└──────────────┬──────────────────────┬───────────────────┘
               │                      │
    ┌──────────┴──────┐    ┌──────────┴──────────┐
    │ macOS Shell      │    │ Tauri Shell          │
    │ (Swift)          │    │ (Rust + WebView)     │
    │                  │    │                      │
    │ • Menu bar icon  │    │ • System tray        │
    │ • Global hotkeys │    │ • Window management  │
    │ • WKWebView      │    │ • WebView windows    │
    │ • Desktop Bridge │    │                      │
    │   (UDS: capture, │    │ Loads:               │
    │    AXTree, input) │    │  localhost:18790/*   │
    │                  │    │                      │
    │ Loads:           │    │ Platforms: Win/Linux  │
    │  localhost:18790/*│    └─────────────────────┘
    │                  │
    │ Platform: macOS  │
    └─────────────────┘
```

## Component Responsibilities

### A. Aleph Server + Leptos UI Hub (single UI codebase)

| Route | Purpose | Status |
|-------|---------|--------|
| `/settings/*` | 19 settings pages | Complete 100% |
| `/dashboard` | Status/monitoring panel | Complete |
| `/halo` | Conversation window (streaming, markdown, images) | **New** |

All UI compiles to WASM, embedded in server binary via `rust-embed`.

### B. macOS Shell (Swift) — "Super Host"

**Retained native functions:**
- `NSStatusItem` menu bar icon
- `NSEvent` global hotkey listener
- `NSWindow` transparent/titlebar-less windows (hosting WKWebView)
- Desktop Bridge (UDS): screen capture, OCR, AXTree, input simulation

**Removed:**
- All SwiftUI settings pages (already deleted)
- SwiftUI Halo conversation UI (migrated to Leptos in Phase 3)

### C. Tauri Shell (Windows/Linux) — "Lightweight Host"

**Retained:**
- System tray + window management (Rust)
- WebView windows loading Leptos UI

**Removed:**
- React code, component library, Vite config, node_modules
- Tauri backend Rust code retained (window management, settings persistence)

## Migration Path

### Phase 1: Settings UI Flash Unification

**Goal:** Settings accessible inside native apps via WebView. Zero new Leptos code needed.

**macOS App changes:**
- Add "Settings..." menu item (Cmd+,)
- On click: open `NSWindow` containing `WKWebView` pointing to `localhost:18790/settings`
- Window config: 900x650, resizable, titled "Aleph Settings", centered
- Handle server-not-running: show native `NSAlert` with "Start Server?" option

**Tauri App changes:**
- Settings window WebView loads `localhost:18790/settings` instead of React
- Remove React settings components (they were never implemented)

### Phase 2: Desktop Bridge Recovery (independent track)

**Goal:** Restore macOS desktop control capabilities via UDS protocol.

**Rust Core side:**
- UDS client in `core/src/capability/`
- `DesktopProvider` trait defining the capability interface
- Protocol: `AlephRequest` / `AlephResponse` enums (Perception, Action, Canvas categories)

**Swift App side:**
- UDS server in `apps/macos/AlephBridge/`
- Implement: screen capture, OCR (Vision.framework), AXTree, input simulation (CoreGraphics)

**This is independent of UI unification and can proceed in parallel.**

### Phase 3: Halo UI Unification

**Goal:** Single Leptos-based conversation UI for all platforms.

**Leptos side:**
- New `/halo` route in Control Plane
- Components: message bubbles, streaming text, markdown rendering, image display, file upload
- CSS: transparent background support, spinner animations, responsive sizing
- WebSocket integration for real-time streaming

**macOS App changes:**
- Halo window switches from native SwiftUI to WKWebView loading `localhost:18790/halo`
- Native window properties retained: transparent, always-on-top, no titlebar
- Bridge: WKWebView ↔ Swift communication for clipboard, context capture

**Tauri App changes:**
- Remove ALL React code, Vite config, node_modules
- Halo window WebView loads `localhost:18790/halo`

**Cleanup:**
- Delete `apps/desktop/src/` React source
- Delete `apps/desktop/halo.html`, `settings.html`, `index.html`
- Simplify `apps/desktop/vite.config.ts` or remove entirely
- Delete Swift `HaloViewV2.swift` and related view files

## Offline / Server Not Running

When Server is not running, WebView fails to load. Shells handle this:

- **macOS**: Native `NSAlert` — "Aleph Server is not running. Start now?"
  - If Core is bundled (Sidecar), auto-launch it
  - If Core is separate, show instructions
- **Tauri**: Built-in HTML error page with retry button

## What Gets Deleted

### macOS App (Swift) — Phase 1
- Already deleted: 11 settings view files

### macOS App (Swift) — Phase 3
- `HaloViewV2.swift` and all Halo-related SwiftUI views
- `HaloStreamingView.swift`, `HaloHistoryListView.swift`
- `Components/Atoms/` UI components that only served Halo
- `HaloState.swift` state management (replaced by Leptos signals)

### Tauri App — Phase 3
- `src/` — entire React source directory
- `src/components/ui/` — Radix UI components
- `src/lib/gateway.ts` — TypeScript RPC client (Leptos uses its own)
- `src/stores/` — Zustand stores
- `src/windows/` — React window implementations
- `halo.html`, `settings.html` — React entry points
- `package.json`, `pnpm-lock.yaml`, `node_modules/`
- `vite.config.ts`, `tailwind.config.ts`, `postcss.config.js`
- `tsconfig*.json`

### What Stays in Tauri
- `src-tauri/` — Rust backend (window management, tray, system integration)
- `tauri.conf.json` — window definitions (updated to load Leptos URLs)

## Key Design Decisions

1. **Why not keep React for Tauri?** — Maintaining two frontend frameworks (React + Leptos) defeats the purpose of unification. Sunk cost should not drive architecture.

2. **Why WebView for Halo?** — WKWebView/WebView2 performance is sufficient for chat UI. CSS animations match native quality for spinners and transitions. The alternative (maintaining Swift Halo + Leptos Halo) creates the exact duplication problem we're solving.

3. **Why phased migration?** — Phase 1 (settings) is zero-risk and immediately reduces maintenance. Phase 3 (Halo) requires building new Leptos components and validating quality. Decoupling them reduces risk.

4. **Why not embed WASM directly in apps?** — Loading from localhost keeps a single source of truth. The server is always running (it's the brain). This also enables remote management (settings from another device).
